use std::collections::HashMap;

#[tauri::command]
pub fn find_claude_executable() -> Result<Option<String>, String> {
    // Try PATH first
    if let Ok(path) = which::which("claude") {
        return Ok(Some(path.to_string_lossy().to_string()));
    }

    // Hardcoded fallback paths
    let candidates = vec![
        expand_env(r"%LOCALAPPDATA%\Programs\claude\claude.exe"),
        expand_env(r"%LOCALAPPDATA%\claude\claude.exe"),
        expand_env(r"%ProgramFiles%\claude\claude.exe"),
        expand_env(r"%ProgramFiles(x86)%\claude\claude.exe"),
        expand_home(r"~\AppData\Local\Programs\claude\claude.exe"),
        expand_home(r"~\AppData\Roaming\npm\claude.cmd"),
        expand_home(r"~\AppData\Roaming\npm\claude"),
    ];

    for path in candidates {
        if std::path::Path::new(&path).is_file() {
            return Ok(Some(path));
        }
    }

    Ok(None)
}

fn expand_env(template: &str) -> String {
    let mut result = template.to_string();
    for (key, val) in std::env::vars() {
        result = result.replace(&format!("%{}%", key), &val);
    }
    result
}

fn expand_home(template: &str) -> String {
    let home = std::env::var("USERPROFILE").unwrap_or_default();
    template.replacen('~', &home, 1)
}

#[tauri::command]
pub fn launch_claude(
    exe: String,
    env_vars: HashMap<String, String>,
    args: Vec<String>,
    cwd: Option<String>,
) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NEW_CONSOLE: u32 = 0x00000010;

    let mut cmd = std::process::Command::new(&exe);
    cmd.args(&args);

    // Build env: start from current env, overlay with provided vars
    let mut env: HashMap<String, String> = std::env::vars().collect();
    for (k, v) in &env_vars {
        env.insert(k.clone(), v.clone());
    }
    cmd.envs(&env);

    if let Some(dir) = &cwd {
        if !dir.is_empty() {
            cmd.current_dir(dir);
        }
    }

    cmd.creation_flags(CREATE_NEW_CONSOLE);

    cmd.spawn()
        .map_err(|e| format!("Failed to launch claude: {}", e))?;

    Ok(())
}
