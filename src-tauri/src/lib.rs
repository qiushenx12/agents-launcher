pub mod claude_launcher;
pub mod cli_capabilities;
pub mod cli_contract;
pub mod cli_migration;
pub mod cli_runtime;
pub mod codex_config;
pub mod config_store;
pub mod dependency_manager;
pub mod env_applier;
pub mod file_transaction;
pub mod model_fetcher;
pub mod opencode_config;
pub mod opencode_db;
pub mod persistent_state;
pub mod platform_env;
pub mod project_manager;
pub mod pty;
pub mod registry;
pub mod session_manager;
pub mod settings_manager;
pub mod tab_cli;
pub mod utils;

mod window_theme {
    #[cfg(target_os = "windows")]
    fn set_titlebar_dark_mode(hwnd: windows::Win32::Foundation::HWND, dark: bool) {
        use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWINDOWATTRIBUTE};
        let value: i32 = if dark { 1 } else { 0 };
        unsafe {
            let _ = DwmSetWindowAttribute(
                hwnd,
                DWMWINDOWATTRIBUTE(20),
                &value as *const _ as *const _,
                std::mem::size_of::<i32>() as u32,
            );
        }
    }

    #[tauri::command]
    pub fn set_titlebar_theme(window: tauri::Window, dark: bool) {
        #[cfg(target_os = "windows")]
        {
            let hwnd = windows::Win32::Foundation::HWND(window.hwnd().unwrap().0 as _);
            set_titlebar_dark_mode(hwnd, dark);
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = (window, dark);
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            session_manager::start_history_watcher(app.handle().clone());
            Ok(())
        })
        .manage(std::sync::Mutex::new(pty::PtyManager::new()))
        .invoke_handler(tauri::generate_handler![
            // shared CLI contract
            cli_contract::get_cli_contract,
            cli_runtime::check_cli,
            cli_runtime::discover_codex_projects,
            cli_runtime::list_codex_threads,
            cli_runtime::list_all_codex_threads,
            cli_runtime::discover_opencode_projects,
            cli_runtime::list_opencode_sessions,
            cli_runtime::list_all_opencode_sessions,
            // CodeX managed configuration
            codex_config::load_codex_profiles,
            codex_config::reveal_codex_profile_api_key,
            codex_config::fetch_codex_models,
            codex_config::save_codex_profile,
            codex_config::apply_codex_profile,
            codex_config::delete_codex_profile,
            codex_config::resolve_codex_profile,
            // OpenCode global JSONC synchronization
            opencode_config::check_opencode_permissions,
            opencode_config::repair_opencode_permissions,
            opencode_config::load_opencode_global_config,
            opencode_config::save_opencode_global_config,
            opencode_config::write_opencode_global_provider,
            opencode_config::delete_opencode_global_provider,
            opencode_config::save_opencode_global_options,
            opencode_config::fetch_opencode_global_models,
            opencode_config::save_opencode_provider_key,
            opencode_config::save_opencode_provider_connection,
            opencode_config::disconnect_opencode_provider,
            opencode_config::resolve_opencode_current_config,
            opencode_config::preview_opencode_current_config,
            // config_store commands
            config_store::load_claude_configs,
            config_store::save_claude_configs,
            // env_applier commands (cross-platform env vars)
            env_applier::apply_env_vars,
            // settings_manager commands
            settings_manager::load_claude_settings,
            settings_manager::save_claude_settings,
            settings_manager::load_claude_env,
            settings_manager::save_claude_env,
            // persistent_state commands
            persistent_state::load_window_state,
            persistent_state::save_window_state,
            persistent_state::load_launch_dir,
            persistent_state::save_launch_dir,
            persistent_state::load_terminal_font_size,
            persistent_state::save_terminal_font_size,
            persistent_state::load_pane_width,
            persistent_state::save_pane_width,
            persistent_state::load_config_order,
            persistent_state::save_config_order,
            persistent_state::load_active_profile_id,
            persistent_state::save_active_profile_id,
            persistent_state::load_profile_ids,
            persistent_state::save_profile_index,
            persistent_state::load_use_builtin_terminal,
            persistent_state::save_use_builtin_terminal,
            persistent_state::load_project_drop_path_mode,
            persistent_state::save_project_drop_path_mode,
            persistent_state::load_last_active_main_tab,
            persistent_state::save_last_active_main_tab,
            // project_manager commands
            project_manager::load_projects,
            project_manager::save_projects,
            project_manager::path_kind,
            project_manager::read_text_file,
            project_manager::save_text_file,
            // model_fetcher commands
            model_fetcher::fetch_claude_models,
            // session_manager commands
            session_manager::load_claude_sessions,
            session_manager::load_claude_recent_projects,
            // claude_launcher commands
            claude_launcher::launch_claude,
            claude_launcher::find_claude_executable,
            claude_launcher::check_claude_code_installed,
            claude_launcher::install_claude_code_via_npm,
            // dependency_manager commands
            dependency_manager::check_node_dependency,
            dependency_manager::check_git_dependency,
            dependency_manager::install_dependency_via_winget,
            // utils commands
            utils::get_claude_config_dir,
            utils::open_directory,
            utils::open_env_vars_dialog,
            utils::get_current_env_vars,
            // pty commands
            pty::pty_create,
            pty::pty_write,
            pty::pty_resize,
            pty::pty_kill,
            // tab_cli commands (inter-tab communication)
            tab_cli::set_tab_permission,
            tab_cli::get_tab_permission,
            tab_cli::save_terminal_snapshot,
            tab_cli::load_terminal_snapshot,
            tab_cli::list_terminal_snapshots,
            tab_cli::list_presets,
            tab_cli::save_preset,
            tab_cli::delete_preset,
            tab_cli::load_preset,
            // window theme
            window_theme::set_titlebar_theme,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|app_handle, event| {
        if let tauri::RunEvent::ExitRequested { .. } = event {
            pty::cleanup_all_sessions(app_handle);
        }
    });
}
