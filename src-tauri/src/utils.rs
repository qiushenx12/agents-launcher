use std::collections::HashMap;
use std::fs;
use std::path::Path;

use serde::Serialize;

#[cfg(any(windows, target_os = "macos"))]
use std::process::Command;

#[tauri::command]
pub fn get_current_env_vars(var_names: Vec<String>) -> HashMap<String, String> {
    let mut result = HashMap::new();
    for name in var_names {
        if let Ok(val) = std::env::var(&name) {
            result.insert(name, val);
        }
    }
    result
}

#[tauri::command]
pub fn get_claude_config_dir() -> Result<String, String> {
    let dir = dirs::data_dir()
        .ok_or("Cannot determine application data directory")?
        .join("ClaudeEnvManager");
    Ok(dir.to_string_lossy().to_string())
}

#[tauri::command]
pub fn get_home_dir() -> Result<String, String> {
    dirs::home_dir()
        .map(|path| path.to_string_lossy().to_string())
        .ok_or_else(|| "Cannot determine user home directory".to_string())
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeSkillEntry {
    pub name: String,
    pub description: String,
}

#[tauri::command]
pub fn list_claude_skills(project_path: Option<String>) -> Vec<ClaudeSkillEntry> {
    let mut skills = HashMap::<String, ClaudeSkillEntry>::new();

    if let Some(home) = dirs::home_dir() {
        collect_claude_skills(&home.join(".claude").join("skills"), &mut skills);
    }
    if let Some(project_path) = project_path.filter(|path| !path.trim().is_empty()) {
        collect_claude_skills(
            &Path::new(&project_path).join(".claude").join("skills"),
            &mut skills,
        );
    }

    let mut skills: Vec<_> = skills.into_values().collect();
    skills.sort_by(|left, right| left.name.cmp(&right.name));
    skills
}

fn collect_claude_skills(root: &Path, skills: &mut HashMap<String, ClaudeSkillEntry>) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let fallback_name = entry.file_name().to_string_lossy().trim().to_string();
        let Some(skill) = parse_claude_skill(&path.join("SKILL.md"), &fallback_name) else {
            continue;
        };
        skills.insert(skill.name.to_ascii_lowercase(), skill);
    }
}

fn parse_claude_skill(path: &Path, fallback_name: &str) -> Option<ClaudeSkillEntry> {
    let contents = fs::read_to_string(path).ok()?;
    let mut lines = contents.lines();
    if lines.next()?.trim() != "---" {
        return None;
    }

    let mut name = None;
    let mut description = None;
    for line in lines {
        let line = line.trim();
        if line == "---" {
            break;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim().trim_matches(['\'', '"']).trim();
        match key.trim() {
            "name" if !value.is_empty() => name = Some(value.to_string()),
            "description" if !value.is_empty() => description = Some(value.to_string()),
            _ => {}
        }
    }

    let name = name.unwrap_or_else(|| fallback_name.to_string());
    if !is_claude_skill_name(&name) {
        return None;
    }
    Some(ClaudeSkillEntry {
        name,
        description: description.unwrap_or_default(),
    })
}

fn is_claude_skill_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(first) if first.is_ascii_lowercase() || first.is_ascii_digit())
        && chars.all(|character| character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-')
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn skill_parser_reads_frontmatter_and_rejects_invalid_names() {
        let directory = tempdir().expect("temporary directory");
        let skill_path = directory.path().join("SKILL.md");
        fs::write(
            &skill_path,
            "---\nname: project-helper\ndescription: Project-specific helper\n---\n# Skill\n",
        )
        .expect("skill file");

        let skill = parse_claude_skill(&skill_path, "fallback").expect("valid skill");
        assert_eq!(skill.name, "project-helper");
        assert_eq!(skill.description, "Project-specific helper");

        fs::write(&skill_path, "---\nname: Invalid Name\n---\n").expect("invalid skill file");
        assert!(parse_claude_skill(&skill_path, "fallback").is_none());
    }

    #[test]
    fn project_skills_override_global_skills_with_the_same_name() {
        let directory = tempdir().expect("temporary directory");
        let global_root = directory.path().join("global");
        let project_root = directory.path().join("project");
        fs::create_dir_all(global_root.join("review")).expect("global skill directory");
        fs::create_dir_all(project_root.join("review")).expect("project skill directory");
        fs::write(
            global_root.join("review").join("SKILL.md"),
            "---\nname: review\ndescription: Global review\n---\n",
        )
        .expect("global skill");
        fs::write(
            project_root.join("review").join("SKILL.md"),
            "---\nname: review\ndescription: Project review\n---\n",
        )
        .expect("project skill");

        let mut skills = HashMap::new();
        collect_claude_skills(&global_root, &mut skills);
        collect_claude_skills(&project_root, &mut skills);

        assert_eq!(skills["review"].description, "Project review");
    }
}

#[tauri::command]
pub fn open_directory(path: String) -> Result<(), String> {
    #[cfg(windows)]
    {
        Command::new("explorer")
            .arg(&path)
            .spawn()
            .map_err(|e| format!("Failed to open directory: {}", e))?;
    }
    #[cfg(target_os = "macos")]
    {
        Command::new("open")
            .arg(&path)
            .spawn()
            .map_err(|e| format!("Failed to open directory: {}", e))?;
    }
    Ok(())
}

#[tauri::command]
pub fn open_env_vars_dialog() -> Result<(), String> {
    #[cfg(windows)]
    {
        Command::new("rundll32")
            .args(["sysdm.cpl,EditEnvironmentVariables"])
            .spawn()
            .map_err(|e| format!("Failed to open env vars dialog: {}", e))?;
        Ok(())
    }
    #[cfg(not(windows))]
    {
        Err("此功能仅支持 Windows。macOS 请通过 ~/.claude/settings.json 配置环境变量。".to_string())
    }
}
