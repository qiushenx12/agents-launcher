pub mod claude_launcher;
pub mod config_store;
pub mod model_fetcher;
pub mod persistent_state;
pub mod project_manager;
pub mod pty;
pub mod tab_cli;
pub mod registry;
pub mod session_manager;
pub mod settings_manager;
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
            // config_store commands
            config_store::load_claude_configs,
            config_store::save_claude_configs,
            // registry commands
            registry::apply_env_vars,
            // settings_manager commands
            settings_manager::load_claude_settings,
            settings_manager::save_claude_settings,
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
