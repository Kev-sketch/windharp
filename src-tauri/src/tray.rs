// tray.rs — tray icon + click/window-close behaviors, port of electron_tray_legacy.ts
//
// Legacy mapping:
//   electron_tray_legacy.ts : createTray()            -> setup_tray()
//   electron_tray_legacy.ts : updateTrayMenu()        -> update_tray()
//   electron_tray_legacy.ts : createWindow() (show)   -> show_window()
//   electron_tray_legacy.ts : window 'close' handler   -> handle_close()
//   electron_tray_legacy.ts : tray.on('click')        -> on_tray_icon_event handler
//
// Port limitation (accepted): the "with-song-info" tray tooltip variant
// (artist - title - applicationName) is skipped; tooltip stays static.

use std::sync::atomic::{AtomicBool, Ordering};

use log::{info, warn};
use serde_json::json;
use tauri::menu::{MenuBuilder, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager};

use crate::config::SharedStore;

/// Shared tray state, managed via `.manage(SharedTray)` in lib.rs.
/// Port of legacy `isPaused` tracker in electron_tray_legacy.ts.
pub type SharedTray = std::sync::Mutex<TrayState>;

#[derive(Debug, Default)]
pub struct TrayState {
    /// None = unknown (use default icon); Some(true) = paused; Some(false) = playing.
    pub is_paused: Option<bool>,
}

// Tray icon assets, EMBEDDED at compile time via include_bytes!.
// All four PNGs are 16x16 on disk (sips-resized from the 144x144 originals —
// tauri 2.12 `Image` has no runtime `resize`, so the files must already be
// tray-sized). Embedding (rather than reading `assets/…` from the CWD at
// runtime) keeps the tray working when the .app is launched from /Applications
// or Finder, where the old repo-root-relative path did not exist.
#[cfg(target_os = "macos")]
const TRAY_ICON_BYTES: &[u8] = include_bytes!("../../assets/tray-white.png");
#[cfg(target_os = "macos")]
const TRAY_ICON_PAUSED_BYTES: &[u8] = include_bytes!("../../assets/tray-paused-white.png");
#[cfg(not(target_os = "macos"))]
const TRAY_ICON_BYTES: &[u8] = include_bytes!("../../assets/tray.png");
#[cfg(not(target_os = "macos"))]
const TRAY_ICON_PAUSED_BYTES: &[u8] = include_bytes!("../../assets/tray-paused.png");

const TRAY_ID: &str = "windharp-tray";
const MAIN_WINDOW: &str = "main";
const TOOLTIP: &str = "Windharp";

fn tray_icon_bytes(paused: bool) -> &'static [u8] {
    if paused {
        TRAY_ICON_PAUSED_BYTES
    } else {
        TRAY_ICON_BYTES
    }
}

/// Load the tray icon from bytes embedded at compile time (16x16 PNGs).
fn load_tray_icon(paused: bool) -> tauri::Result<tauri::image::Image<'static>> {
    tauri::image::Image::from_bytes(tray_icon_bytes(paused))
}

/// Legacy read of config `options.tray` (truthy check).
fn tray_config_enabled(app: &AppHandle) -> bool {
    let state = app.try_state::<SharedStore>();
    let Some(store) = state else {
        warn!("[tray] config store not managed; assuming tray disabled");
        return false;
    };
    let Ok(store) = store.lock() else {
        warn!("[tray] config store poisoned; assuming tray disabled");
        return false;
    };
    if let Some(b) = store.get_bool("options.tray") {
        return b;
    }
    // Fall back to a generic truthiness check (legacy `config.get("options.tray")`).
    let v = store.get("options.tray");
    match v {
        serde_json::Value::Null => false,
        serde_json::Value::Bool(b) => b,
        serde_json::Value::Number(n) => n.as_f64().map(|f| f != 0.0).unwrap_or(false),
        serde_json::Value::String(s) => !s.is_empty() && s != "false",
        _ => true,
    }
}

/// Emit a legacy colon-channel command to the renderer.
/// The webview shim normalizes/relays these; event name stays un-normalized
/// ("pear-ipc-event" with the ORIGINAL channel name in the payload).
fn emit_to_renderer(app: &AppHandle, channel: &str, args: serde_json::Value) {
    let payload = json!({ "channel": channel, "args": args });
    if let Err(e) = app.emit_to(MAIN_WINDOW, "pear-ipc-event", payload) {
        warn!("[tray] failed to emit channel {channel}: {e}");
    }
}

/// Legacy createWindow(): show + unminimize + focus, and restore the dock.
pub async fn show_window(app: &AppHandle) {
    if let Some(win) = app.get_webview_window(MAIN_WINDOW) {
        let _ = win.show();
        let _ = win.unminimize();
        let _ = win.set_focus();
    } else {
        warn!("[tray] window '{MAIN_WINDOW}' not found in show_window");
    }
    #[cfg(target_os = "macos")]
    {
        // Legacy: app.dock.show() when window is re-shown.
        if let Err(e) = app.set_dock_visibility(true) {
            warn!("[tray] failed to show dock: {e}");
        }
    }
}

/// Legacy: tray click toggles window visibility (close-to-tray semantics).
/// Play/pause is driven by the tray menu item, not by the raw click.
fn toggle_play_and_window(app: &AppHandle) {
    let Some(win) = app.get_webview_window(MAIN_WINDOW) else {
        warn!("[tray] window '{MAIN_WINDOW}' not found on tray click");
        return;
    };
    if win.is_visible().unwrap_or(false) {
        let _ = win.hide();
        #[cfg(target_os = "macos")]
        if let Err(e) = app.set_dock_visibility(false) {
            warn!("[tray] failed to hide dock: {e}");
        }
        info!("[tray] window hidden (tray click)");
    } else {
        // show_window is async; spawn the best-effort sequence on the runtime.
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            show_window(&app).await;
            info!("[tray] window shown (tray click)");
        });
    }
}

/// Legacy electron_tray_legacy.ts: createTray()
pub fn setup_tray(app: &AppHandle) -> tauri::Result<()> {
    if !tray_config_enabled(app) {
        info!("[tray] options.tray falsy — tray disabled");
        return Ok(());
    }

    // Icon selection: is_paused None -> default; Some(true) -> paused; Some(false) -> default.
    let is_paused = app
        .try_state::<SharedTray>()
        .and_then(|s| {
            let Ok(guard) = s.lock() else {
                warn!("[tray] tray state poisoned; using default icon");
                return None;
            };
            Some(guard.is_paused)
        })
        .unwrap_or_default();
    let paused_icon = matches!(is_paused, Some(true));
    let icon = load_tray_icon(paused_icon)?;

    // Menu template (legacy createTray menu).
    let play_pause = MenuItem::with_id(app, "tray-play-pause", "Play/Pause", true, None::<&str>)?;
    let next = MenuItem::with_id(app, "tray-next", "Next", true, None::<&str>)?;
    let previous = MenuItem::with_id(app, "tray-previous", "Previous", true, None::<&str>)?;
    let show = MenuItem::with_id(app, "tray-show", "Show Windharp", true, None::<&str>)?;
    let restart = MenuItem::with_id(app, "tray-restart", "Restart", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "tray-quit", "Quit", true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;

    let menu = MenuBuilder::new(app)
        .item(&play_pause)
        .item(&next)
        .item(&previous)
        .item(&show)
        .item(&separator)
        .item(&restart)
        .item(&separator)
        .item(&quit)
        .build()?;

    let builder = TrayIconBuilder::with_id(TRAY_ID)
        .icon(icon)
        .tooltip(TOOLTIP)
        .menu(&menu)
        // tauri 2.12: `menu_on_left_click` is the DEPRECATED name (vendored
        // tray/mod.rs:307); `show_menu_on_left_click` (:319) is the current
        // setter. Called unconditionally — behavior is macOS/Windows only,
        // and an expression-position #[cfg(...)] on the call itself would
        // not compile. Keeps left-click free for the click handler (legacy
        // `setIgnoreDoubleClickEvents` + click-to-toggle behavior).
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().0.as_str() {
            "tray-play-pause" => {
                // legacy: playPause() -> media key toggle
                emit_to_renderer(app, "peard:toggle-play", json!([]));
            }
            "tray-next" => {
                // legacy: nextVideo() — renderer.ts listens PREFIXED
                // ('peard:next-video'), so emit the prefixed channel.
                emit_to_renderer(app, "peard:next-video", json!([]));
            }
            "tray-previous" => {
                // legacy: previousVideo() — renderer listens PREFIXED
                // ('peard:previous-video').
                emit_to_renderer(app, "peard:previous-video", json!([]));
            }
            "tray-show" => {
                // legacy: showWindow()
                let app = app.clone();
                tauri::async_runtime::spawn(async move {
                    show_window(&app).await;
                });
            }
            "tray-restart" => {
                // legacy: app.relaunch(); app.exit(0)
                info!("[tray] restart requested");
                app.restart();
            }
            "tray-quit" => {
                // legacy: quit()
                info!("[tray] quit requested");
                app.exit(0);
            }
            other => {
                // Unknown menu id — ignore (defensive; should not happen).
                let _ = other;
            }
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                let app = tray.app_handle().clone();
                toggle_play_and_window(&app);
            }
            // TrayIconEvent::DoubleClick is ignored (legacy
            // setIgnoreDoubleClickEvents semantics).
        });

    builder.build(app)?;
    info!("[tray] tray created (options.tray enabled, paused_icon={paused_icon})");
    Ok(())
}

/// Legacy electron_tray_legacy.ts: updateTrayMenu() — swap icon on pause state.
/// Port limitation: tooltip-with-song-info variant is skipped; tooltip static.
pub fn update_tray(app: &AppHandle, is_paused: Option<bool>) -> tauri::Result<()> {
    // Update shared state.
    if let Some(state) = app.try_state::<SharedTray>() {
        if let Ok(mut guard) = state.lock() {
            guard.is_paused = is_paused;
        } else {
            warn!("[tray] tray state poisoned in update_tray");
        }
    }

    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        // Tray may legitimately not exist when options.tray is falsy.
        return Ok(());
    };

    let paused = matches!(is_paused, Some(true));
    // Legacy only swapped icons when the paused state actually changed; a
    // stray update with the same state is a cheap no-op either way.
    let icon = load_tray_icon(paused)?;
    tray.set_icon(Some(icon))?;
    Ok(())
}

/// Legacy: window 'close' handler — close to tray when enabled.
/// Returns `true` when the close was intercepted (window hidden to tray),
/// `false` when the close should proceed.
/// `force_quit_flag` is a static AtomicBool owned by lib.rs (set in its
/// before-quit handler); lib.rs clones the AppHandle into the window-close
/// event handler when calling this.
pub fn handle_close(app: &AppHandle, force_quit_flag: &AtomicBool) -> bool {
    if !tray_config_enabled(app) {
        // No tray: let the window close (legacy: app quits).
        return false;
    }
    if force_quit_flag.load(Ordering::SeqCst) {
        // Quitting for real (quit menu / before-quit) — let the close proceed.
        return false;
    }
    // Prevent close + hide window (tray mode). Dock stays visible: the F-11
    // dock-hide is REMOVED — hiding the dock icon made macOS treat the app
    // as LSUIElement, so clicking the Dock icon no longer re-showed the
    // window (no Reopen re-parent); lib.rs RunEvent::Reopen handler restores
    // visibility instead WITHOUT deactivating the dock icon.
    if let Some(win) = app.get_webview_window(MAIN_WINDOW) {
        let _ = win.hide();
    } else {
        warn!("[tray] window '{MAIN_WINDOW}' not found in handle_close");
    }
    info!("[tray] close-to-tray: window hidden instead of quitting");
    true
}
