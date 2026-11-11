// config.rs — persistent JSON config store, port of src/config/index.ts semantics for Tauri.
//
// Store file: <app_config_dir>/config.json (see tauri.conf.json identifier).
// Seeding: `__defaults__` key is seeded from dist/plugins-manifest.json `__defaults__`
// when missing. A file that fails to parse is NEVER overwritten: we fall back to a
// seeded in-memory root and leave the user's file as-is on disk.
//
// Event contract: after every write (set / set_partial / menu options / programmatic
// menu effects) the FULL current store is persisted and emitted to the webview as a
// Tauri event `pear-config-changed` with payload {"snapshot": {...}} — the pear-shim
// mirrors it into its storeConfig representation and runs registered config watchers.

use std::{fs, path::PathBuf, sync::Mutex};

use serde_json::{Map, Value};
use tauri::{AppHandle, Emitter, Manager};

/// Embedded plugin manifest written by vite-plugins/manifest-generator.mts.
pub const MANIFEST_JSON: &str = include_str!("../../dist/plugins-manifest.json");

pub type SharedStore = Mutex<PearStore>;

// ---------------------------------------------------------------------------
// deepmerge — mirrors the TS deepmerge used by src/config/index.ts with
// mergeArrays:false: plain objects merge recursively; every other combination
// (arrays, scalars, type mismatch) takes the source value.
// ---------------------------------------------------------------------------
pub fn deepmerge(target: &Value, source: &Value) -> Value {
    match (target, source) {
        (Value::Object(t), Value::Object(s)) => {
            let mut out = t.clone();
            for (k, sv) in s {
                let merged = match out.get(k) {
                    Some(tv) => deepmerge(tv, sv),
                    None => sv.clone(),
                };
                out.insert(k.clone(), merged);
            }
            Value::Object(out)
        }
        // mirrors TS deepmerge: unless BOTH sides are plain objects the
        // source wins (mergeArrays:false behavior for arrays/scalars/nulls)
        (_, s) => s.clone(),
    }
}

pub fn obj_key(value: &Value, key: &str) -> Value {
    match value {
        Value::Object(map) => map.get(key).cloned().unwrap_or(Value::Null),
        _ => Value::Null,
    }
}

pub struct PearStore {
    path: PathBuf,
    root: Map<String, Value>,
}

impl PearStore {
    /// Load (or seed) the config file. Never fails for a readable config dir:
    /// any on-disk damage falls back to a seeded in-memory root without
    /// touching the damaged file.
    pub fn load(app: &AppHandle) -> Self {
        let dir = match app.path().app_config_dir() {
            Ok(d) => d,
            Err(_) => PathBuf::from("./"),
        };
        let path = dir.join("config.json");

        let seeded_root = |map: Map<String, Value>| {
            let mut map = map;
            if !map.contains_key("__defaults__") {
                if let Ok(manifest) = serde_json::from_str::<Value>(MANIFEST_JSON) {
                    map.insert("__defaults__".into(), obj_key(&manifest, "__defaults__"));
                }
            }
            map
        };

        let root: Map<String, Value> = match fs::read_to_string(&path) {
            Err(_) => {
                let fresh = seeded_root(Map::new());
                let _ = fs::create_dir_all(&dir);
                let _ = fs::write(&path, serde_json::to_string_pretty(&Value::Object(fresh.clone())).unwrap_or_default());
                fresh
            }
            Ok(raw) => match serde_json::from_str::<Value>(&raw) {
                Ok(Value::Object(map)) => seeded_root(map),
                Ok(_) => seeded_root(Map::new()), // valid JSON but not an object
                Err(_) => {
                    log::warn!("config.json is unreadable; using seeded defaults and leaving the file untouched");
                    seeded_root(Map::new())
                }
            },
        };

        log::info!(
            "config store loaded from {} ({} top-level keys)",
            path.display(),
            root.len()
        );

        Self { path, root }
    }

    // -- reads ---------------------------------------------------------------

    /// Mirrors store.get(key): dot-separated path; missing -> Null.
    pub fn get(&self, key: &str) -> Value {
        let mut current = Value::Object(self.root.clone());
        for part in key.split('.') {
            if part.is_empty() {
                continue;
            }
            current = obj_key(&current, part);
        }
        current
    }

    /// Convenience reads used across modules.
    pub fn get_bool(&self, key: &str) -> Option<bool> {
        self.get(key).as_bool()
    }

    pub fn get_str(&self, key: &str) -> Option<String> {
        self.get(key).as_str().map(|s| s.to_string())
    }

    /// Full clone of the root (snapshot for events).
    pub fn snapshot(&self) -> Value {
        Value::Object(self.root.clone())
    }

    pub fn path(&self) -> &PathBuf {
        &self.path
    }

    // -- writes --------------------------------------------------------------

    /// Mirrors store.set(key, value). Inserts intermediate objects as needed.
    /// Does NOT persist or emit — use `apply_config_change` for that in
    /// command paths, so every mutation funnels through one place.
    pub fn insert(&mut self, key: &str, value: Value) {
        let parts: Vec<&str> = key.split('.').filter(|p| !p.is_empty()).collect();
        if parts.is_empty() {
            if let Value::Object(map) = value {
                self.root = map;
            }
            return;
        }
        let mut current = &mut self.root;
        for part in &parts[..parts.len() - 1] {
            let entry = current
                .entry(part.to_string())
                .or_insert_with(|| Value::Object(Map::new()));
            if !entry.is_object() {
                *entry = Value::Object(Map::new());
            }
            current = entry.as_object_mut().expect("entry is an object");
        }
        current.insert(parts[parts.len() - 1].to_string(), value);
    }

    /// Persist the current root to disk (pretty JSON). Best-effort: a failed
    /// write logs but does not panic the app.
    pub fn persist(&self) {
        if let Ok(json) = serde_json::to_string_pretty(&Value::Object(self.root.clone())) {
            if let Err(err) = fs::write(&self.path, json + "\n") {
                log::error!("failed to persist config: {}", err);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Single mutation funnel: write -> persist -> emit snapshot (+ optional restart).
//
// `restart_if_configured` mirrors setMenuOption semantics from
// src/config/index.ts: apply, then if options.restartOnConfigChanges is
// truthy, restart the whole app process.
// ---------------------------------------------------------------------------
pub fn apply_config_change(app: &AppHandle, store: &mut PearStore, key: &str, value: Value, restart_if_configured: bool) {
    // F-5 — legacy config watch (electron_main_index_legacy.ts:209-243): when
    // a plugin config ROOT (`plugins.<id>`, single segment only) changes,
    // compare raw-OLD stored `enabled` (Option<bool>; None == legacy
    // undefined) against merged-NEW (deepmerge(stub config ?? {enabled:false},
    // new value)); on inequality broadcast `plugin:enable` / `plugin:unload`.
    // This is the ONLY funnel through which every write path passes
    // (peard:set-config, peard:store-set*, setMenuOption, native TogglePlugin).
    let plugin_root: Option<(String, Value)> = match key.strip_prefix("plugins.") {
        Some(rest) if !rest.is_empty() && !rest.contains('.') => {
            Some((rest.to_string(), store.get(key)))
        }
        _ => None,
    };
    // F-5a — bare 'plugins' key: legacy's watcher diffed the whole map
    // entry-by-entry (electron_main_index_legacy.ts:209-243). When the key is
    // literally 'plugins', snapshot EVERY plugin root before the write so the
    // per-id comparison below can run for each id present in old or new.
    let bare_plugins_old: Option<Map<String, Value>> = if key == "plugins" {
        store.get("plugins").as_object().cloned()
    } else {
        None
    };

    store.insert(key, value.clone());
    store.persist();
    let snapshot = store.snapshot();
    if let Err(err) = app.emit("pear-config-changed", serde_json::json!({ "snapshot": snapshot })) {
        log::error!("failed to emit pear-config-changed: {}", err);
    }

    if let Some((plugin_id, old_raw)) = plugin_root {
        let old_enabled = old_raw.get("enabled").and_then(Value::as_bool);
        let defaults = crate::ipc::manifest_plugin_config(&plugin_id);
        let defaults = if defaults.is_object() {
            defaults
        } else {
            serde_json::json!({ "enabled": false })
        };
        let merged_new = deepmerge(&defaults, &value);
        let new_enabled = merged_new.get("enabled").and_then(Value::as_bool);
        if new_enabled != old_enabled {
            // None (undefined) vs Some(false) still counts as a transition,
            // faithful to legacy `false !== undefined` → falls to unload.
            let channel = if new_enabled.unwrap_or(false) { "plugin:enable" } else { "plugin:unload" };
            crate::ipc::echo_ipc(app, channel, serde_json::json!([plugin_id]));
        }
    }

    // F-5a — bare 'plugins' map write: diff old vs new per id, mirroring the
    // legacy watcher (electron_main_index_legacy.ts:219-243) which iterates
    // ONLY the new map's entries (ids deleted from the map are never
    // visited). Missing/None roots behave as `undefined`, so a None vs
    // Some(_) comparison still transitions like legacy.
    if let Some(old_map) = bare_plugins_old {
        if let Some(new_obj) = store.get("plugins").as_object() {
            for (plugin_id, new_raw) in new_obj {
                // legacy: `deepmerge(stub ?? {enabled:false}, newPluginConfig ?? {})`
                // — a null entry becomes {} (?? fallback); always merge objects.
                let new_source = if new_raw.is_object() {
                    new_raw.clone()
                } else {
                    Value::Object(Map::new())
                };
                let old_enabled = old_map
                    .get(plugin_id)
                    .and_then(|v| v.get("enabled"))
                    .and_then(Value::as_bool);
                let defaults = crate::ipc::manifest_plugin_config(plugin_id);
                let defaults = if defaults.is_object() {
                    defaults
                } else {
                    serde_json::json!({ "enabled": false })
                };
                let merged_new = deepmerge(&defaults, &new_source);
                let new_enabled = merged_new.get("enabled").and_then(Value::as_bool);
                if new_enabled != old_enabled {
                    let channel = if new_enabled.unwrap_or(false) { "plugin:enable" } else { "plugin:unload" };
                    crate::ipc::echo_ipc(app, channel, serde_json::json!([plugin_id]));
                }
            }
        }
    }

    if restart_if_configured && store.get_bool("options.restartOnConfigChanges").unwrap_or(false) {
        log::info!("config change requires restart (restartOnConfigChanges)");
        app.restart();
    }
}

// ---------------------------------------------------------------------------
// Tauri commands (invoked by assets/pear-shim.js via the normalized names).
// Shim contract:
//   configApi.set(key, value)                    -> 'peard:store-set' {key, value}
//   configApi.setPartial(key, value, default?)  -> 'peard:store-set-partial'
//   configApi.setMenuOption(key, value)         -> 'peard:set-menu-option'
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn peard_store_set(app: AppHandle, state: tauri::State<'_, SharedStore>, key: String, value: Value) -> Result<(), String> {
    {
        let mut store = state.lock().unwrap_or_else(|e| e.into_inner());
        apply_config_change(&app, &mut store, &key, value, false);
    } // guard MUST be dropped before menu helpers: reattach_native_menu
      // rebuilds the menu model and locks SharedStore again (std Mutex is
      // NOT re-entrant → deadlock if held; F-7).
    crate::menu::reattach_native_menu(&app);
    crate::menu::emit_refresh_in_app_menu(&app);
    Ok(())
}

#[tauri::command]
pub fn peard_store_set_partial(
    app: AppHandle,
    state: tauri::State<'_, SharedStore>,
    key: String,
    value: Value,
    default_value: Option<Value>,
) -> Result<(), String> {
    // const newValue = deepmerge(defaultValue ?? {}, store.get(key) ?? {}, value)
    {
        let mut store = state.lock().unwrap_or_else(|e| e.into_inner());
        let base = deepmerge(default_value.as_ref().unwrap_or(&Value::Object(Map::new())), &store.get(key.as_str()));
        let merged = deepmerge(&base, &value);
        apply_config_change(&app, &mut store, &key, merged, false);
    } // guard dropped before menu helpers (F-7, see peard_store_set).
    crate::menu::reattach_native_menu(&app);
    crate::menu::emit_refresh_in_app_menu(&app);
    Ok(())
}

#[tauri::command]
pub fn peard_set_menu_option(app: AppHandle, state: tauri::State<'_, SharedStore>, key: String, value: Value) -> Result<(), String> {
    {
        let mut store = state.lock().unwrap_or_else(|e| e.into_inner());
        apply_config_change(&app, &mut store, &key, value, true);
    } // guard dropped before menu helpers (F-7, see peard_store_set).
    crate::menu::reattach_native_menu(&app);
    crate::menu::emit_refresh_in_app_menu(&app);
    Ok(())
}
