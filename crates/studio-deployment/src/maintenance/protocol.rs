use super::{RunStatus, StepReport, StepStatus};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

pub(super) const IDS: [&str; 8] = [
    "brew_linkage_audit",
    "cli_versions",
    "github_cli_extensions",
    "github_cli_skills",
    "helm",
    "tldr",
    "llvm_head",
    "llvm_stable",
];
pub(super) const EVENT_LIMIT: u64 = 16 * 1024;

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Request {
    pub schema_version: u8,
    pub run_id: String,
    pub steps: Vec<String>,
    pub review_digest: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub llvm_plan_digest: Option<String>,
}

pub(super) fn manual_llvm(id: &str) -> bool {
    matches!(id, "llvm_head" | "llvm_stable")
}

pub(super) fn valid_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

pub(super) fn valid_run_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

pub(super) fn validate_request(request: &Request, run_id: &str) -> Result<()> {
    ensure!(
        request.schema_version == 1
            && request.run_id == run_id
            && valid_run_id(run_id)
            && valid_hex(&request.review_digest, 64)
            && !request.steps.is_empty()
            && request.steps.len() <= IDS.len()
            && request.steps.iter().all(|id| IDS.contains(&id.as_str()))
            && request.steps.windows(2).all(|pair| pair[0] < pair[1]),
        "Invalid Topgrade maintenance request"
    );
    let llvm = request.steps.iter().any(|id| manual_llvm(id));
    ensure!(
        match &request.llvm_plan_digest {
            Some(digest) => llvm && request.steps.len() == 1 && valid_hex(digest, 64),
            None => !llvm,
        },
        "LLVM changes require a single reviewed candidate"
    );
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Event {
    schema_version: u8,
    run_id: String,
    seq: usize,
    kind: String,
    #[serde(deserialize_with = "nullable")]
    step: Option<String>,
    #[serde(deserialize_with = "nullable")]
    status: Option<String>,
}

fn nullable<'de, D: serde::Deserializer<'de>>(input: D) -> Result<Option<String>, D::Error> {
    Option::deserialize(input)
}

#[derive(Debug, Default)]
pub(super) struct Trace {
    pub running: Option<String>,
    pub results: Vec<StepReport>,
    pub final_status: Option<RunStatus>,
}

pub(super) fn parse_events(bytes: &[u8], request: &Request, complete: bool) -> Result<Trace> {
    ensure!(
        bytes.len() as u64 <= EVENT_LIMIT,
        "Maintenance events exceed their limit"
    );
    let (complete_lines, trailing) = match bytes.iter().rposition(|byte| *byte == b'\n') {
        Some(index) => bytes.split_at(index + 1),
        None => (&[][..], bytes),
    };
    ensure!(
        !complete || trailing.is_empty(),
        "Maintenance events end with an incomplete line"
    );
    let mut trace = Trace::default();
    for (index, line) in complete_lines
        .split_inclusive(|byte| *byte == b'\n')
        .enumerate()
    {
        let event: Event = serde_json::from_slice(line).context("Malformed maintenance event")?;
        ensure!(
            event.schema_version == 1
                && event.run_id == request.run_id
                && event.seq == index + 1
                && trace.final_status.is_none(),
            "Invalid maintenance event sequence or identity"
        );
        let last = trace.results.last().map(|result| result.status);
        match event.kind.as_str() {
            "started" => {
                ensure!(
                    trace.running.is_none()
                        && matches!(last, None | Some(StepStatus::Success))
                        && event.status.is_none()
                        && event.step.is_some()
                        && event.step.as_ref() == request.steps.get(trace.results.len()),
                    "Maintenance step started outside the reviewed order"
                );
                trace.running = event.step;
            }
            "result" => {
                let id = trace
                    .running
                    .take()
                    .context("Maintenance result has no started step")?;
                ensure!(
                    event.step.as_ref() == Some(&id),
                    "Maintenance result belongs to the wrong step"
                );
                let status = match event.status.as_deref() {
                    Some("success") => StepStatus::Success,
                    Some("failed") => StepStatus::Failed,
                    Some("skipped") => StepStatus::Skipped,
                    Some("timeout") => StepStatus::TimedOut,
                    _ => anyhow::bail!("Unknown maintenance step status"),
                };
                trace.results.push(StepReport { id, status });
            }
            "finished" => {
                ensure!(
                    event.step.is_none() && trace.running.is_none(),
                    "Maintenance finished while a step was still active"
                );
                trace.final_status = Some(match event.status.as_deref() {
                    Some("success")
                        if trace.results.len() == request.steps.len()
                            && trace
                                .results
                                .iter()
                                .all(|result| result.status == StepStatus::Success) =>
                    {
                        RunStatus::Verified
                    }
                    Some("failed") if last != Some(StepStatus::TimedOut) => RunStatus::Failed,
                    Some("cancelled") if matches!(last, None | Some(StepStatus::Success)) => {
                        RunStatus::Cancelled
                    }
                    Some("timeout") if last == Some(StepStatus::TimedOut) => RunStatus::TimedOut,
                    _ => anyhow::bail!("Maintenance final status contradicts the step results"),
                });
            }
            _ => anyhow::bail!("Unknown maintenance event kind"),
        }
    }
    Ok(trace)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> Request {
        Request {
            schema_version: 1,
            run_id: "test-123".into(),
            steps: IDS[..2].iter().map(|s| (*s).into()).collect(),
            llvm_plan_digest: None,
            review_digest: "a".repeat(64),
        }
    }

    #[test]
    fn llvm_approval_binds_exactly_one_manual_channel() {
        let mut request = request();
        request.steps = vec!["llvm_head".into()];
        assert!(validate_request(&request, "test-123").is_err());
        request.llvm_plan_digest = Some("b".repeat(64));
        assert!(validate_request(&request, "test-123").is_ok());
        request.steps.push("llvm_stable".into());
        assert!(validate_request(&request, "test-123").is_err());
        request.steps = vec!["helm".into()];
        assert!(validate_request(&request, "test-123").is_err());
    }

    fn event(seq: usize, kind: &str, step: Option<&str>, status: Option<&str>) -> String {
        format!(
            "{}\n",
            serde_json::json!({
                "schema_version": 1, "run_id": "test-123", "seq": seq,
                "kind": kind, "step": step, "status": status,
            })
        )
    }

    #[test]
    fn only_complete_ordered_success_verifies() {
        let request = request();
        let mut events = event(1, "started", Some(IDS[0]), None);
        events += &event(2, "result", Some(IDS[0]), Some("success"));
        let premature = events.clone() + &event(3, "finished", None, Some("success"));
        assert!(parse_events(premature.as_bytes(), &request, true).is_err());
        events += &event(3, "started", Some(IDS[1]), None);
        events += &event(4, "result", Some(IDS[1]), Some("success"));
        events += &event(5, "finished", None, Some("success"));
        let trace = parse_events(events.as_bytes(), &request, true).unwrap();
        assert_eq!(trace.final_status, Some(RunStatus::Verified));
        assert_eq!(trace.results.len(), 2);
        events += &event(6, "finished", None, Some("failed"));
        assert!(parse_events(events.as_bytes(), &request, true).is_err());
    }

    #[test]
    fn cancellation_and_partial_or_foreign_events_are_distinct() {
        let request = request();
        validate_request(&request, "test-123").unwrap();
        assert!(validate_request(&request, "../test-123").is_err());
        let cancelled = event(1, "finished", None, Some("cancelled"));
        assert_eq!(
            parse_events(cancelled.as_bytes(), &request, true)
                .unwrap()
                .final_status,
            Some(RunStatus::Cancelled)
        );
        let partial = event(1, "started", Some(IDS[0]), None) + "{";
        assert_eq!(
            parse_events(partial.as_bytes(), &request, false)
                .unwrap()
                .running
                .as_deref(),
            Some(IDS[0])
        );
        assert!(parse_events(partial.as_bytes(), &request, true).is_err());
        assert!(
            parse_events(
                cancelled.replace("test-123", "other-run").as_bytes(),
                &request,
                true
            )
            .is_err()
        );
        assert!(
            parse_events(
                event(1, "started", Some(IDS[1]), None).as_bytes(),
                &request,
                true
            )
            .is_err()
        );
    }
}
