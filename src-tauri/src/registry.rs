//! registry.rs
//!
//! Windows registry operations for writing environment variables.
//!
//! User scope:   HKCU\Environment
//! System scope: HKLM\SYSTEM\CurrentControlSet\Control\Session Manager\Environment
//!
//! All values are written as REG_EXPAND_SZ.
//! Empty values cause the registry value to be deleted.
//! After every write, WM_SETTINGCHANGE is broadcast so running processes
//! pick up the change without a reboot.

use std::collections::HashMap;

#[cfg(windows)]
use winreg::{
    enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_ALL_ACCESS, REG_EXPAND_SZ},
    RegKey,
};

#[cfg(windows)]
use windows::Win32::{
    Foundation::LPARAM,
    UI::WindowsAndMessaging::{
        SendMessageTimeoutW, HWND_BROADCAST, SMTO_ABORTIFHUNG, WM_SETTINGCHANGE,
    },
};

// ---------------------------------------------------------------------------
// Broadcast helper
// ---------------------------------------------------------------------------

/// Broadcast WM_SETTINGCHANGE so the OS notifies running processes that the
/// environment has changed.  Mirrors Python's `broadcast_env_change()`.
///
/// The broadcast is run in a separate thread to avoid a deadlock: if this
/// function were called on the Tauri command handler thread, the Tauri window's
/// message loop would be blocked waiting for the command to return, while
/// SendMessageTimeoutW(HWND_BROADCAST) would be waiting for that same window to
/// process WM_SETTINGCHANGE — a classic deadlock.  Spawning a thread lets the
/// command return immediately so the window can handle the broadcast normally.
#[cfg(windows)]
fn broadcast_env_change() {
    std::thread::spawn(|| {
        // Encode "Environment" as a null-terminated wide string.
        let env_wide: Vec<u16> = "Environment\0".encode_utf16().collect();

        unsafe {
            let mut result = 0usize;
            SendMessageTimeoutW(
                HWND_BROADCAST,
                WM_SETTINGCHANGE,
                None,
                LPARAM(env_wide.as_ptr() as isize),
                SMTO_ABORTIFHUNG,
                5000,
                Some(&mut result),
            );
        }
    });
}

#[cfg(not(windows))]
fn broadcast_env_change() {
    // No-op on non-Windows platforms.
}

// ---------------------------------------------------------------------------
// Windows implementation used by env_applier::apply_env_vars
// ---------------------------------------------------------------------------

/// Apply a map of environment variables to the Windows registry.
///
/// `scope` must be `"user"` (HKCU) or `"system"` (HKLM).
///
/// - Non-empty values are written as REG_EXPAND_SZ.
/// - Empty values delete the registry entry (if it exists).
/// - After all writes, WM_SETTINGCHANGE is broadcast.
pub fn apply_env_vars_impl(vars: HashMap<String, String>, scope: String) -> Result<(), String> {
    #[cfg(windows)]
    {
        let (root_key, key_path) = if scope == "system" {
            (
                RegKey::predef(HKEY_LOCAL_MACHINE),
                r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment",
            )
        } else {
            (RegKey::predef(HKEY_CURRENT_USER), r"Environment")
        };

        let key = root_key
            .open_subkey_with_flags(key_path, KEY_ALL_ACCESS)
            .map_err(|e| {
                if scope == "system" {
                    format!(
                        "修改系统环境变量需要管理员权限!\n\n\
                         请右键点击此程序，选择'以管理员身份运行'。\n\n({e})"
                    )
                } else {
                    format!("无法修改当前用户的环境变量，请检查权限。\n\n({e})")
                }
            })?;

        for (var_name, value) in &vars {
            if value.is_empty() {
                // Delete the value; ignore "not found" errors.
                match key.delete_value(var_name) {
                    Ok(_) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => {
                        return Err(format!("Failed to delete registry value '{var_name}': {e}"));
                    }
                }
            } else {
                // Write as REG_EXPAND_SZ to preserve %VAR% expansions.
                key.set_raw_value(
                    var_name,
                    &winreg::RegValue {
                        bytes: encode_reg_expand_sz(value),
                        vtype: REG_EXPAND_SZ,
                    },
                )
                .map_err(|e| format!("Failed to write registry value '{var_name}': {e}"))?;
            }
        }

        broadcast_env_change();
        Ok(())
    }

    #[cfg(not(windows))]
    {
        let _ = (vars, scope);
        Err("Registry operations are only supported on Windows".to_string())
    }
}

pub(crate) fn read_user_env_var(name: &str) -> Result<Option<String>, String> {
    #[cfg(windows)]
    {
        let key = RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey(r"Environment")
            .map_err(|error| format!("无法读取当前用户环境变量：{error}"))?;
        match key.get_value::<String, _>(name) {
            Ok(value) => Ok(Some(value)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(format!("无法读取环境变量 '{name}'：{error}")),
        }
    }

    #[cfg(not(windows))]
    {
        let _ = name;
        Err("Registry operations are only supported on Windows".to_string())
    }
}

// ---------------------------------------------------------------------------
// Encoding helper
// ---------------------------------------------------------------------------

/// Encode a Rust `&str` as a null-terminated UTF-16LE byte sequence,
/// which is the on-disk format for REG_SZ / REG_EXPAND_SZ.
#[cfg(windows)]
fn encode_reg_expand_sz(value: &str) -> Vec<u8> {
    let wide: Vec<u16> = value.encode_utf16().chain(std::iter::once(0u16)).collect();
    wide.iter().flat_map(|w| w.to_le_bytes()).collect()
}
