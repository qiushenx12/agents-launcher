#[cfg(target_os = "macos")]
mod native {
    use std::sync::OnceLock;

    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, NSObject};
    use objc2::{define_class, msg_send, sel, ClassType};
    use objc2_app_kit::{NSWindow, NSWindowButton};
    use tauri::{Emitter, Manager};

    static APP_HANDLE: OnceLock<tauri::AppHandle> = OnceLock::new();

    define_class!(
        #[unsafe(super = NSObject)]
        struct AnimatedFullscreenTarget;

        impl AnimatedFullscreenTarget {
            #[unsafe(method(requestAnimatedFullscreen:))]
            fn request_animated_fullscreen(&self, _sender: &AnyObject) {
                let Some(app) = APP_HANDLE.get() else {
                    return;
                };
                // get_window 而非 get_webview_window：dsh 内嵌 child webview
                // 存在时后者的 is_webview_window 校验会失败返回 None。
                let Some(window) = app.get_window("main") else {
                    return;
                };
                let fullscreen = window.is_fullscreen().unwrap_or(false);
                let _ = window.emit("macos-fullscreen-toggle-requested", fullscreen);
            }
        }
    );

    pub fn install(
        app_handle: &tauri::AppHandle,
        window: &tauri::Window,
    ) -> Result<(), String> {
        let _ = APP_HANDLE.set(app_handle.clone());

        let target: Retained<AnimatedFullscreenTarget> =
            unsafe { msg_send![AnimatedFullscreenTarget::class(), new] };
        let ns_window =
            unsafe { &*(window.ns_window().map_err(|error| error.to_string())? as *mut NSWindow) };
        let zoom_button = ns_window
            .standardWindowButton(NSWindowButton::ZoomButton)
            .ok_or_else(|| "macOS zoom button is unavailable".to_string())?;

        unsafe {
            // NSControl keeps its target weakly. This one target intentionally
            // lives for the process lifetime so the native green button can
            // always forward clicks to the webview animation sequence.
            zoom_button.setTarget(Some(&target));
            zoom_button.setAction(Some(sel!(requestAnimatedFullscreen:)));
        }
        let _ = Retained::into_raw(target);
        Ok(())
    }

    /// Activates the application so a restored window can come forward.
    ///
    /// tao's `Window::set_focus` relies on `activateIgnoringOtherApps:`,
    /// which Apple deprecated in macOS 14 (Sonoma): it no longer activates
    /// the app, so a window shown after `hide()` stays on its old Space,
    /// never becomes key, and looks "unrestorable" from the tray or Dock.
    /// Use `-[NSApplication activate]` on macOS 14+ and fall back to the
    /// legacy selector on macOS 13 (the bundle's minimum system version).
    pub fn activate_application() {
        use objc2::MainThreadMarker;
        use objc2_app_kit::NSApplication;

        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let app = NSApplication::sharedApplication(mtm);
        unsafe {
            let supports_activate: bool =
                msg_send![NSApplication::class(), instancesRespondToSelector: sel!(activate)];
            if supports_activate {
                let _: () = msg_send![&app, activate];
            } else {
                let _: () = msg_send![&app, activateIgnoringOtherApps: true];
            }
        }
    }

    /// Order the window in by talking to AppKit directly, bypassing the tao
    /// dispatcher message queue: `Window::show()` only posts a message that the
    /// event loop applies later, while `orderFrontRegardless` puts the window
    /// back on screen immediately, even before `activate()` takes effect.
    pub fn order_window_front<R: tauri::Runtime>(window: &tauri::Window<R>) {
        let Ok(ns_window_ptr) = window.ns_window() else {
            return;
        };
        let ns_window = unsafe { &*(ns_window_ptr as *const NSWindow) };
        ns_window.orderFrontRegardless();
    }
}

pub fn install_animated_fullscreen_button(
    app_handle: &tauri::AppHandle,
    window: &tauri::Window,
) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        native::install(app_handle, window)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (app_handle, window);
        Ok(())
    }
}

/// Activates the application before focusing a restored window. No-op outside
/// macOS, where `Window::set_focus` already activates the app correctly.
pub fn activate_application() {
    #[cfg(target_os = "macos")]
    native::activate_application();
}

/// Order the window to the front regardless of activation state. No-op
/// outside macOS.
pub fn order_window_front<R: tauri::Runtime>(window: &tauri::Window<R>) {
    #[cfg(target_os = "macos")]
    native::order_window_front(window);
    #[cfg(not(target_os = "macos"))]
    let _ = window;
}

/// Toggles the native macOS fullscreen mode.
///
/// On macOS, Tauri implements `set_fullscreen` through AppKit's
/// `toggleFullScreen:` selector, so both entering and leaving fullscreen use
/// the system transition animation and respect the user's Reduce Motion
/// preference.
#[tauri::command]
pub fn toggle_animated_fullscreen(window: tauri::Window) -> Result<bool, String> {
    let fullscreen = window.is_fullscreen().map_err(|error| error.to_string())?;
    let next_fullscreen = !fullscreen;
    window
        .set_fullscreen(next_fullscreen)
        .map_err(|error| error.to_string())?;
    Ok(next_fullscreen)
}
