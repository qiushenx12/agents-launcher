//! Child-WebView hosting for the dsh browser UI.
//!
//! dsh cannot be embedded with an `<iframe>`: its index page trades a `?token=`
//! query for an `HttpOnly; SameSite=Strict` cookie bound to the authority, and
//! a cross-site frame (the app's top-level origin is `http://tauri.localhost`)
//! never gets that cookie — every `/api` call and WebSocket then stays at 401.
//! A child WebView is its own top-level browsing context, so cookies work.
//!
//! Three constraints shape this module:
//!
//! * **Every command is `async fn`.** On Windows, creating a webview from a
//!   synchronous command deadlocks the whole application (no error, just a
//!   frozen UI).
//! * **`add_child` needs Tauri's `unstable` feature.** That is the direct cost
//!   of embedding; the usage is confined to this file so a future Tauri upgrade
//!   only has one place to fix.
//! * **A child WebView is a native control painted above the window's DOM.**
//!   Vue cannot hide it with `v-show` or by unmounting: switching tabs or
//!   opening an overlay must call [`dsh_embed_hide`] explicitly. See §4.9 of
//!   `docs/dsh-integration-plan.md` for the overlay checklist.
//!
//! # Lifecycle invariants
//!
//! The control is expensive and stateful, so the rules below are enforced here
//! rather than relied on from the frontend:
//!
//! 1. **One creation at a time.** `Window::add_child` hops to the main thread
//!    and builds a whole WebView2 environment there, so it blocks the event loop
//!    for hundreds of milliseconds. Two concurrent shows used to queue two
//!    creations, each overwriting the previous handle and destroying the other's
//!    webview — a frozen UI and, in the worst ordering, a live native control
//!    that no state pointed at (and therefore nothing could hide). Creation and
//!    destruction now hold [`embed_lifecycle`], so the second caller waits and
//!    then simply repositions the control that already exists.
//! 2. **Visibility follows intent, not call order.** Every show records
//!    `desired_visible = true` *before* it starts creating. A hide that lands
//!    during creation therefore wins: the show finishes, sees the intent was
//!    revoked and keeps the control hidden instead of putting it back on screen.
//! 3. **Creation is transactional.** A failure after the control exists (bad
//!    URL, rejected bounds) destroys it and clears the state. Leaving it behind
//!    produced a visible orphan the app could no longer reach.
//! 4. **The frontend owns geometry.** `set_auto_resize` is deliberately *not*
//!    used: it turns bounds into ratios of the window and re-applies them on
//!    every `WM_SIZE`, i.e. a second source of truth that moves the control
//!    during exactly the transitions the frontend is measuring for. Re-measuring
//!    the placeholder is cheap; guessing from a ratio is not the same rectangle.

use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::Duration;

use serde::Serialize;
use tauri::webview::Color;
use tauri::{LogicalPosition, LogicalSize, Manager, Webview, WebviewUrl};

/// Label kept distinct from the main webview window's `"main"` label.
const EMBED_LABEL: &str = "dsh-embed";

/// WebView2's built-in default background is plain white, which reads as a
/// bright flash in a dark window before the embedded page paints its first
/// frame. These match the dsh web UI's own boot-screen backgrounds
/// (`--dsh-boot-bg: #151517` dark / `#fff` light), so the chain
/// control-created → boot screen → app UI stays the same color throughout.
const BOOT_BACKGROUND_DARK: Color = Color(0x15, 0x15, 0x17, 0xff);
const BOOT_BACKGROUND_LIGHT: Color = Color(0xff, 0xff, 0xff, 0xff);

/// Cap for the diagnostic trace, so a long session cannot fill the disk.
const DEBUG_LOG_MAX_BYTES: u64 = 256 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DshEmbedResult {
    pub ok: bool,
    /// True when the webview exists and is currently visible.
    pub visible: bool,
    /// True when a webview was created by this call (diagnostics only).
    pub created: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct EmbedBounds {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

impl EmbedBounds {
    fn is_usable(self) -> bool {
        self.width >= 1.0 && self.height >= 1.0 && self.x.is_finite() && self.y.is_finite()
    }
}

struct EmbedState {
    webview: Option<Webview>,
    /// Token-bearing URL currently loaded; compared on every show so a service
    /// restart (which mints a new token) reloads instead of staying at 401.
    url: Option<String>,
    visible: bool,
    bounds: Option<EmbedBounds>,
    /// Last visibility *intent* from the frontend. See invariant 2.
    desired_visible: bool,
}

fn embed_state() -> &'static Mutex<EmbedState> {
    static STATE: OnceLock<Mutex<EmbedState>> = OnceLock::new();
    STATE.get_or_init(|| {
        Mutex::new(EmbedState {
            webview: None,
            url: None,
            visible: false,
            bounds: None,
            desired_visible: false,
        })
    })
}

/// Serializes every operation that can create or destroy the native control.
/// Never held together with the state lock in the opposite order.
fn embed_lifecycle() -> &'static Mutex<()> {
    static LIFECYCLE: OnceLock<Mutex<()>> = OnceLock::new();
    LIFECYCLE.get_or_init(|| Mutex::new(()))
}

fn state_lock() -> Result<MutexGuard<'static, EmbedState>, String> {
    embed_state()
        .lock()
        .map_err(|_| "dsh 内嵌状态不可用".to_string())
}

fn lifecycle_lock() -> Result<MutexGuard<'static, ()>, String> {
    embed_lifecycle()
        .lock()
        .map_err(|_| "dsh 内嵌状态不可用".to_string())
}

fn snapshot(created: bool) -> DshEmbedResult {
    let visible = state_lock().map(|state| state.visible).unwrap_or(false);
    DshEmbedResult {
        ok: true,
        visible,
        created,
    }
}

/// Read the live URL from the runtime supervisor. Never cached in the frontend
/// store, never persisted: the token is a full-access credential.
fn current_url() -> Option<String> {
    crate::dsh_runtime::dsh_runtime_urls()
        .ok()
        .flatten()
        .map(|urls| urls.local_url)
}

fn parse_url(raw: &str) -> Result<url::Url, String> {
    url::Url::parse(raw).map_err(|error| format!("dsh 界面地址无效: {error}"))
}

/// Append a line to `<app data>/dsh/embed-debug.log` (diagnostic only).
///
/// Deliberately reads *our own* state and never calls into the webview
/// dispatcher. `Webview::position()`/`Webview::size()` are blocking getters: they
/// post a message to the event loop and wait for the answer, so while the event
/// loop is building a webview they hang for the whole duration. That is how this
/// trace previously went silent at the exact moment the embed first appeared —
/// the last four lines had no native geometry because the fifth call never
/// returned. The bounds we were asked for are just as informative and cannot
/// block.
#[tauri::command]
pub async fn dsh_debug_log(line: String) -> Result<(), String> {
    let Ok(dir) = dirs::data_dir()
        .map(|path| path.join("ClaudeEnvManager").join("dsh"))
        .ok_or_else(|| "no data dir".to_string())
    else {
        return Ok(());
    };
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("embed-debug.log");
    let native = match state_lock() {
        Ok(state) => format!(
            " | native={} visible={} bounds={}",
            if state.webview.is_some() { "tracked" } else { "none" },
            state.visible,
            match state.bounds {
                Some(bounds) => format!(
                    "{:.0},{:.0} {:.0}x{:.0}",
                    bounds.x, bounds.y, bounds.width, bounds.height
                ),
                None => "n/a".to_string(),
            },
        ),
        Err(error) => format!(" | state unavailable: {error}"),
    };
    if std::fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0) > DEBUG_LOG_MAX_BYTES {
        let _ = std::fs::remove_file(&path);
    }
    use std::io::Write;
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = writeln!(file, "{line}{native}");
    }
    Ok(())
}

/// Drop the native control. Must never be called while holding the state lock.
fn destroy(webview: Webview) {
    let _ = webview.hide();
    if let Err(error) = webview.close() {
        eprintln!("[dsh] 关闭内嵌 WebView 失败: {error}");
    }
}

fn create_webview(
    window: &tauri::Window,
    url: &url::Url,
    bounds: EmbedBounds,
    dark_theme: bool,
) -> Result<Webview, String> {
    let builder = tauri::webview::WebviewBuilder::new(EMBED_LABEL, WebviewUrl::External(url.clone()))
        .background_color(if dark_theme {
            BOOT_BACKGROUND_DARK
        } else {
            BOOT_BACKGROUND_LIGHT
        })
        // Tauri installs its own drag-drop handler on every webview, which
        // converts drops into `tauri://drag-drop` events the page never sees.
        // The dsh UI uses plain HTML5 drag & drop (like in a browser), so the
        // handler has to go — otherwise dropping a file into the embedded page
        // silently does nothing.
        .disable_drag_drop_handler();
    let webview = window
        .add_child(
            builder,
            LogicalPosition::new(bounds.x, bounds.y),
            LogicalSize::new(bounds.width, bounds.height),
        )
        .map_err(|error| format!("无法创建 dsh 内嵌界面: {error}"))?;
    // No `set_auto_resize`: Tauri would then reposition the control from ratios
    // of the window on every resize, which is a second geometry source that
    // fights the measured rect (and fires exactly during maximize / restore /
    // DPI transitions). The panel re-measures instead — see invariant 4.
    Ok(webview)
}

/// Show (creating or repositioning as needed) the embedded dsh UI.
///
/// `width`/`height` are logical pixels measured by the frontend's placeholder
/// element; DPI scaling stays Tauri's business. `theme` is the launcher's
/// current theme, used only to pick the creation-time background color.
#[tauri::command]
pub async fn dsh_embed_show(
    app: tauri::AppHandle,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    theme: Option<String>,
) -> Result<DshEmbedResult, String> {
    let bounds = EmbedBounds {
        x,
        y,
        width,
        height,
    };
    if !bounds.is_usable() {
        return Err("dsh 内嵌区域尺寸无效。".to_string());
    }
    let dark_theme = theme.as_deref() == Some("dark");
    // Everything below blocks (creating a WebView2 environment round-trips to the
    // main thread), so it runs on the blocking pool instead of occupying an async
    // worker — and, more importantly, instead of the event loop.
    tauri::async_runtime::spawn_blocking(move || {
        let Some(url) = current_url() else {
            return Err("dsh 服务未运行，无法显示内嵌界面。".to_string());
        };
        let parsed = parse_url(&url)?;
        show_blocking(&app, &url, parsed, bounds, dark_theme)
    })
    .await
    .map_err(|error| format!("dsh 内嵌界面任务异常结束: {error}"))?
}

fn show_blocking(
    app: &tauri::AppHandle,
    url: &str,
    parsed: url::Url,
    bounds: EmbedBounds,
    dark_theme: bool,
) -> Result<DshEmbedResult, String> {
    // Intent first, work second: a hide that arrives while the creation below is
    // running must win, and it can only win if it can observe the revocation.
    state_lock()?.desired_visible = true;

    let _lifecycle = lifecycle_lock()?;
    // Re-read the intent now that creations are serialized: a hide that already
    // completed is the newer instruction, so this show must not resurrect the
    // control (the panel asks again on its next layout event).
    if !state_lock()?.desired_visible {
        return Ok(snapshot(false));
    }

    let (needs_create, needs_navigate, bounds_unchanged) = {
        let state = state_lock()?;
        match state.webview.as_ref() {
            // A fresh webview is built with the URL already loaded.
            None => (true, false, false),
            Some(_) => (
                false,
                state.url.as_deref() != Some(url),
                state.bounds == Some(bounds),
            ),
        }
    };

    if needs_create {
        let window = app
            .get_window("main")
            .ok_or_else(|| "找不到主窗口".to_string())?;
        let webview = create_webview(&window, &parsed, bounds, dark_theme)?;
        let previous = state_lock()?.webview.replace(webview);
        if let Some(previous) = previous {
            destroy(previous);
        }
    }

    let webview = state_lock()?
        .webview
        .clone()
        .ok_or_else(|| "dsh 内嵌界面创建失败".to_string())?;

    // Transactional from here on: the control exists, so every failure has to
    // tear it down. Returning early used to leave a created (and visible) native
    // control that no state referenced — the app could not hide it any more.
    let applied = (|| -> Result<(), String> {
        if needs_navigate {
            webview
                .navigate(parsed)
                .map_err(|error| format!("无法重新加载 dsh 界面: {error}"))?;
        }
        // Skip geometry that is already applied: WebView2 repaints on every
        // `put_Bounds`, even for an identical rectangle, and the re-measure
        // ladder issues several shows while the layout settles — each redundant
        // repaint used to read as a visible flicker.
        if !bounds_unchanged {
            webview
                .set_position(LogicalPosition::new(bounds.x, bounds.y))
                .map_err(|error| format!("无法定位 dsh 内嵌界面: {error}"))?;
            webview
                .set_size(LogicalSize::new(bounds.width, bounds.height))
                .map_err(|error| format!("无法调整 dsh 内嵌界面尺寸: {error}"))?;
        }
        Ok(())
    })();
    if let Err(error) = applied {
        let leaked = {
            let mut state = state_lock()?;
            state.url = None;
            state.bounds = None;
            state.visible = false;
            state.webview.take()
        };
        if let Some(webview) = leaked {
            destroy(webview);
        }
        return Err(error);
    }

    let should_show = {
        let mut state = state_lock()?;
        state.url = Some(url.to_string());
        state.bounds = Some(bounds);
        state.desired_visible
    };
    if !should_show {
        // The intent was revoked while this call was creating the control: keep
        // it off screen and report the truth.
        let _ = webview.hide();
        state_lock()?.visible = false;
        return Ok(DshEmbedResult {
            ok: true,
            visible: false,
            created: needs_create,
        });
    }
    webview
        .show()
        .map_err(|error| format!("无法显示 dsh 内嵌界面: {error}"))?;
    state_lock()?.visible = true;
    Ok(DshEmbedResult {
        ok: true,
        visible: true,
        created: needs_create,
    })
}

/// Hide the native control. Required whenever a DOM overlay opens or the dsh
/// tab is switched away: the WebView is not part of the DOM and would cover
/// the overlay completely.
///
/// Takes the lifecycle lock so a hide issued while a creation is in flight runs
/// *after* it and therefore does hide the control that was just created, instead
/// of racing it (and losing) as it used to.
#[tauri::command]
pub async fn dsh_embed_hide() -> Result<DshEmbedResult, String> {
    tauri::async_runtime::spawn_blocking(hide_blocking)
        .await
        .map_err(|error| format!("dsh 隐藏任务异常结束: {error}"))?
}

fn hide_blocking() -> Result<DshEmbedResult, String> {
    let _lifecycle = lifecycle_lock()?;
    let webview = {
        let mut state = state_lock()?;
        state.desired_visible = false;
        state.visible = false;
        state.webview.clone()
    };
    if let Some(webview) = webview {
        let _ = webview.hide();
    }
    Ok(snapshot(false))
}

/// Destroy the native control (closing the dsh service, or exiting).
#[tauri::command]
pub async fn dsh_embed_close() -> Result<DshEmbedResult, String> {
    tauri::async_runtime::spawn_blocking(close_blocking)
        .await
        .map_err(|error| format!("dsh 关闭任务异常结束: {error}"))?
}

fn close_blocking() -> Result<DshEmbedResult, String> {
    let _lifecycle = lifecycle_lock()?;
    let webview = {
        let mut state = state_lock()?;
        state.desired_visible = false;
        state.visible = false;
        state.url = None;
        state.bounds = None;
        state.webview.take()
    };
    if let Some(webview) = webview {
        destroy(webview);
    }
    Ok(snapshot(false))
}

/// Reload with the current token URL. A restarted dsh mints a new process token,
/// so the old page would sit at 401 forever.
///
/// Serialized with creation/destruction: navigating a control that is being
/// replaced would target a handle that is about to disappear.
#[tauri::command]
pub async fn dsh_embed_reload() -> Result<DshEmbedResult, String> {
    tauri::async_runtime::spawn_blocking(reload_blocking)
        .await
        .map_err(|error| format!("dsh 重新加载任务异常结束: {error}"))?
}

fn reload_blocking() -> Result<DshEmbedResult, String> {
    let Some(url) = current_url() else {
        return Err("dsh 服务未运行，无法重新加载。".to_string());
    };
    let parsed = parse_url(&url)?;
    let _lifecycle = lifecycle_lock()?;
    let webview = state_lock()?.webview.clone();
    let Some(webview) = webview else {
        return Ok(snapshot(false));
    };
    // A restart always mints a new token, i.e. a new URL — so an unchanged URL
    // means nothing to reload. This matters because the panel also calls reload
    // on every mount: navigating unconditionally used to re-render the whole
    // dsh page (a white flash) on every tab switch back to dsh.
    if state_lock()?.url.as_deref() == Some(url.as_str()) {
        return Ok(snapshot(false));
    }
    webview
        .navigate(parsed)
        .map_err(|error| format!("无法重新加载 dsh 界面: {error}"))?;
    state_lock()?.url = Some(url);
    Ok(snapshot(false))
}

/// Close the embedded webview during application shutdown.
///
/// Called from the run-event callback, which already sits on the main thread;
/// `Webview::close` posts to the runtime dispatcher, so this cannot deadlock.
/// The lifecycle lock is taken best-effort only: at exit a creation that is still
/// waiting on this very thread will never finish, so waiting for it would hang
/// the shutdown instead.
pub fn close_on_exit() {
    let _lifecycle = embed_lifecycle().try_lock();
    let webview = state_lock().ok().and_then(|mut state| {
        state.desired_visible = false;
        state.visible = false;
        state.webview.take()
    });
    if let Some(webview) = webview {
        // Give the dispatcher a moment to process the teardown before the
        // process exits underneath it.
        let _ = webview.hide();
        let _ = webview.close();
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounds_reject_zero_and_non_finite_sizes() {
        assert!(!EmbedBounds {
            x: 0.0,
            y: 0.0,
            width: 0.0,
            height: 100.0
        }
        .is_usable());
        assert!(!EmbedBounds {
            x: 0.0,
            y: 0.0,
            width: 100.0,
            height: 0.5
        }
        .is_usable());
        assert!(!EmbedBounds {
            x: f64::NAN,
            y: 0.0,
            width: 10.0,
            height: 10.0
        }
        .is_usable());
        assert!(EmbedBounds {
            x: -5.0,
            y: 40.0,
            width: 800.0,
            height: 600.0
        }
        .is_usable());
    }

    #[test]
    fn parse_url_rejects_garbage() {
        assert!(parse_url("not a url").is_err());
        assert!(parse_url("http://127.0.0.1:3080/?token=abc").is_ok());
    }

    /// The intent flag is what makes "hide during an in-flight show" win, so the
    /// state has to start hidden: a show that never recorded its intent must not
    /// be able to put the control on screen.
    #[test]
    fn visibility_intent_starts_revoked() {
        let state = embed_state().lock().expect("state lock");
        assert!(!state.desired_visible);
    }

    /// `close_on_exit` runs on the main thread while creations block on it, so it
    /// must never wait for the lifecycle lock.
    #[test]
    fn exit_teardown_does_not_wait_for_the_lifecycle_lock() {
        let guard = lifecycle_lock().expect("lifecycle lock");
        let taken = embed_lifecycle().try_lock();
        assert!(taken.is_err(), "the exit path must not block on a creation");
        drop(guard);
        assert!(embed_lifecycle().try_lock().is_ok());
    }
}
