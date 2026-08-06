//! Small transactional writer for user-owned JSON files.
//!
//! Content is validated before the current file is moved aside. A successful
//! commit keeps a `.bak` copy; a failed commit restores that backup.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::Value;

fn sidecar_path(path: &Path, suffix: &str) -> PathBuf {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| format!("{value}.{suffix}"))
        .unwrap_or_else(|| suffix.to_string());
    path.with_extension(extension)
}

#[derive(Clone, Copy)]
enum JsonFilePrivacy {
    Preserve,
    CurrentUserOnly,
}

pub fn write_json_atomic(path: &Path, content: &[u8], label: &str) -> Result<(), String> {
    write_json_atomic_with_privacy(path, content, label, JsonFilePrivacy::Preserve)
}

/// Atomically write JSON that may contain a plaintext credential.
///
/// Unix creates the containing directory as `0700` and keeps the current,
/// temporary, and backup files at `0600`. Windows continues to rely on its
/// ACLs and uses the same transactional sequence as ordinary JSON.
pub fn write_private_json_atomic(path: &Path, content: &[u8], label: &str) -> Result<(), String> {
    write_json_atomic_with_privacy(path, content, label, JsonFilePrivacy::CurrentUserOnly)
}

fn write_json_atomic_with_privacy(
    path: &Path,
    content: &[u8],
    label: &str,
    privacy: JsonFilePrivacy,
) -> Result<(), String> {
    serde_json::from_slice::<Value>(content)
        .map_err(|error| format!("{label} 写入内容不是有效 JSON：{error}"))?;

    let parent = path
        .parent()
        .ok_or_else(|| format!("{label} 文件没有父目录"))?;
    fs::create_dir_all(parent).map_err(|error| format!("无法创建 {label} 目录：{error}"))?;
    set_private_directory_permissions(parent, privacy, label)?;

    let temp_path = sidecar_path(path, "tmp");
    let backup_path = sidecar_path(path, "bak");
    let write_result = (|| {
        let mut temp_options = OpenOptions::new();
        temp_options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        if matches!(privacy, JsonFilePrivacy::CurrentUserOnly) {
            use std::os::unix::fs::OpenOptionsExt;
            temp_options.mode(0o600);
        }
        let mut temp = temp_options
            .open(&temp_path)
            .map_err(|error| format!("无法创建 {label} 临时文件：{error}"))?;
        set_private_file_permissions(&temp_path, privacy, label)?;
        temp.write_all(content)
            .map_err(|error| format!("无法写入 {label} 临时文件：{error}"))?;
        temp.sync_all()
            .map_err(|error| format!("无法刷新 {label} 临时文件：{error}"))?;
        drop(temp);

        let verification =
            fs::read(&temp_path).map_err(|error| format!("无法校验 {label} 临时文件：{error}"))?;
        serde_json::from_slice::<Value>(&verification)
            .map_err(|error| format!("{label} 临时文件校验失败：{error}"))?;

        if path.exists() {
            // Tighten the current file before moving it. A failed chmod must
            // leave the committed file in place instead of stranding it as a
            // backup with no current file.
            set_private_file_permissions(path, privacy, label)?;
            if backup_path.exists() {
                set_private_file_permissions(&backup_path, privacy, label)?;
                fs::remove_file(&backup_path)
                    .map_err(|error| format!("无法替换 {label} 备份：{error}"))?;
            }
            fs::rename(path, &backup_path).map_err(|error| format!("无法备份 {label}：{error}"))?;
        }

        if let Err(error) = fs::rename(&temp_path, path) {
            if backup_path.exists() && !path.exists() {
                let _ = fs::rename(&backup_path, path);
            }
            return Err(format!("无法提交 {label}：{error}"));
        }
        // rename preserves the mode already applied to the temporary file,
        // so there is no fallible permission change after the commit point.

        Ok(())
    })();

    if write_result.is_err() && temp_path.exists() {
        let _ = fs::remove_file(&temp_path);
    }
    write_result
}

fn set_private_directory_permissions(
    path: &Path,
    privacy: JsonFilePrivacy,
    label: &str,
) -> Result<(), String> {
    #[cfg(unix)]
    if matches!(privacy, JsonFilePrivacy::CurrentUserOnly) {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .map_err(|error| format!("无法限制 {label} 目录权限：{error}"))?;
    }
    let _ = (path, privacy, label);
    Ok(())
}

fn set_private_file_permissions(
    path: &Path,
    privacy: JsonFilePrivacy,
    label: &str,
) -> Result<(), String> {
    #[cfg(unix)]
    if matches!(privacy, JsonFilePrivacy::CurrentUserOnly) {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
            .map_err(|error| format!("无法限制 {label} 文件权限：{error}"))?;
    }
    let _ = (path, privacy, label);
    Ok(())
}

pub fn restore_json_backup_if_missing(path: &Path, label: &str) -> Result<bool, String> {
    restore_json_backup_if_missing_with_privacy(path, label, JsonFilePrivacy::Preserve)
}

pub fn restore_private_json_backup_if_missing(path: &Path, label: &str) -> Result<bool, String> {
    restore_json_backup_if_missing_with_privacy(path, label, JsonFilePrivacy::CurrentUserOnly)
}

fn restore_json_backup_if_missing_with_privacy(
    path: &Path,
    label: &str,
    privacy: JsonFilePrivacy,
) -> Result<bool, String> {
    if path.exists() {
        return Ok(false);
    }
    let backup_path = sidecar_path(path, "bak");
    if !backup_path.exists() {
        return Ok(false);
    }

    let content =
        fs::read(&backup_path).map_err(|error| format!("无法读取 {label} 备份：{error}"))?;
    serde_json::from_slice::<Value>(&content)
        .map_err(|error| format!("{label} 备份不是有效 JSON：{error}"))?;
    write_json_atomic_with_privacy(path, &content, label, privacy)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn temp_file(name: &str) -> PathBuf {
        std::env::temp_dir()
            .join(format!("agents-launcher-{}", Uuid::new_v4()))
            .join(name)
    }

    #[test]
    fn invalid_json_never_replaces_the_current_file() {
        let path = temp_file("state.json");
        fs::create_dir_all(path.parent().expect("parent")).expect("create temp directory");
        fs::write(&path, br#"{"safe":true}"#).expect("write original");

        let error =
            write_json_atomic(&path, b"{invalid", "测试配置").expect_err("invalid JSON must fail");

        assert!(error.contains("不是有效 JSON"));
        assert_eq!(
            fs::read_to_string(&path).expect("read original"),
            r#"{"safe":true}"#
        );
        let _ = fs::remove_dir_all(path.parent().expect("parent"));
    }

    #[test]
    fn successful_commit_keeps_a_verified_backup() {
        let path = temp_file("state.json");
        fs::create_dir_all(path.parent().expect("parent")).expect("create temp directory");
        fs::write(&path, br#"{"version":1}"#).expect("write original");

        write_json_atomic(&path, br#"{"version":2}"#, "测试配置").expect("commit");

        assert_eq!(
            fs::read_to_string(&path).expect("read current"),
            r#"{"version":2}"#
        );
        assert_eq!(
            fs::read_to_string(sidecar_path(&path, "bak")).expect("read backup"),
            r#"{"version":1}"#,
        );
        let _ = fs::remove_dir_all(path.parent().expect("parent"));
    }

    #[test]
    fn missing_current_file_is_restored_from_a_verified_backup() {
        let path = temp_file("state.json");
        fs::create_dir_all(path.parent().expect("parent")).expect("create temp directory");
        fs::write(sidecar_path(&path, "bak"), br#"{"version":1}"#).expect("write backup");

        assert!(restore_json_backup_if_missing(&path, "测试配置").expect("restore backup"));
        assert_eq!(
            fs::read_to_string(&path).expect("read restored file"),
            r#"{"version":1}"#,
        );
        let _ = fs::remove_dir_all(path.parent().expect("parent"));
    }

    #[cfg(unix)]
    #[test]
    fn private_json_keeps_directory_current_and_backup_user_only() {
        use std::os::unix::fs::PermissionsExt;

        let path = temp_file("private/auth.json");
        fs::create_dir_all(path.parent().expect("parent")).expect("create temp directory");
        fs::write(&path, br#"{"key":"old"}"#).expect("write original");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).expect("loosen original");

        write_private_json_atomic(&path, br#"{"key":"new"}"#, "测试凭据").expect("commit");

        assert_eq!(
            fs::metadata(&path)
                .expect("current metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(sidecar_path(&path, "bak"))
                .expect("backup metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600,
        );
        assert_eq!(
            fs::metadata(path.parent().expect("parent"))
                .expect("directory metadata")
                .permissions()
                .mode()
                & 0o777,
            0o700,
        );
        assert!(!sidecar_path(&path, "tmp").exists());
        let _ = fs::remove_dir_all(path.parent().and_then(Path::parent).expect("temp root"));
    }
}
