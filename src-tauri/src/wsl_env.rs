//! wsl_env.rs
//!
//! Applies the Claude Code profile to the default WSL distribution (Windows only):
//!   - environment variables → a managed marker block in the distro's `~/.bashrc`
//!   - `skipDangerousModePermissionPrompt` / `permissions.defaultMode` /
//!     `awaySummaryEnabled` → the distro's `~/.claude/settings.json`
//!
//! All invocations use `wsl.exe -e bash -c <script>`: `-e` execs directly and
//! bypasses the WSL default shell re-parsing the argument line (the scripts
//! contain `;`, `||`, `>` …). File contents travel through stdin, so values
//! never pass through shell quoting. Every write is preceded by a one-shot
//! `cp` backup (`.cl-launcher.bak`) inside the distro.

use std::collections::HashMap;
use std::time::Duration;

use serde::Serialize;
use serde_json::{Map, Value};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

const WSL_TIMEOUT: Duration = Duration::from_secs(60);

const BLOCK_START: &str = "# >>> Agents Launcher: Claude Code env >>>";
const BLOCK_END: &str = "# <<< Agents Launcher: Claude Code env <<<";

// Scripts run as a single argv element via `wsl.exe -e bash -c <script>`.
const READ_BASHRC_SCRIPT: &str =
    r#"if [ -f "$HOME/.bashrc" ]; then cp "$HOME/.bashrc" "$HOME/.bashrc.cl-launcher.bak"; cat "$HOME/.bashrc"; fi"#;
const WRITE_BASHRC_SCRIPT: &str = r#"cat > "$HOME/.bashrc""#;
const READ_SETTINGS_SCRIPT: &str =
    r#"if [ -f "$HOME/.claude/settings.json" ]; then cp "$HOME/.claude/settings.json" "$HOME/.claude/settings.json.cl-launcher.bak"; cat "$HOME/.claude/settings.json"; fi"#;
const WRITE_SETTINGS_SCRIPT: &str =
    r#"mkdir -p "$HOME/.claude" && cat > "$HOME/.claude/settings.json""#;
const CHECK_SCRIPT: &str = r#"if command -v claude >/dev/null 2>&1 || grep -qs "alias claude=" "$HOME/.bashrc"; then echo WSL_CLAUDE_OK; else echo WSL_CLAUDE_MISSING; fi"#;

// ---------------------------------------------------------------------------
// wsl.exe helper
// ---------------------------------------------------------------------------

/// Runs `wsl.exe -e bash -c <script>` (optionally feeding stdin) and returns
/// (exit_success, stdout, stderr). Spawning a missing wsl.exe is an error.
async fn run_wsl_bash(script: &str, stdin: Option<&str>) -> std::io::Result<(bool, String, String)> {
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;

        let mut command = Command::new("wsl.exe");
        command.args(["-e", "bash", "-c", script]);
        command.creation_flags(CREATE_NO_WINDOW);
        command.stdin(std::process::Stdio::piped());
        command.stdout(std::process::Stdio::piped());
        command.stderr(std::process::Stdio::piped());
        command.kill_on_drop(true);

        let mut child = command
            .spawn()
            .map_err(|e| std::io::Error::new(e.kind(), format!("无法启动 wsl.exe: {e}")))?;

        if let Some(content) = stdin {
            if let Some(mut handle) = child.stdin.take() {
                handle.write_all(content.as_bytes()).await?;
            }
        }
        // Drop the pipe so the child sees EOF.
        drop(child.stdin.take());

        let output = tokio::time::timeout(WSL_TIMEOUT, child.wait_with_output())
            .await
            .map_err(|_| {
                std::io::Error::new(std::io::ErrorKind::TimedOut, "wsl.exe timed out")
            })?
            .map_err(|e| std::io::Error::new(e.kind(), format!("wsl.exe 执行失败: {e}")))?;
        Ok((
            output.status.success(),
            String::from_utf8_lossy(&output.stdout).into_owned(),
            String::from_utf8_lossy(&output.stderr).into_owned(),
        ))
    }
    #[cfg(not(windows))]
    {
        let _ = (script, stdin);
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "WSL 仅支持 Windows",
        ))
    }
}

fn wsl_error(step: &str, e: std::io::Error) -> String {
    match e.kind() {
        std::io::ErrorKind::TimedOut => {
            format!("WSL 响应超时（{step}），WSL 可能正在启动，请稍后重试")
        }
        _ => format!(
            "WSL 调用失败（{step}）：{e}。请确认已安装 WSL 并设置了默认发行版（wsl --set-default）"
        ),
    }
}

fn wsl_error_text(step: &str, stderr: &str) -> String {
    let message = stderr.trim().replace(['\r', '\n'], " ");
    if message.is_empty() {
        format!("WSL 调用失败（{step}）。请确认已安装 WSL 并设置了默认发行版（wsl --set-default）")
    } else {
        format!("WSL 调用失败（{step}）：{}", message.chars().take(500).collect::<String>())
    }
}

// ---------------------------------------------------------------------------
// Pure helpers (unit tested)
// ---------------------------------------------------------------------------

/// Single-quotes a value for bash (an embedded `'` becomes `''`).
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

/// Renders the managed bashrc block: keys sorted; non-empty → `export`, empty → `unset`.
pub fn render_block(vars: &HashMap<String, String>) -> String {
    let mut keys: Vec<&String> = vars.keys().collect();
    keys.sort();
    let mut lines = vec![BLOCK_START.to_string()];
    for key in keys {
        let value = &vars[key];
        if value.is_empty() {
            lines.push(format!("unset {key}"));
        } else {
            lines.push(format!("export {key}={}", shell_quote(value)));
        }
    }
    lines.push(BLOCK_END.to_string());
    lines.join("\n")
}

/// Splices the managed block into the existing .bashrc content:
/// replace in place when both markers exist, otherwise append at the end.
pub fn splice_block(existing: &str, block: &str) -> String {
    match (existing.find(BLOCK_START), existing.find(BLOCK_END)) {
        (Some(start), Some(end)) if end > start => {
            let after_end = end + BLOCK_END.len();
            // Consume the newline terminating the end-marker line (if any) so
            // the replacement doesn't duplicate it (keeps splice idempotent).
            let remainder = if existing[after_end..].starts_with('\n') {
                &existing[after_end + 1..]
            } else {
                &existing[after_end..]
            };
            let mut out = String::with_capacity(existing.len() + block.len());
            out.push_str(&existing[..start]);
            out.push_str(block);
            out.push('\n');
            out.push_str(remainder);
            out
        }
        _ => {
            let mut out = existing.to_string();
            if !out.is_empty() && !out.ends_with('\n') {
                out.push('\n');
            }
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(block);
            out.push('\n');
            out
        }
    }
}

// ---------------------------------------------------------------------------
// Tauri commands
// ---------------------------------------------------------------------------

/// WSL + Claude Code availability for the "应用到 WSL" button state.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WslCheckResult {
    pub wsl_available: bool,
    pub claude_found: bool,
}

/// Detects whether the default WSL distro is usable and whether `claude` is
/// reachable in it (PATH install incl. the inherited Windows shim, or a
/// `claude` alias in ~/.bashrc). "WSL missing" is a normal result, not an error.
#[tauri::command]
pub async fn check_wsl_claude() -> Result<WslCheckResult, String> {
    #[cfg(windows)]
    {
        // Any failure (missing wsl.exe, no default distro, non-zero exit)
        // simply means "WSL unavailable" — a normal result, not an error.
        match run_wsl_bash(CHECK_SCRIPT, None).await {
            Ok((true, stdout, _stderr)) => Ok(WslCheckResult {
                wsl_available: true,
                claude_found: stdout.contains("WSL_CLAUDE_OK"),
            }),
            _ => Ok(WslCheckResult {
                wsl_available: false,
                claude_found: false,
            }),
        }
    }
    #[cfg(not(windows))]
    {
        Ok(WslCheckResult {
            wsl_available: false,
            claude_found: false,
        })
    }
}

/// Applies the profile env vars to the default WSL distro's `~/.bashrc`
/// (managed marker block) and the managed settings fields to its
/// `~/.claude/settings.json` (all other fields preserved).
#[tauri::command]
pub async fn apply_wsl_env_vars(
    vars: HashMap<String, String>,
    skip_permissions: bool,
    away_summary_disabled: bool,
) -> Result<String, String> {
    #[cfg(windows)]
    {
        apply_wsl_env_vars_impl(vars, skip_permissions, away_summary_disabled).await
    }
    #[cfg(not(windows))]
    {
        let _ = (vars, skip_permissions, away_summary_disabled);
        Err("仅 Windows 支持应用到 WSL".to_string())
    }
}

#[cfg(windows)]
async fn apply_wsl_env_vars_impl(
    vars: HashMap<String, String>,
    skip_permissions: bool,
    away_summary_disabled: bool,
) -> Result<String, String> {
    // 1. Backup + read ~/.bashrc.
    let (ok, existing_bashrc, stderr) =
        run_wsl_bash(READ_BASHRC_SCRIPT, None)
            .await
            .map_err(|e| wsl_error("读取 ~/.bashrc", e))?;
    if !ok {
        return Err(wsl_error_text("读取 ~/.bashrc", &stderr));
    }

    // 2. Write the updated ~/.bashrc (managed block, original content kept).
    let new_bashrc = splice_block(&existing_bashrc, &render_block(&vars));
    let (ok, _stdout, stderr) =
        run_wsl_bash(WRITE_BASHRC_SCRIPT, Some(&new_bashrc))
            .await
            .map_err(|e| wsl_error("写入 ~/.bashrc", e))?;
    if !ok {
        return Err(wsl_error_text("写入 ~/.bashrc", &stderr));
    }

    // 3. Backup + read ~/.claude/settings.json (missing → start from an empty object).
    let (ok, raw_settings, stderr) =
        run_wsl_bash(READ_SETTINGS_SCRIPT, None)
            .await
            .map_err(|e| wsl_error("读取 ~/.claude/settings.json", e))?;
    if !ok {
        return Err(wsl_error_text("读取 ~/.claude/settings.json", &stderr));
    }
    let mut settings: Map<String, Value> = if raw_settings.trim().is_empty() {
        Map::new()
    } else {
        // Malformed JSON is a hard failure: never overwrite it with a partial object.
        let value: Value = serde_json::from_str(&raw_settings)
            .map_err(|e| format!("WSL 内 ~/.claude/settings.json 无法解析，已跳过写入：{e}"))?;
        value
            .as_object()
            .cloned()
            .ok_or_else(|| "WSL 内 ~/.claude/settings.json 根节点必须是对象，已跳过写入".to_string())?
    };
    crate::settings_manager::apply_managed_fields(
        &mut settings,
        skip_permissions,
        away_summary_disabled,
    )?;

    // 4. Write the updated ~/.claude/settings.json.
    let new_settings = serde_json::to_string_pretty(&Value::Object(settings))
        .map_err(|e| format!("序列化 settings.json 失败：{e}"))?;
    let (ok, _stdout, stderr) =
        run_wsl_bash(WRITE_SETTINGS_SCRIPT, Some(&new_settings))
            .await
            .map_err(|e| wsl_error("写入 ~/.claude/settings.json", e))?;
    if !ok {
        return Err(wsl_error_text("写入 ~/.claude/settings.json", &stderr));
    }

    Ok("已应用到 WSL（默认发行版）".to_string())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn render_block_exports_sorted_and_unsets_empty() {
        let block = render_block(&vars(&[
            ("ANTHROPIC_MODEL", "qwen3"),
            ("ANTHROPIC_BASE_URL", ""),
            ("DISABLE_AUTOUPDATER", "1"),
        ]));
        assert_eq!(
            block,
            "# >>> Agents Launcher: Claude Code env >>>\n\
             unset ANTHROPIC_BASE_URL\n\
             export ANTHROPIC_MODEL='qwen3'\n\
             export DISABLE_AUTOUPDATER='1'\n\
             # <<< Agents Launcher: Claude Code env <<<"
        );
    }

    #[test]
    fn render_block_escapes_single_quotes_and_keeps_dollar_literal() {
        let block = render_block(&vars(&[("ANTHROPIC_AUTH_TOKEN", "it's a $ecret")]));
        assert!(block.contains("export ANTHROPIC_AUTH_TOKEN='it''s a $ecret'"));
    }

    #[test]
    fn splice_appends_preserving_existing_content() {
        let block = render_block(&vars(&[("A", "1")]));
        let out = splice_block("line1\nline2\n", &block);
        assert_eq!(out, format!("line1\nline2\n\n{block}\n"));
        // No trailing newline on the last line: terminate it first.
        let out = splice_block("alias ll='ls -l'", &block);
        assert!(out.starts_with("alias ll='ls -l'\n\n"));
    }

    #[test]
    fn splice_replaces_existing_block_in_place() {
        let old_block = render_block(&vars(&[("A", "old")]));
        let existing = format!("before\n\n{old_block}\n\nafter\n");
        let new_block = render_block(&vars(&[("A", "new"), ("B", "")]));
        let out = splice_block(&existing, &new_block);
        assert_eq!(out, format!("before\n\n{new_block}\n\nafter\n"));
    }

    #[test]
    fn splice_is_idempotent() {
        let block = render_block(&vars(&[("A", "1"), ("B", "")]));
        let once = splice_block("keep\n", &block);
        let twice = splice_block(&once, &block);
        assert_eq!(once, twice);
    }

    #[test]
    fn splice_ignores_end_marker_without_start() {
        let existing = format!("odd\n{BLOCK_END}\ntail\n");
        let block = render_block(&vars(&[("A", "1")]));
        let out = splice_block(&existing, &block);
        assert_eq!(out, format!("odd\n{BLOCK_END}\ntail\n\n{block}\n"));
    }
}
