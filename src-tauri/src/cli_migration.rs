use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::cli_contract::{CliKind, MainTab};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MigrationReport {
    pub changed: bool,
    pub cli_kind_fields_added: usize,
    pub legacy_tab_mapped: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LegacyProfileAssignment {
    pub profile_name: String,
    pub cli_kind: CliKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoverySource {
    Original,
    Backup,
    None,
}

pub fn normalize_main_tab(value: &str) -> MainTab {
    match value {
        "project" | "claude" => MainTab::Claude,
        "codex" => MainTab::Codex,
        "opencode" => MainTab::Opencode,
        "dsh" => MainTab::Dsh,
        "terminal" => MainTab::Terminal,
        "orchestration" => MainTab::Orchestration,
        _ => MainTab::Config,
    }
}

pub fn migrate_app_state_value(mut value: Value) -> Result<(Value, MigrationReport), String> {
    let root = value
        .as_object_mut()
        .ok_or_else(|| "app_state.json 根节点不是对象".to_string())?;
    let mut report = MigrationReport::default();

    if root.get("last_active_main_tab").and_then(Value::as_str) == Some("project") {
        root.insert(
            "last_active_main_tab".to_string(),
            Value::String("claude".to_string()),
        );
        report.changed = true;
        report.legacy_tab_mapped = true;
    }

    Ok((value, report))
}

pub fn migrate_project_store_value(mut value: Value) -> Result<(Value, MigrationReport), String> {
    let root = value
        .as_object_mut()
        .ok_or_else(|| "projects.json 根节点不是对象".to_string())?;
    let mut report = MigrationReport::default();

    for collection_name in ["projects", "sessions"] {
        let Some(collection) = root.get_mut(collection_name) else {
            continue;
        };
        let entries = collection
            .as_array_mut()
            .ok_or_else(|| format!("projects.json 的 {collection_name} 不是数组"))?;
        for entry in entries {
            let object = entry
                .as_object_mut()
                .ok_or_else(|| format!("projects.json 的 {collection_name} 含有非对象条目"))?;
            if !object.contains_key("cliKind") {
                object.insert("cliKind".to_string(), Value::String("claude".to_string()));
                report.changed = true;
                report.cli_kind_fields_added += 1;
            }
        }
    }

    Ok((value, report))
}

pub fn classify_legacy_profiles(value: &Value) -> Result<Vec<LegacyProfileAssignment>, String> {
    let profiles = value
        .as_object()
        .ok_or_else(|| "env_configs.json 根节点不是对象".to_string())?;

    profiles
        .iter()
        .map(|(profile_name, profile)| {
            if !profile.is_object() {
                return Err(format!("旧配置方案 {profile_name} 不是对象"));
            }
            Ok(LegacyProfileAssignment {
                profile_name: profile_name.clone(),
                cli_kind: CliKind::Claude,
            })
        })
        .collect()
}

pub fn choose_recovery_source(original_exists: bool, backup_exists: bool) -> RecoverySource {
    if original_exists {
        RecoverySource::Original
    } else if backup_exists {
        RecoverySource::Backup
    } else {
        RecoverySource::None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(path: &str) -> Value {
        serde_json::from_str(path).expect("fixture should contain valid JSON")
    }

    #[test]
    fn legacy_project_tab_maps_to_claude_without_dropping_unknown_fields() {
        let input = fixture(include_str!(
            "../tests/fixtures/migration/legacy-app-state-project-tab.json"
        ));
        let (migrated, report) = migrate_app_state_value(input).expect("migration should succeed");
        assert_eq!(migrated["last_active_main_tab"], "claude");
        assert_eq!(migrated["future_field"]["keep"], true);
        assert!(report.legacy_tab_mapped);
    }

    #[test]
    fn missing_cli_kinds_default_to_claude_and_migration_is_idempotent() {
        let input = fixture(include_str!(
            "../tests/fixtures/migration/legacy-projects-no-cli-kind.json"
        ));
        let expected = fixture(include_str!(
            "../tests/fixtures/migration/migrated-projects-default-claude.json"
        ));
        let (migrated, report) =
            migrate_project_store_value(input).expect("migration should succeed");
        assert_eq!(migrated, expected);
        assert_eq!(report.cli_kind_fields_added, 2);

        let (second_pass, second_report) =
            migrate_project_store_value(migrated.clone()).expect("second pass should succeed");
        assert_eq!(second_pass, migrated);
        assert!(!second_report.changed);
    }

    #[test]
    fn legacy_profiles_are_classified_as_claude_without_reading_secret_values() {
        let input = fixture(include_str!(
            "../tests/fixtures/migration/legacy-env-configs-unknown-fields.json"
        ));
        let assignments = classify_legacy_profiles(&input).expect("classification should succeed");
        assert_eq!(assignments.len(), 1);
        assert_eq!(assignments[0].cli_kind, CliKind::Claude);
        assert_eq!(input["legacy-profile"]["future_option"], "preserve-me");
    }

    #[test]
    fn interrupted_write_never_promotes_an_uncommitted_temporary_file() {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct InterruptedWriteFixture {
            original_exists: bool,
            temporary_exists: bool,
            backup_exists: bool,
            expected_recovery_source: RecoverySource,
        }

        let value = include_str!("../tests/fixtures/migration/interrupted-write.json");
        let fixture: InterruptedWriteFixture =
            serde_json::from_str(value).expect("fixture should parse");
        assert!(fixture.temporary_exists);
        assert_eq!(
            choose_recovery_source(fixture.original_exists, fixture.backup_exists),
            fixture.expected_recovery_source
        );
    }

    #[test]
    fn main_tab_normalization_has_a_safe_unknown_fallback() {
        assert_eq!(normalize_main_tab("project"), MainTab::Claude);
        assert_eq!(normalize_main_tab("codex"), MainTab::Codex);
        assert_eq!(normalize_main_tab("future-tab"), MainTab::Config);
    }
}
