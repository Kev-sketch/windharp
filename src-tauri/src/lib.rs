// lib.rs — Pear desktop: the Electron main-process surface ported to Tauri 2 + Rust.
// Port of electron-main/{index.ts,menu.ts,tray.ts} + providers/song-info.ts.
//
// Modules:
//   config.rs    — persistent JSON store + pear-config-changed funnel (peard_store_set*)
//   ipc.rs       — peard_emit echo pipe + peard_ipc_invoke channel resolver
//   menu.rs      — menu model, commandId map, get-menu JSON, click-effect engine
//   song_info.rs — GetPlayerResponse parser -> peard:update-song-info
//   tray.rs      — tray icon, tooltip/menu, click-to-show/hide, close-to-tray

pub mod config;
pub mod ipc;
pub mod menu;
pub mod song_info;
pub mod tray;

use std::sync::atomic::{AtomicBool, Ordering};

use tauri::{Emitter, Manager};
use tauri_plugin_autostart::MacosLauncher;

use config::PearStore;

// Runtime assets baked in at compile time. dist/ is produced by
// `pnpm build:frontend` (vite + vite-plugins/manifest-generator.mts).
pub const RENDERER_BUNDLE: &str = include_str!("../../dist/index.js");
pub const PLUGIN_STUBS: &str = include_str!("../../dist/plugins-manifest.json");
pub const SHIM_JS: &str = include_str!("../../assets/pear-shim.js");

// Dev-mode renderer: when true the bundle is fetched from the vite dev
// server instead of the baked-in dist bundle (hot reload inside the real
// music.youtube.com page, mirroring the legacy ELECTRON_RENDERER_URL flow).
const DEV: bool = cfg!(debug_assertions);
const DEV_URL: &str = "http://localhost:5173";
const DEFAULT_URL: &str = "https://music.youtube.com";

// Legacy window chrome forced a Chrome UA on macOS so Google sign-in works
// in the embedded webview (defaults.ts now ships overrideUserAgent: true).
const CHROME_UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.6723.152 Safari/537.36";

// Set the moment the app begins quitting so the window CloseRequested
// handler lets the close pass instead of hiding to tray (legacy forceQuit).
static FORCE_QUIT: AtomicBool = AtomicBool::new(false);
/// Last-known maximized state for the Resized-driven maximize echo (F-1):
/// Tauri's WindowEvent has no Maximize/Unmaximize variants on macOS, so the
/// TitleBar `window-maximize`/`window-unmaximize` listeners are driven from
/// Resized transitions instead (deduped to fire only on state CHANGE).
static REPORTED_MAXIMIZED: AtomicBool = AtomicBool::new(false);

#[tauri::command]
fn peard_restart(app: tauri::AppHandle) {
    log::info!("peard:restart — restarting the app process");
    app.restart();
}

fn main_window(app: &tauri::AppHandle) -> Option<tauri::WebviewWindow> {
    app.get_webview_window("main")
}

/// peard:reload — re-navigate the window to the configured startup url.
pub fn do_reload(app: &tauri::AppHandle) {
    let url = {
        let store = app.state::<config::SharedStore>();
        let store = store.lock().unwrap_or_else(|e| e.into_inner());
        if store.get_bool("options.resumeOnStart").unwrap_or(false) {
            store.get_str("url").filter(|u| !u.is_empty()).unwrap_or_else(|| DEFAULT_URL.to_string())
        } else {
            DEFAULT_URL.to_string()
        }
    };
    if let Some(win) = main_window(app) {
        log::info!("peard:reload — navigating to {}", url);
        let js = format!("location.assign({:?})", url);
        let _ = win.eval(&js);
    }
}

fn build_bootstrap_script(store: &PearStore, plugin_names: &[String]) -> String {
    let snapshot = serde_json::to_string(&store.snapshot()).unwrap_or_else(|_| "{}".into());
    let names = serde_json::to_string(plugin_names).unwrap_or_else(|_| "[]".into());
    format!(
        "window.__PEAR_BOOTSTRAP__ = {{ dev: {}, devUrl: {:?}, config: {}, pluginNames: {} }};",
        DEV, DEV_URL, snapshot, names
    )
}

/// Seed window.__PEAR_PLUGIN_STUBS__ from the baked-in manifest so config
/// default lookups work before renderer.ts re-publishes the live stubs.
fn build_stubs_seed_script() -> String {
    format!(
        "window.__PEAR_PLUGIN_STUBS__ = (function (m) {{ var o = {{ __defaults__: (m && m.__defaults__) || {{}} }}; var list = (m && m.plugins) || []; for (var i = 0; i < list.length; i++) o[list[i].id] = list[i]; return o; }})({});",
        PLUGIN_STUBS
    )
}

/// The renderer bundle runs only in the top frame of youtube.com pages (and
/// localhost in dev) — mirroring the legacy preload/main-frame split.
fn build_renderer_script() -> String {
    if DEV {
        format!(
            "try {{ if (window.top === window.self) {{ var s = document.createElement('script'); s.src = {:?}; document.documentElement.appendChild(s); }} }} catch (e) {{ console.error('[pear] dev renderer inject failed', e); }}",
            format!("{}/src/renderer.ts", DEV_URL)
        )
    } else {
        format!(
            "try {{ var h = window.location.hostname; if (window.top === window.self && (/(^|\\.)youtube\\.com$/.test(h) || h === 'localhost')) {{\n{}\n}} }} catch (e) {{ console.error('[pear] renderer bootstrap failed', e); }}",
            RENDERER_BUNDLE
        )
    }
}

fn plugin_names_from_manifest() -> Vec<String> {
    serde_json::from_str::<serde_json::Value>(PLUGIN_STUBS)
        .ok()
        .and_then(|m| m.get("plugins").cloned())
        .and_then(|p| p.as_array().cloned())
        .map(|arr| {
            arr.iter()
                .filter_map(|e| e.get("id").and_then(|i| i.as_str()).map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

fn setup_window(app: &tauri::AppHandle, store: &PearStore, plugin_names: &[String]) -> tauri::Result<()> {
    let resume = store.get_bool("options.resumeOnStart").unwrap_or(false);
    let start_url = if resume {
        store.get_str("url").filter(|u| !u.is_empty()).unwrap_or_else(|| DEFAULT_URL.to_string())
    } else {
        DEFAULT_URL.to_string()
    };
    let always_on_top = store.get_bool("options.alwaysOnTop").unwrap_or(false);
    let override_ua = store.get_bool("options.overrideUserAgent").unwrap_or(true);

    let url = tauri::Url::parse(&start_url).unwrap_or_else(|_| tauri::Url::parse(DEFAULT_URL).unwrap());

    let mut builder = tauri::WebviewWindowBuilder::new(app, "main", tauri::WebviewUrl::External(url))
        .title("Windharp")
        .inner_size(1100.0, 550.0)
        .min_inner_size(325.0, 425.0)
        .resizable(true)
        .visible(true);

    // Order matters: the shim reads __PEAR_BOOTSTRAP__ synchronously at
    // install time, so the bootstrap object must exist before the shim.
    builder = builder
        .initialization_script(&build_bootstrap_script(store, plugin_names))
        .initialization_script(SHIM_JS)
        .initialization_script(&build_stubs_seed_script())
        .initialization_script(&build_renderer_script());

    if always_on_top {
        builder = builder.always_on_top(true);
    }

    // Google sign-in requires a Chrome user agent in the embedded webview;
    // forced on macOS per the port decision (default overrideUserAgent=true).
    if override_ua || cfg!(target_os = "macos") {
        builder = builder.user_agent(CHROME_UA);
    }

    // Build FIRST, then register the window-event handler on the built window
    // (vendored `WebviewWindow::on_window_event<F: Fn(&WindowEvent)>`); the
    // close-to-tray decision runs through the AppHandle snapshot.
    let win = builder.build()?;

    let close_handle = app.clone();
    let max_handle = app.clone();
    win.on_window_event(move |event| match event {
        tauri::WindowEvent::CloseRequested { api, .. } => {
            if tray::handle_close(&close_handle, &FORCE_QUIT) {
                api.prevent_close();
            }
        }
        // macOS maximize/restore flows through Resized (Tauri has no
        // Maximize/Unmaximize WindowEvent variants); the TitleBar listens on
        // the legacy `window-maximize`/`window-unmaximize` channels (args
        // ignored — it refetches state), so echo on state TRANSITION only.
        tauri::WindowEvent::Resized(_) => {
            let channel = if max_handle
                .get_webview_window("main")
                .map(|w| w.is_maximized().unwrap_or(false))
                .unwrap_or(REPORTED_MAXIMIZED.load(Ordering::SeqCst))
            {
                "window-maximize"
            } else {
                "window-unmaximize"
            };
            let now_max = channel == "window-maximize";
            if now_max != REPORTED_MAXIMIZED.load(Ordering::SeqCst) {
                REPORTED_MAXIMIZED.store(now_max, Ordering::SeqCst);
                if let Err(e) = max_handle.emit(
                    "pear-ipc-event",
                    serde_json::json!({ "channel": channel, "args": [] }),
                ) {
                    log::warn!("maximize transition emit failed: {e}");
                }
            }
        }
        _ => {}
    });

    log::info!("main window created ({})", start_url);
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // single-instance must be the first registered plugin.
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            if let Some(win) = app.get_webview_window("main") {
                let _ = win.show();
                let _ = win.unminimize();
                let _ = win.set_focus();
            }
        }))
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .on_menu_event(|app, event| menu::on_native_menu_event(app, &event))
        .invoke_handler(tauri::generate_handler![
            config::peard_store_set,
            config::peard_store_set_partial,
            config::peard_set_menu_option,
            ipc::peard_emit,
            ipc::peard_subscribe,
            ipc::peard_ipc_invoke,
            menu::get_menu,
            menu::get_menu_by_id,
            menu::peard_menu_event,
            song_info::peard_video_src_changed,
            song_info::peard_play_or_paused,
            song_info::peard_time_changed,
            peard_restart,
        ])
        .setup(|app| {
            if cfg!(debug_assertions) {
                app.handle()
                    .plugin(tauri_plugin_log::Builder::default().level(log::LevelFilter::Info).build())?;
            }

            let handle = app.handle().clone();

            // 1. Load + manage the config store before anything reads it.
            let store = PearStore::load(&handle);
            app.manage(config::SharedStore::new(store));
            app.manage(song_info::SharedSong::default());
            app.manage(tray::SharedTray::default());

            let plugin_names = plugin_names_from_manifest();

            // 2. Startup options (legacy main/index.ts) — hoist the config
            //    reads out of the lock scopes so no MutexGuard (borrowing
            //    `app` via State) is alive across the autostart / dock calls.
            let start_at_login = {
                let store_ref = app.state::<config::SharedStore>();
                let store_ref = store_ref.lock().unwrap_or_else(|e| e.into_inner());
                store_ref.get_bool("options.startAtLogin").unwrap_or(false)
            };
            if start_at_login {
                use tauri_plugin_autostart::ManagerExt;
                if let Err(err) = app.autolaunch().enable() {
                    log::warn!("failed to enable autostart: {}", err);
                }
            }
            #[cfg(target_os = "macos")]
            let app_visible = {
                let store_ref = app.state::<config::SharedStore>();
                let store_ref = store_ref.lock().unwrap_or_else(|e| e.into_inner());
                store_ref.get_bool("options.appVisible").unwrap_or(true)
            };
            #[cfg(target_os = "macos")]
            if !app_visible {
                let _ = app.set_dock_visibility(false);
            }

            // 3. Window (with the init-script chain: bootstrap -> shim -> stubs -> renderer).
            {
                let store_ref = app.state::<config::SharedStore>();
                let store_view = store_ref.lock().unwrap_or_else(|e| e.into_inner());
                setup_window(&handle, &store_view, &plugin_names)?;
            } // store guard dropped before menu/tray reload config

            // 4. Native menu (macOS menu bar mirror of the model).
            menu::attach_native_menu(&handle)?;

            // 5. Tray (only when options.tray is set).
            tray::setup_tray(&handle)?;

            // 6. Global shortcut: backtick toggles the in-app menu when focused.
            {
                use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};
                let gs = app.global_shortcut();
                let parsed = "`".parse::<Shortcut>().or_else(|_| "Backquote".parse::<Shortcut>());
                match parsed {
                    Ok(sc) => {
                        gs.on_shortcut(sc, move |app, _shortcut, event| {
                            if event.state == ShortcutState::Pressed {
                                if let Some(win) = main_window(app) {
                                    if win.is_focused().unwrap_or(false) {
                                        let _ = win.emit(
                                            "pear-ipc-event",
                                            serde_json::json!({"channel": "toggle-in-app-menu", "args": []}),
                                        );
                                    }
                                }
                            }
                        })?;
                        log::info!("registered backtick in-app-menu shortcut");
                    }
                    Err(_) => log::warn!("could not parse the backtick shortcut; in-app-menu toggle disabled"),
                }
            }

            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building the tauri application")
        .run(|app, event| {
            match event {
                tauri::RunEvent::ExitRequested { .. } => {
                    FORCE_QUIT.store(true, Ordering::SeqCst);
                }
                // macOS dock-icon click while all windows are hidden
                // (close-to-tray hides the window, not the app): legacy
                // activate-with-no-visible-windows re-showed the main window.
                #[cfg(target_os = "macos")]
                tauri::RunEvent::Reopen { has_visible_windows, .. } => {
                    if !has_visible_windows {
                        if let Some(win) = app.get_webview_window("main") {
                            let _ = win.show();
                            let _ = win.set_focus();
                        }
                    }
                }
                _ => {}
            }
        });
}
