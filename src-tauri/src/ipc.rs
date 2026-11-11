//! IPC bridge for the Windharp (Pear) Tauri 2 port.
//!
//! Ports the legacy Electron main-process IPC surface (src/interfaces/*
//! and the ipcMain handlers in src/main.ts) onto three Tauri commands:
//!
//! - `peard_emit`       — one-way `ipcRenderer.send` messages; echoed back to
//!                        the webview on the single `"pear-ipc-event"` event so
//!                        in-page `.on` listeners fire (all renderer listeners
//!                        are registered in-page; the echo is what triggers them).
//! - `peard_subscribe` — shim-compat no-op registration.
//! - `peard_ipc_invoke`— resolver for legacy `ipcRenderer.invoke` channels
//!                        (get-config, set-config, paths, in-app-menu reads…).
//!
//! All Rust->webview notifications go through ONE Tauri event
//! `"pear-ipc-event"` with payload `{"channel": <legacy channel>, "args": [...]}`,
//! where `channel` is the LEGACY (non-normalized) channel name.

use serde_json::{json, Map, Value};
use tauri::{Emitter, Manager, State};

use crate::config::{apply_config_change, PearStore, SharedStore};

/// Menu JSON accessors + commandId executor implemented in `menu.rs`
/// (lib.rs re-exports `mod menu`).
use crate::menu::{execute_by_command_id, get_menu_item_json, get_menu_json};

/// Emit a legacy IPC message back to the webview on the single bridge event.
///
/// Used by `peard_emit` and by sibling modules (config/menu/tray) that need to
/// push legacy-channel notifications (e.g. `"config-changed"`) to the renderer.
pub(crate) fn echo_ipc(app: &tauri::AppHandle, channel: &str, args: Value) {
    if let Err(e) = app.emit("pear-ipc-event", json!({ "channel": channel, "args": args })) {
        log::warn!("pear-ipc-event emit failed on channel '{}': {}", channel, e);
    }
}

/// `ipcRenderer.send(channel, ...args)` shim target.
///
/// Legacy main process mostly consumed a few control channels itself; in the
/// Tauri port every listener lives in the webview, so the only correct
/// behavior is to echo the message back on `"pear-ipc-event"` and let the
/// shim fan it out to `ipcRenderer.on` listeners. The shim already decided on
/// the subscription side (it registered via `peard_subscribe`); no filtering
/// happens here.
#[tauri::command]
pub fn peard_emit(
    app: tauri::AppHandle,
    channel: String,
    args: Option<Vec<serde_json::Value>>,
) -> Result<(), String> {
    let args = args.unwrap_or_default();
    log::debug!("peard_emit: channel='{}'", channel);

    // Ported legacy main-process consumers: these channels need REAL work in
    // Rust (song-info cache/config side effects), not just the echo below.
    match channel.as_str() {
        // Legacy app-controls reload listener: re-navigate to the startup url.
        "peard:reload" => crate::do_reload(&app),

        // Song-info provider: rebuild cached SongInfo + persist canonical url.
        // Args: [playerResponse object]. In-page listeners
        // (sponsorblock, downloader) are driven by the echo below.
        "peard:video-src-changed" => {
            if let Some(player_response) = args.first().cloned() {
                let store_state = app.state::<SharedStore>();
                let song_state = app.state::<crate::song_info::SharedSong>();
                if let Err(e) = crate::song_info::peard_video_src_changed(
                    app.clone(),
                    store_state,
                    song_state,
                    player_response,
                ) {
                    log::warn!("peard_emit: video-src-changed handler failed: {e}");
                }
            }
        }

        // Song-info provider: payload is ONE object `{ isPaused, elapsedSeconds }`
        // (NOT positional). Extract, then the merged SongInfo is re-emitted by
        // song_info::peard_play_or_paused itself (legacy parity).
        "peard:play-or-paused" => {
            let is_paused = args
                .first()
                .and_then(|o| o.get("isPaused"))
                .and_then(Value::as_bool);
            let elapsed = args
                .first()
                .and_then(|o| o.get("elapsedSeconds"))
                .and_then(Value::as_f64);
            if is_paused.is_some() || elapsed.is_some() {
                let song_state = app.state::<crate::song_info::SharedSong>();
                if let Err(e) = crate::song_info::peard_play_or_paused(
                    app.clone(),
                    song_state,
                    app.state::<SharedStore>(),
                    is_paused,
                    elapsed,
                ) {
                    log::warn!("peard_emit: play-or-paused handler failed: {e}");
                }
            }
        }

        // Song-info provider: elapsed tick, cache-only. Args: [seconds].
        "peard:time-changed" => {
            let seconds = args.first().and_then(Value::as_f64);
            let song_state = app.state::<crate::song_info::SharedSong>();
            if let Err(e) =
                crate::song_info::peard_time_changed(app.clone(), song_state, seconds)
            {
                log::warn!("peard_emit: time-changed handler failed: {e}");
            }
        }

        // `peard:seeked` and everything else: echo-only (dormant/legacy).
        _ => {}
    }

    // Universal echo: all renderer `.on` listeners are driven by the
    // pear-ipc-event fan-out (see shim.js) — including the channels above,
    // EXCEPT the two song-info tick channels: `peard:play-or-paused` is
    // re-emitted by song_info::peard_play_or_paused itself with the merged
    // SongInfo payload (legacy parity), and `peard:time-changed` had NO
    // legacy broadcaster (cache-only) — echoing either would double-fire
    // renderer listeners (F-8).
    if channel != "peard:play-or-paused" && channel != "peard:time-changed" {
        echo_ipc(&app, &channel, Value::Array(args));
    }
    Ok(())
}

/// No-op registration for shim compatibility. Subscription echo-firing is
/// delivered entirely through `peard_emit`, so there is nothing to record.
#[tauri::command]
pub fn peard_subscribe(_channel: String) -> Result<(), String> {
    Ok(())
}

/// Deep-merge with legacy semantics (ports `deepmerge` usage in
/// src/config/utils.ts): when both sides are objects, merge recursively per
/// key (target-first); otherwise the source (second argument) wins.
/// `Value::Null` is not an object, so a Null default simply falls through to
/// the source value.
fn deepmerge(base: &Value, over: &Value) -> Value {
    match (base, over) {
        (Value::Object(b), Value::Object(o)) => {
            let mut merged: Map<String, Value> = b.clone();
            for (k, ov) in o {
                let merged_entry = match merged.get(k) {
                    Some(bv) => deepmerge(bv, ov),
                    None => ov.clone(),
                };
                merged.insert(k.clone(), merged_entry);
            }
            Value::Object(merged)
        }
        // Non-object (or missing/Null) base: source wins.
        _ => over.clone(),
    }
}

/// Legacy `getdefaults` lookup ports src/config/utils.ts `getdefaults`:
/// dist/plugins-manifest.json (PLUGIN_STUBS) is a flat plugin ARRAY
/// `[ { "id": "<id>", "restartNeeded": bool, "config": { ... } }, ... ]`
/// under top-level `plugins`, plus `__defaults__` for option-level defaults.
/// Returns the plugin entry's `config` object verbatim (missing entry or
/// missing `config` -> Null; callers apply the legacy `{enabled:false}`
/// fallback themselves, mirroring `` ?? { enabled: false }`` semantics).
pub(crate) fn manifest_plugin_config(plugin_id: &str) -> Value {
    let manifest: Value = match serde_json::from_str(super::PLUGIN_STUBS) {
        Ok(v) => v,
        Err(e) => {
            log::error!("failed to parse plugins manifest: {}", e);
            return Value::Null;
        }
    };
    manifest
        .get("plugins")
        .and_then(Value::as_array)
        .and_then(|plugins| {
            plugins
                .iter()
                .find(|entry| entry.get("id").and_then(Value::as_str) == Some(plugin_id))
        })
        .and_then(|entry| entry.get("config"))
        .cloned()
        .unwrap_or(Value::Null)
}

/// Poison-self-healing lock helper: a panicking previous holder must not
/// wedge the whole IPC surface. The returned guard borrows `state`.
fn lock_store<'a>(
    state: &'a State<'_, SharedStore>,
) -> std::sync::MutexGuard<'a, PearStore> {
    state.lock().unwrap_or_else(|e| e.into_inner())
}

/// `ipcRenderer.invoke(channel, ...args)` shim target — resolver for the
/// remaining legacy main-process invoked channels (src/main.ts ipcMain.handle
/// plus src/interfaces/* exposed APIs). Unknown channels resolve `null`
/// (legacy would reject) so renderer plugin promises keep flowing.
#[tauri::command]
pub fn peard_ipc_invoke(
    app: tauri::AppHandle,
    state: State<'_, SharedStore>,
    channel: String,
    args: Option<Vec<serde_json::Value>>,
) -> Result<serde_json::Value, String> {
    let args = args.unwrap_or_default();
    match channel.as_str() {
        // ---------------------------------------------------------------
        // peard:get-config — legacy `get-config(id)`:
        //   merged = deepmerge(getdefaults(`plugins.${id}`) ?? {enabled:false},
        //                       config.plugins.<id> ?? {})
        // (getdefaults reads the DEFAULTS store backed by src/config/defaults.ts).
        "peard:get-config" => {
            let id = args
                .first()
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_default();
            let defaults = manifest_plugin_config(&id);
            let base = if defaults.is_object() {
                defaults
            } else {
                json!({ "enabled": false })
            };
            // Read-lock only: clone the stored slice and drop the guard
            // before merging (no re-entrant lock risk).
            let stored = {
                let store = lock_store(&state);
                store.get(&format!("plugins.{}", id))
            };
            Ok(deepmerge(&base, &stored))
        }

        // ---------------------------------------------------------------
        // peard:set-config — legacy `set-config(id, obj)`:
        //   setPartial(`plugins.${id}`, obj) with NO defaultValue
        //   => deepmerge(deepmerge({}, stored), obj); setPartial never
        //      triggers restarts (restart_on_* only fires for setMenuOption).
        // Legacy then emitted per-plugin "config-changed" [pluginId, merged]
        // (the "pear-config-changed" {snapshot} half is emitted by
        // apply_config_change; neither is echoed as invoke response).
        "peard:set-config" => {
            let id = args
                .first()
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_default();
            let obj = args.get(1).cloned().unwrap_or(Value::Null);
            let key = format!("plugins.{}", id);

            // Short-lived write-lock: compute merged, run apply_config_change
            // (which itself takes &mut PearStore and emits pear-config-changed
            // through the AppHandle), then drop before we echo.
            let merged = {
                let mut store = lock_store(&state);
                let stored = store.get(&key);
                let m = deepmerge(&deepmerge(&json!({}), &stored), &obj);
                apply_config_change(&app, &mut store, &key, m.clone(), false);
                m
            }; // guard dropped here — do NOT hold across emit.

            echo_ipc(&app, "config-changed", json!([id, merged]));
            Ok(Value::Null)
        }

        // Handled by the shim from BOOT.pluginNames in the renderer; if it
        // still reaches Rust, resolve an empty list rather than erroring.
        "peard:get-main-plugin-names" => {
            log::warn!("peard:get-main-plugin-names reached Rust; shim should resolve it");
            Ok(json!([]))
        }

        // Legacy app.getPath('downloads') — OS downloads directory.
        "peard:get-downloads-folder" => Ok(Value::String(
            app.path()
                .download_dir()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default(),
        )),

        // Legacy app.getPath(name) mapping (electron -> tauri path api).
        "peard:get-path" => {
            let name = args.first().and_then(Value::as_str).unwrap_or_default();
            let path = match name {
                "downloads" => app.path().download_dir().ok(),
                "desktop" => app.path().desktop_dir().ok(),
                "documents" => app.path().document_dir().ok(),
                "userData" | "appData" => app.path().app_config_dir().ok(),
                "home" => app.path().home_dir().ok(),
                other => {
                    log::warn!("peard:get-path: unknown path name '{}'", other);
                    None
                }
            };
            Ok(match path {
                Some(p) => Value::String(p.to_string_lossy().to_string()),
                None => Value::Null,
            })
        }

        // In-app-menu reads (legacy src/main.ts getMenu/getMenuById
        // listeners, non-prefixed channels funneled here by the shim).
        // get_menu_json manages its own SharedStore locking — call it with
        // NO guard held.
        "get-menu" => Ok(get_menu_json(&app)),
        "get-menu-by-id" => {
            let command_id = args
                .first()
                .and_then(Value::as_u64)
                .unwrap_or(0) as u32;
            Ok(get_menu_item_json(&app, command_id).unwrap_or(Value::Null))
        }

        // PanelItem menu clicks: `invoke('peard:menu-event', commandId)`.
        // Args: [commandId:number] (shim keeps the channel verbatim).
        "peard:menu-event" => {
            let command_id = args
                .first()
                .and_then(Value::as_u64)
                .unwrap_or(0) as u32;
            execute_by_command_id(&app, command_id);
            Ok(Value::Null)
        }

        // ---------------------------------------------------------------
        // Window controls (legacy TitleBar src/titlebar/index.ts):
        // `invoke('window-is-maximized')` -> bool, and the four single-word
        // actions returning null.
        "window-is-maximized" => Ok(json!(
            app.get_webview_window("main")
                .map(|w| w.is_maximized().unwrap_or(false))
                .unwrap_or(false)
        )),
        "window-maximize" | "window-unmaximize" | "window-minimize" => {
            if let Some(win) = app.get_webview_window("main") {
                let result = match channel.as_str() {
                    "window-maximize" => win.maximize(),
                    "window-unmaximize" => win.unmaximize(),
                    _ => win.minimize(),
                };
                if let Err(e) = result {
                    log::warn!("peard_ipc_invoke: {} failed: {e}", channel);
                }
            }
            Ok(Value::Null)
        }
        "window-close" => {
            if let Some(win) = app.get_webview_window("main") {
                if let Err(e) = win.close() {
                    log::warn!("peard_ipc_invoke: window-close failed: {e}");
                }
            }
            Ok(Value::Null)
        }

        // Everything else: resolve null (legacy would reject the invoke, but
        // null-resolve keeps plugin promises flowing).
        other => {
            log::warn!("peard_ipc_invoke: unhandled channel '{}', resolving null", other);
            Ok(Value::Null)
        }
    }
}
