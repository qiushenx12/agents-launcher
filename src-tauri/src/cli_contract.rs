use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

const CLI_CONTRACT_JSON: &str = include_str!("../../contracts/cli-contract.json");

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CliKind {
    #[default]
    Claude,
    Codex,
    Opencode,
    Dsh,
}

impl CliKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Opencode => "opencode",
            Self::Dsh => "dsh",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CliConfigFormat {
    Json,
    Toml,
    Jsonc,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CliCommand {
    Claude,
    Codex,
    Opencode,
    Dsh,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CliIssueCode {
    ExecutableMissing,
    VersionCommandFailed,
    VersionTooOld,
    ConfigParseFailed,
    AuthenticationMissing,
    ProviderUnreachable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CliStatusState {
    Checking,
    Ready,
    Blocked,
    Degraded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MainTab {
    Config,
    Claude,
    Codex,
    Opencode,
    Dsh,
    Terminal,
    Orchestration,
}

impl MainTab {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Config => "config",
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Opencode => "opencode",
            Self::Dsh => "dsh",
            Self::Terminal => "terminal",
            Self::Orchestration => "orchestration",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CliDescriptor {
    pub kind: CliKind,
    pub label: String,
    pub command: CliCommand,
    pub config_format: CliConfigFormat,
    pub supports_managed_profile: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MainTabContract {
    pub values: Vec<MainTab>,
    pub legacy_aliases: BTreeMap<String, MainTab>,
    pub unknown_fallback: MainTab,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CliIssueDefinition {
    pub code: CliIssueCode,
    pub state: CliStatusState,
    pub messages: BTreeMap<CliKind, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CliContract {
    pub contract_version: u32,
    pub serialized_cli_kind_field: String,
    pub legacy_default_cli_kind: CliKind,
    pub main_tab: MainTabContract,
    pub cli_descriptors: Vec<CliDescriptor>,
    pub issue_definitions: Vec<CliIssueDefinition>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CliStatus {
    pub kind: CliKind,
    pub state: CliStatusState,
    pub issue_code: Option<CliIssueCode>,
    pub message: String,
    pub executable_path: Option<String>,
    pub version: Option<String>,
}

pub fn load_cli_contract() -> Result<CliContract, String> {
    serde_json::from_str(CLI_CONTRACT_JSON).map_err(|error| format!("内置 CLI 契约无效: {error}"))
}

pub fn status_for_issue(kind: CliKind, code: CliIssueCode) -> Result<CliStatus, String> {
    let contract = load_cli_contract()?;
    let definition = contract
        .issue_definitions
        .iter()
        .find(|definition| definition.code == code)
        .ok_or_else(|| format!("CLI 契约缺少错误码: {code:?}"))?;
    let message = definition
        .messages
        .get(&kind)
        .cloned()
        .ok_or_else(|| format!("CLI 契约缺少 {kind:?} 的错误提示"))?;

    Ok(CliStatus {
        kind,
        state: definition.state,
        issue_code: Some(code),
        message,
        executable_path: None,
        version: None,
    })
}

#[tauri::command]
pub fn get_cli_contract() -> Result<CliContract, String> {
    load_cli_contract()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contract_contains_exactly_four_stable_cli_kinds() {
        let contract = load_cli_contract().expect("contract should parse");
        assert_eq!(contract.contract_version, 1);
        assert_eq!(contract.serialized_cli_kind_field, "cliKind");
        assert_eq!(contract.legacy_default_cli_kind, CliKind::Claude);
        assert_eq!(contract.cli_descriptors.len(), 4);
        assert_eq!(contract.cli_descriptors[0].label, "Claude Code");
        assert_eq!(contract.cli_descriptors[0].command, CliCommand::Claude);
        assert_eq!(contract.cli_descriptors[1].label, "CodeX");
        assert_eq!(contract.cli_descriptors[1].command, CliCommand::Codex);
        assert_eq!(contract.cli_descriptors[2].label, "OpenCode");
        assert_eq!(contract.cli_descriptors[2].command, CliCommand::Opencode);
        assert_eq!(contract.cli_descriptors[3].label, "DeepSeek Harness");
        assert_eq!(contract.cli_descriptors[3].command, CliCommand::Dsh);
        assert_eq!(contract.cli_descriptors[3].config_format, CliConfigFormat::None);
        assert!(!contract.cli_descriptors[3].supports_managed_profile);
    }

    #[test]
    fn every_issue_has_a_message_for_every_cli() {
        let contract = load_cli_contract().expect("contract should parse");
        for definition in contract.issue_definitions {
            assert!(matches!(
                definition.state,
                CliStatusState::Blocked | CliStatusState::Degraded
            ));
            for kind in [
                CliKind::Claude,
                CliKind::Codex,
                CliKind::Opencode,
                CliKind::Dsh,
            ] {
                assert!(definition.messages.contains_key(&kind));
            }
        }
    }

    #[test]
    fn status_serialization_uses_frontend_field_names() {
        let status = status_for_issue(CliKind::Codex, CliIssueCode::VersionCommandFailed)
            .expect("status should build");
        let value = serde_json::to_value(status).expect("status should serialize");
        assert_eq!(value["kind"], "codex");
        assert_eq!(value["state"], "blocked");
        assert_eq!(value["issueCode"], "version_command_failed");
        assert!(value.get("executablePath").is_some());
    }
}
