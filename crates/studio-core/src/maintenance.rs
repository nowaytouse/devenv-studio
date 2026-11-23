//! Wire contract shared by the Studio supervisor and the Topgrade engine.
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

pub const SCHEMA: u32 = 1;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Step {
    BrewLinkageAudit,
    CliVersions,
    GithubCliExtensions,
    GithubCliSkills,
    Helm,
    LlvmHead,
    LlvmStable,
    Tldr,
}

impl Step {
    pub const ALL: [Self; 8] = [
        Self::BrewLinkageAudit,
        Self::CliVersions,
        Self::GithubCliExtensions,
        Self::GithubCliSkills,
        Self::Helm,
        Self::LlvmHead,
        Self::LlvmStable,
        Self::Tldr,
    ];

    pub fn id(self) -> &'static str {
        match self {
            Self::BrewLinkageAudit => "brew_linkage_audit",
            Self::CliVersions => "cli_versions",
            Self::GithubCliExtensions => "github_cli_extensions",
            Self::GithubCliSkills => "github_cli_skills",
            Self::Helm => "helm",
            Self::LlvmHead => "llvm_head",
            Self::LlvmStable => "llvm_stable",
            Self::Tldr => "tldr",
        }
    }

    pub fn find(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|step| step.id() == id)
    }

    pub fn manual_llvm(self) -> bool {
        matches!(self, Self::LlvmHead | Self::LlvmStable)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::BrewLinkageAudit => "Homebrew Linkage Audit",
            Self::CliVersions => "CLI Tool Versions",
            Self::GithubCliExtensions => "GitHub CLI Extensions",
            Self::GithubCliSkills => "GitHub CLI Skills",
            Self::Helm => "Helm Repositories",
            Self::LlvmHead => "LLVM HEAD",
            Self::LlvmStable => "LLVM Stable",
            Self::Tldr => "tldr Cache",
        }
    }

    pub fn tools(self) -> &'static [&'static str] {
        match self {
            Self::BrewLinkageAudit | Self::LlvmHead | Self::LlvmStable => &["brew"],
            Self::CliVersions => &[],
            Self::GithubCliExtensions | Self::GithubCliSkills => &["gh"],
            Self::Helm => &["helm", "curl"],
            Self::Tldr => &["tldr"],
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub schema_version: u32,
    pub run_id: String,
    pub steps: Vec<Step>,
    pub review_digest: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub llvm_plan_digest: Option<String>,
}

pub fn valid_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

pub fn valid_run_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
}

impl Request {
    pub fn validate(&self, run_id: &str) -> Result<()> {
        ensure!(
            self.schema_version == SCHEMA && self.run_id == run_id && valid_run_id(run_id),
            "Invalid Studio request identity"
        );
        ensure!(
            valid_hex(&self.review_digest, 64),
            "Invalid reviewed receipt digest"
        );
        ensure!(
            (1..=Step::ALL.len()).contains(&self.steps.len())
                && self
                    .steps
                    .windows(2)
                    .all(|pair| pair[0].id() < pair[1].id()),
            "Select distinct supported Studio steps"
        );
        let manual = self.steps.iter().any(|step| step.manual_llvm());
        ensure!(
            match &self.llvm_plan_digest {
                Some(digest) => manual && self.steps.len() == 1 && valid_hex(digest, 64),
                None => !manual,
            },
            "LLVM changes require a single reviewed candidate"
        );
        Ok(())
    }
}
