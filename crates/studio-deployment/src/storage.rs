#[cfg(any(windows, test))]
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationState {
    Enabled,
    Disabled,
    NotSet,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StorageStatus {
    pub preview: bool,
    pub ntfs: NotificationState,
    pub refs: NotificationState,
    pub detail: String,
}

pub(crate) fn is_task(id: &str) -> bool {
    matches!(
        id,
        "storage-analyze-system"
            | "storage-optimize-system"
            | "storage-analyze-all"
            | "storage-optimize-all"
    )
}

#[cfg(any(windows, test))]
pub(crate) fn valid_volume_guid(path: &str) -> bool {
    let Some(guid) = path
        .strip_prefix(r"\\?\Volume{")
        .and_then(|value| value.strip_suffix("}\\"))
    else {
        return false;
    };
    guid.len() == 36
        && guid.bytes().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
}

#[cfg(any(windows, test))]
pub(crate) fn arguments(id: &str, system_volume: Option<&str>) -> Result<Vec<String>> {
    let (system, mode) = match id {
        "storage-analyze-system" => (true, "/A"),
        "storage-optimize-system" => (true, "/O"),
        "storage-analyze-all" => (false, "/A"),
        "storage-optimize-all" => (false, "/O"),
        _ => anyhow::bail!("Unsupported storage maintenance task"),
    };
    let target = if system {
        let volume = system_volume
            .ok_or_else(|| anyhow::anyhow!("Windows system volume identity is unavailable"))?;
        ensure!(
            valid_volume_guid(volume),
            "Windows system volume must be a canonical volume GUID path"
        );
        volume
    } else {
        "/C"
    };
    Ok(vec![target.into(), mode.into(), "/U".into()])
}

#[cfg(any(windows, test))]
fn notification_value(rest: &str) -> Result<NotificationState> {
    let rest = rest.trim();
    if rest.eq_ignore_ascii_case("is not currently set") {
        return Ok(NotificationState::NotSet);
    }
    let Some(value) = rest.strip_prefix('=').map(str::trim) else {
        return Ok(NotificationState::Unknown);
    };
    let Some(number) = value.split_whitespace().next() else {
        anyhow::bail!("Delete-notification row has no numeric value");
    };
    let state = match number {
        "0" => NotificationState::Enabled,
        "1" => NotificationState::Disabled,
        _ => anyhow::bail!("Delete-notification value must be exactly 0 or 1"),
    };
    let suffix = value[number.len()..].trim();
    ensure!(
        suffix.is_empty()
            || (suffix.starts_with('(')
                && suffix.ends_with(')')
                && !suffix[1..suffix.len() - 1].contains(['(', ')', '\0'])),
        "Unexpected trailing delete-notification value"
    );
    Ok(state)
}

#[cfg(any(windows, test))]
pub(crate) fn parse_notifications(text: &str) -> Result<(NotificationState, NotificationState)> {
    let mut ntfs = None;
    let mut refs = None;
    for line in text.lines() {
        let line = line.trim();
        let lower = line.to_ascii_lowercase();
        let (rest, slot) = if let Some(rest) = lower.strip_prefix("ntfs") {
            (rest, &mut ntfs)
        } else if let Some(rest) = lower.strip_prefix("refs") {
            (rest, &mut refs)
        } else {
            continue;
        };
        if !rest.starts_with(char::is_whitespace) {
            continue;
        }
        let Some(rest) = rest.trim_start().strip_prefix("disabledeletenotify") else {
            continue;
        };
        if !rest.is_empty()
            && !rest.starts_with(|character: char| character.is_whitespace() || character == '=')
        {
            continue;
        }
        ensure!(
            slot.is_none(),
            "Duplicate delete-notification filesystem row"
        );
        *slot = Some(notification_value(rest)?);
    }
    Ok((
        ntfs.unwrap_or(NotificationState::Unknown),
        refs.unwrap_or(NotificationState::Unknown),
    ))
}

#[cfg(any(windows, test))]
pub(crate) fn inspect_with_runner(
    command: &crate::process::Invocation,
    runner: &mut impl crate::process::Runner,
    cancel: &std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> Result<StorageStatus> {
    ensure!(
        !cancel.load(std::sync::atomic::Ordering::Acquire),
        "Storage inspection cancelled before command"
    );
    let output = runner.run(command, cancel)?;
    ensure!(
        !output.cancelled
            && !output.timed_out
            && !cancel.load(std::sync::atomic::Ordering::Acquire)
            && output.code == 0,
        "Filesystem notification inspection failed: {}",
        output.diagnostic()
    );
    let (ntfs, refs) = parse_notifications(&output.stdout)?;
    Ok(StorageStatus {
        preview: false,
        ntfs,
        refs,
        detail: format!(
            "Read-only filesystem delete-notification policy; device TRIM support and optimization were not tested.\n{}",
            output.diagnostic()
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::{Invocation, Output, Runner};
    use std::sync::{Arc, atomic::AtomicBool};

    #[test]
    fn storage_arguments_bind_system_identity_and_keep_modes_separate() {
        let volume = r"\\?\Volume{928842df-5a01-11de-a85c-806e6f6e6963}\";
        for (id, expected_target, mode) in [
            ("storage-analyze-system", volume, "/A"),
            ("storage-optimize-system", volume, "/O"),
            ("storage-analyze-all", "/C", "/A"),
            ("storage-optimize-all", "/C", "/O"),
        ] {
            assert!(is_task(id));
            assert_eq!(
                arguments(id, Some(volume)).unwrap(),
                [expected_target, mode, "/U"]
            );
        }
        assert!(arguments("storage-analyze-system", None).is_err());
        for target in ["C:", r"\\server\share", "/C", r"\\?\Volume{bad}\"] {
            assert!(arguments("storage-optimize-system", Some(target)).is_err());
        }
        assert!(arguments("storage-optimize-user", Some(volume)).is_err());
    }

    #[test]
    fn parser_preserves_independent_values_and_unknown_or_unset() {
        assert_eq!(
            parse_notifications(
                "NTFS DisableDeleteNotify = 0 (已禁用)\nReFS DisableDeleteNotify = 1 (Enabled)"
            )
            .unwrap(),
            (NotificationState::Enabled, NotificationState::Disabled)
        );
        assert_eq!(
            parse_notifications(
                "NTFS DisableDeleteNotify = 1\nReFS DisableDeleteNotify is not currently set"
            )
            .unwrap(),
            (NotificationState::Disabled, NotificationState::NotSet)
        );
        assert_eq!(
            parse_notifications("ReFS DisableDeleteNotify 尚未设置\nNTFSDisableDeleteNotify = 0\nNTFS DisableDeleteNotifyOther = 0").unwrap(),
            (NotificationState::Unknown, NotificationState::Unknown)
        );
        for text in [
            "NTFS DisableDeleteNotify = 0\nNTFS DisableDeleteNotify = 1",
            "ReFS DisableDeleteNotify is not currently set\nReFS DisableDeleteNotify = 0",
            "NTFS DisableDeleteNotify = 2",
            "NTFS DisableDeleteNotify = 0 1",
            "NTFS DisableDeleteNotify =",
            "NTFS DisableDeleteNotify = 00",
            "NTFS DisableDeleteNotify = 0 (foo)(garbage)",
        ] {
            assert!(parse_notifications(text).is_err(), "{text}");
        }
    }

    struct Fake(Option<Result<Output>>);
    impl Runner for Fake {
        fn run(&mut self, _: &Invocation, _: &Arc<AtomicBool>) -> Result<Output> {
            self.0.take().unwrap()
        }
    }
    #[test]
    fn inspection_never_turns_command_failure_or_incomplete_capture_into_policy() {
        let command = Invocation {
            program: "/trusted/fsutil.exe".into(),
            args: vec![
                "behavior".into(),
                "query".into(),
                "DisableDeleteNotify".into(),
            ],
            vscode_node: false,
            npm_node: false,
        };
        let success = Output {
            code: 0,
            stdout: "NTFS DisableDeleteNotify = 0\nReFS DisableDeleteNotify = 1".into(),
            stderr: String::new(),
            timed_out: false,
            cancelled: false,
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let status =
            inspect_with_runner(&command, &mut Fake(Some(Ok(success.clone()))), &cancel).unwrap();
        assert!(!status.preview);
        assert_eq!(status.ntfs, NotificationState::Enabled);
        assert!(status.detail.contains("were not tested"));
        for result in [
            Err(anyhow::anyhow!("output exceeded limit")),
            Ok(Output {
                code: 5,
                ..success.clone()
            }),
            Ok(Output {
                timed_out: true,
                ..success.clone()
            }),
            Ok(Output {
                cancelled: true,
                ..success
            }),
        ] {
            assert!(inspect_with_runner(&command, &mut Fake(Some(result)), &cancel).is_err());
        }
        let cancelled = Arc::new(AtomicBool::new(true));
        assert!(inspect_with_runner(&command, &mut Fake(None), &cancelled).is_err());
    }
}
