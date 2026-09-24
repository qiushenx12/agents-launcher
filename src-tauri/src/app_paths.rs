//! Where this app keeps its own data.
//!
//! One source of truth for `<platform data dir>/AgentsLauncher`. Every module
//! used to build that path itself — ten copies across the backend — and that is
//! precisely how the pre-rename folder name outlived the rename: it survived in
//! three different error strings, a doc comment and a test assertion, and
//! fixing two of them looked like fixing all of them. A path belongs here.
//!
//! `%APPDATA%` on Windows, `~/Library/Application Support` on macOS. Note that
//! `dirs::data_dir()` and `dirs::config_dir()` resolve to that same directory on
//! both supported platforms, which is why this single helper can serve call
//! sites that used either.

use std::path::{Path, PathBuf};

/// Folder under the platform data directory.
pub const APP_DATA_DIR_NAME: &str = "AgentsLauncher";

/// The folder name used before this app was renamed. Read only by
/// [`migrate_legacy_app_data_dir`], so an install from before the rename can be
/// carried across once instead of silently starting empty.
const LEGACY_APP_DATA_DIR_NAME: &str = "ClaudeEnvManager";

/// `<platform data dir>/<APP_DATA_DIR_NAME>`.
///
/// `None` only when the platform data directory itself cannot be resolved
/// (an unset `HOME`/`APPDATA`), which callers already treat as an error.
pub fn app_data_dir() -> Option<PathBuf> {
    dirs::data_dir().map(|dir| dir.join(APP_DATA_DIR_NAME))
}

/// [`app_data_dir`] as the `Result` the call sites speak.
pub fn app_data_dir_result() -> Result<PathBuf, String> {
    app_data_dir().ok_or_else(|| "无法确定应用数据目录".to_string())
}

/// Move a pre-rename data directory to the new name, once per install.
///
/// Returns `Ok(true)` when a move actually happened.
///
/// A **rename**, not a copy: two live trees that both look authoritative is how
/// a user ends up with a project list that keeps reverting to an older state.
/// Doing nothing when the new directory already exists is deliberate — that
/// means the renamed app has already run here, so the new tree holds this
/// install's state and merging an old one on top of it would be guesswork.
///
/// Best-effort by design: it returns a message for the caller to log rather
/// than an error that could block startup, because a failed migration leaves
/// the old directory intact and readable — nothing is lost by continuing.
pub fn migrate_legacy_app_data_dir() -> Result<bool, String> {
    match dirs::data_dir() {
        Some(data_dir) => migrate_within(&data_dir),
        // Nothing to resolve means nothing to move; every path helper will
        // report the same failure on its own.
        None => Ok(false),
    }
}

/// Testable core of [`migrate_legacy_app_data_dir`], against an explicit base.
fn migrate_within(data_dir: &Path) -> Result<bool, String> {
    let target = data_dir.join(APP_DATA_DIR_NAME);
    let legacy = data_dir.join(LEGACY_APP_DATA_DIR_NAME);
    if target.exists() || !legacy.is_dir() {
        return Ok(false);
    }
    std::fs::rename(&legacy, &target)
        .map_err(|error| format!("迁移旧数据目录 {} 失败：{error}", legacy.display()))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("agents-launcher-paths-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create scratch dir");
        dir
    }

    #[test]
    fn legacy_directory_is_moved_when_the_new_name_does_not_exist() {
        let base = scratch_dir("moves-legacy");
        let legacy = base.join(LEGACY_APP_DATA_DIR_NAME);
        std::fs::create_dir_all(legacy.join("codex")).expect("create legacy tree");
        std::fs::write(legacy.join("projects.json"), b"{\"projects\":[]}").expect("seed a file");

        assert!(migrate_within(&base).expect("migration should succeed"));

        // The whole tree moves, not just the top level.
        assert!(base.join(APP_DATA_DIR_NAME).join("projects.json").is_file());
        assert!(base.join(APP_DATA_DIR_NAME).join("codex").is_dir());
        assert!(!legacy.exists());
    }

    #[test]
    fn the_new_directory_wins_when_both_exist() {
        let base = scratch_dir("keeps-new");
        let legacy = base.join(LEGACY_APP_DATA_DIR_NAME);
        let target = base.join(APP_DATA_DIR_NAME);
        std::fs::create_dir_all(&legacy).expect("create legacy tree");
        std::fs::create_dir_all(&target).expect("create current tree");
        std::fs::write(legacy.join("marker"), b"legacy").expect("seed legacy marker");
        std::fs::write(target.join("marker"), b"current").expect("seed current marker");

        assert!(!migrate_within(&base).expect("migration should be a no-op"));

        // Neither side is touched: merging would have to guess which wins.
        assert_eq!(std::fs::read(target.join("marker")).unwrap(), b"current");
        assert_eq!(std::fs::read(legacy.join("marker")).unwrap(), b"legacy");
    }

    #[test]
    fn a_first_run_without_any_history_does_not_create_directories() {
        let base = scratch_dir("fresh-install");

        assert!(!migrate_within(&base).expect("migration should be a no-op"));

        assert!(!base.join(APP_DATA_DIR_NAME).exists());
    }

    #[test]
    fn running_the_migration_again_does_not_move_anything_else() {
        let base = scratch_dir("idempotent");
        let legacy = base.join(LEGACY_APP_DATA_DIR_NAME);
        std::fs::create_dir_all(&legacy).expect("create legacy tree");
        std::fs::write(legacy.join("projects.json"), b"one").expect("seed a file");

        assert!(migrate_within(&base).expect("first run should migrate"));

        // A new legacy tree appearing afterwards (an older build run once more)
        // must not clobber the migrated one.
        std::fs::create_dir_all(&legacy).expect("recreate legacy tree");
        std::fs::write(legacy.join("projects.json"), b"two").expect("seed the second tree");

        assert!(!migrate_within(&base).expect("second run should be a no-op"));
        assert_eq!(
            std::fs::read(base.join(APP_DATA_DIR_NAME).join("projects.json")).unwrap(),
            b"one"
        );
    }

    #[test]
    fn the_resolved_path_uses_the_new_folder_name() {
        let path = app_data_dir().expect("platform data dir should resolve in tests");

        assert!(path.ends_with(APP_DATA_DIR_NAME));
        assert!(!path.to_string_lossy().contains(LEGACY_APP_DATA_DIR_NAME));
    }
}
