//! menu.rs — port of the Electron `electron_menu_legacy.ts` menu model for the Tauri 2 app.
//!
//! Ports (electron_menu_legacy.ts functions cited where used):
//! - `regenerateMenu` / `buildMenuTemplate` -> [`build_menu_model`]
//! - `t-menu-item` click handlers (setMenuOption / setOption etc.) -> [`MenuEffect`] + [`exec_effect`]
//! - `pluginEnabledMenu` / per-plugin submenu templates -> simplified flat checkbox per plugin
//!   (electron plugin menu templates came from plugin backends; Tauri-port plugins are
//!   renderer-only stubs — documented limitation)
//! - electron `Menu.buildFromTemplate` + `Menu.setApplicationMenu` -> [`attach_native_menu`]
//!   (the native macOS menu bar is built and kept in sync; the in-app menu renderer
//!    TitleBar.tsx gets the same tree via `get_menu` / `get_menu_by_id`)
//! - in-app-menu plugin commands `get-menu` / `get-menu-by-id` -> [`get_menu`] / [`get_menu_by_id`]
//!
//! Documented limitations (agreed in the port contract):
//! - "Starting page" radios reduced to a fixed pair: "" (Default) / "music.youtube.com".
//! - Theme remove-confirmation dialog dropped; themes come from `options.themes` config only.
//! - Custom window title prompt menu item omitted.
//! - Advanced options dialog (electron-only `advancedOptions` window) reduced to checkboxes
//!   ("Always on top", "Disable hardware acceleration") plus an explicit "Restart app" item.
//! - Tauri 2 core has no RadioMenuItem (only CheckMenuItem), so radio items are native
//!   CheckMenuItems whose JSON `type` is still "radio" so TitleBar renders radio groups;
//!   `get_menu_by_id` rebuilds from config so radio siblings refresh checked states.
//! - Menu click effects that write config always emit `refresh-in-app-menu` afterwards so the
//!   in-app menu re-fetches `get-menu`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;

use serde_json::{json, Map, Value};
use tauri::{
    menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu},
    AppHandle, Manager, State, Wry,
};

use super::config::{apply_config_change, deepmerge, PearStore, SharedStore};

/// lib.rs exposes `pub const PLUGIN_STUBS: &str = include_str!("../../dist/plugins-manifest.json")`.
use super::PLUGIN_STUBS;

// ---------------------------------------------------------------------------
// Model (port of the electron_menu_legacy.ts template tree)
// ---------------------------------------------------------------------------

/// Monotonic commandId counter — electron's `commandId` on every clickable menu item.
static COMMAND_ID: AtomicU32 = AtomicU32::new(1);

/// Kind of predefined native item (electron roles: about, close, quit, cut, copy, ...).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PredefKind {
    About,
    CloseWindow,
    Quit,
    Cut,
    Copy,
    Paste,
    SelectAll,
    Hide,
    HideOthers,
    ShowAll,
    Minimize,
    Separator,
}

/// The item types the in-app menu JSON understands + predefined native items.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ItemType {
    Normal,
    Checkbox,
    Radio,
    Separator,
    Predefined(PredefKind),
}

/// What a click does — port of the electron `template[i].click` handlers
/// (setMenuOption / setOption / togglePluginEnabled / history.back / restart ...).
#[derive(Clone, Debug)]
pub enum MenuEffect {
    ToggleBool { key: String },
    SetRadioOption { key: String, value: String },
    TogglePlugin { plugin_id: String },
    NavBack,
    NavForward,
    Restart,
    ToggleAlwaysOnTop,
}

/// One menu item in the model tree.
#[derive(Clone, Debug)]
pub struct MenuItemModel {
    pub item_type: ItemType,
    pub label: String,
    pub checked: bool,
    pub command_id: Option<u32>,
    pub effect: Option<MenuEffect>,
    pub submenu: Vec<MenuItemModel>,
}

impl MenuItemModel {
    fn click_normal(label: &str, effect: MenuEffect) -> Self {
        Self {
            item_type: ItemType::Normal,
            label: label.to_string(),
            checked: false,
            command_id: Some(next_command_id()),
            effect: Some(effect),
            submenu: Vec::new(),
        }
    }
    fn checkbox(label: &str, checked: bool, effect: MenuEffect) -> Self {
        Self {
            item_type: ItemType::Checkbox,
            label: label.to_string(),
            checked,
            command_id: Some(next_command_id()),
            effect: Some(effect),
            submenu: Vec::new(),
        }
    }
    /// Radio with config `key` set to `value`; JSON type stays "radio".
    fn radio(label: &str, value: &str, key: &str, checked: bool) -> Self {
        Self {
            item_type: ItemType::Radio,
            label: label.to_string(),
            checked,
            command_id: Some(next_command_id()),
            effect: Some(MenuEffect::SetRadioOption { key: key.to_string(), value: value.to_string() }),
            submenu: Vec::new(),
        }
    }
    /// Grouping item (a parent submenu); carries a commandId so the native id is unique.
    fn submenu(label: &str, items: Vec<Self>) -> Self {
        Self {
            item_type: ItemType::Normal,
            label: label.to_string(),
            checked: false,
            command_id: Some(next_command_id()),
            effect: None,
            submenu: items,
        }
    }
    fn separator() -> Self {
        Self {
            item_type: ItemType::Separator,
            label: String::new(),
            checked: false,
            command_id: None,
            effect: None,
            submenu: Vec::new(),
        }
    }
    fn predefined(kind: PredefKind) -> Self {
        let label = match kind {
            PredefKind::About => "About",
            PredefKind::CloseWindow => "Close Window",
            PredefKind::Quit => "Quit Windharp",
            PredefKind::Cut => "Cut",
            PredefKind::Copy => "Copy",
            PredefKind::Paste => "Paste",
            PredefKind::SelectAll => "Select All",
            PredefKind::Hide => "Hide",
            PredefKind::HideOthers => "Hide Others",
            PredefKind::ShowAll => "Show All",
            PredefKind::Minimize => "Minimize",
            PredefKind::Separator => "",
        };
        Self {
            item_type: ItemType::Predefined(kind),
            label: label.to_string(),
            checked: false,
            command_id: None,
            effect: None,
            submenu: Vec::new(),
        }
    }
}

fn next_command_id() -> u32 {
    COMMAND_ID.fetch_add(1, Ordering::SeqCst)
}

/// Whole menu tree.
pub struct MenuTree {
    pub items: Vec<MenuItemModel>,
}

/// Managed state: after each rebuild (`get_menu` / `attach_native_menu`) the baked
/// model + commandId -> effect map ("commandsMap") are stored here so
/// `peard_menu_event` and [`on_native_menu_event`] resolve clicks without rebuilding.
pub struct MenuState {
    pub commands: Mutex<HashMap<u32, MenuEffect>>,
    pub model: Mutex<Vec<MenuItemModel>>,
}

impl Default for MenuState {
    fn default() -> Self {
        Self { commands: Mutex::new(HashMap::new()), model: Mutex::new(Vec::new()) }
    }
}

// ---------------------------------------------------------------------------
// Model building (port of regenerateMenu / buildMenuTemplate)
// ---------------------------------------------------------------------------

/// Parse the plugins manifest (`dist/plugins-manifest.json` via `super::PLUGIN_STUBS`)
/// into (id, label, restart_needed) triples; label falls back to id.
fn manifest_plugins() -> Vec<(String, String, bool)> {
    let mut out = Vec::new();
    if let Ok(v) = serde_json::from_str::<Value>(PLUGIN_STUBS) {
        // Manifest plugins may be a top-level array or { "plugins": [...] }.
        let arr = if v.is_array() { v } else { v.get("plugins").cloned().unwrap_or(Value::Null) };
        if let Some(list) = arr.as_array() {
            for p in list {
                let id = p.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
                if id.is_empty() {
                    continue;
                }
                let label = p
                    .get("label")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .unwrap_or_else(|| id.clone());
                let restart = p.get("restartNeeded").and_then(Value::as_bool).unwrap_or(false);
                out.push((id, label, restart));
            }
        }
    }
    out
}

/// Build the full menu model from the current config + plugin manifest.
/// Port of electron_menu_legacy.ts `regenerateMenu()`.
pub fn build_menu_model(store: &PearStore, manifest: &str) -> MenuTree {
    let _ = manifest; // only PLUGIN_STUBS is used today; reserved for per-plugin templates
    let mut items: Vec<MenuItemModel> = Vec::new();

    // --- macOS "Windharp" app menu (top-level first submenu) ---
    let start_at_login = store.get_bool("options.startAtLogin").unwrap_or(false);
    let app_menu = vec![
        MenuItemModel::predefined(PredefKind::About),
        MenuItemModel::separator(),
        // legacy: start-at-login reads/writes config `options.startAtLogin`; the actual
        // autostart plugin enablement is applied by lib.rs listening for the config change.
        MenuItemModel::checkbox(
            "Start at login",
            start_at_login,
            MenuEffect::ToggleBool { key: "options.startAtLogin".into() },
        ),
        MenuItemModel::separator(),
        MenuItemModel::predefined(PredefKind::CloseWindow),
        MenuItemModel::separator(),
        MenuItemModel::predefined(PredefKind::Quit),
    ];
    items.push(MenuItemModel::submenu("Windharp", app_menu));

    // --- Options submenu ---
    let starting_page = store.get_str("options.startingPage").unwrap_or_default();
    let like_buttons = store.get_str("options.likeButtons").unwrap_or_default();
    let remove_upgrade = store.get_bool("options.removeUpgradeButton").unwrap_or(false);
    let swap_like = store.get_bool("options.swapLikeButtonsOrder").unwrap_or(false);
    let theme_current = store.get_str("options.theme").unwrap_or_default();

    // "Starting page" radios — documented limitation: fixed pair. Values not in the
    // set are displayed as "Default" checked. The second entry is labelled with the
    // product name so no YouTube branding reaches the native menu.
    let starting_page_current = if starting_page == "music.youtube.com" { "music.youtube.com" } else { "" };
    let mut starting_page_items = Vec::new();
    for (value, label) in [("", "Default"), ("music.youtube.com", "Windharp")] {
        let checked = value == starting_page_current;
        starting_page_items.push(MenuItemModel::radio(label, value, "options.startingPage", checked));
    }

    // "Like buttons" radios: '' (Default), 'force' (Force show), 'hide' (Hide).
    let mut like_buttons_items = Vec::new();
    for (value, label) in [("", "Default"), ("force", "Force show"), ("hide", "Hide")] {
        let checked = like_buttons == value;
        like_buttons_items.push(MenuItemModel::radio(label, value, "options.likeButtons", checked));
    }
    like_buttons_items.push(MenuItemModel::checkbox(
        "Swap like buttons order",
        swap_like,
        MenuEffect::ToggleBool { key: "options.swapLikeButtonsOrder".into() },
    ));

    // "Theme" submenu: "" (Default) + every id in the `options.themes` config array.
    // Documented limitation: legacy built-in themes list dropped — config is the source.
    let mut theme_items = Vec::new();
    let mut theme_ids: Vec<String> = vec![String::new()];
    if let Some(arr) = store.get("options.themes").as_array() {
        for t in arr {
            if let Some(id) = t.as_str() {
                theme_ids.push(id.to_string());
            }
        }
    }
    for id in &theme_ids {
        let label = if id.is_empty() { "Default".to_string() } else { id.clone() };
        let checked = theme_current == *id;
        theme_items.push(MenuItemModel::radio(&label, id, "options.theme", checked));
    }

    let visual_tweaks = vec![
        MenuItemModel::checkbox(
            "Remove upgrade button",
            remove_upgrade,
            MenuEffect::ToggleBool { key: "options.removeUpgradeButton".into() },
        ),
        MenuItemModel::submenu("Like buttons", like_buttons_items),
        MenuItemModel::submenu("Theme", theme_items),
    ];

    let options_menu = vec![
        MenuItemModel::checkbox(
            "Auto update",
            store.get_bool("options.autoUpdates").unwrap_or(true),
            MenuEffect::ToggleBool { key: "options.autoUpdates".into() },
        ),
        MenuItemModel::checkbox(
            "Resume on start",
            store.get_bool("options.resumeOnStart").unwrap_or(true),
            MenuEffect::ToggleBool { key: "options.resumeOnStart".into() },
        ),
        MenuItemModel::submenu("Starting page", starting_page_items),
        MenuItemModel::submenu("Visual tweaks", visual_tweaks),
    ];
    items.push(MenuItemModel::submenu("Options", options_menu));

    // --- Plugins submenu ---
    // Port of pluginEnabledMenu — simplified: one flat checkbox per plugin (electron
    // plugin-defined submenu templates came from plugin backends that do not exist here).
    let mut plugins_menu = Vec::new();
    for (id, label, _restart) in manifest_plugins() {
        // Checkbox state = legacy merged view: deepmerge(stub config ?? {enabled:false},
        // stored plugins.<id> ?? {}) then .enabled ?? false. Reading the raw
        // plugins.<id>.enabled key alone misses manifest-defaulted plugins and
        // treats "missing" as enabled (F-6).
        let defaults = crate::ipc::manifest_plugin_config(&id);
        let defaults = if defaults.is_object() {
            defaults
        } else {
            json!({ "enabled": false })
        };
        let stored = store.get(&format!("plugins.{id}"));
        let merged = deepmerge(&defaults, &stored);
        let enabled = merged.get("enabled").and_then(Value::as_bool).unwrap_or(false);
        plugins_menu.push(MenuItemModel::checkbox(&label, enabled, MenuEffect::TogglePlugin { plugin_id: id }));
    }
    if plugins_menu.is_empty() {
        plugins_menu.push(MenuItemModel::click_normal("No plugins", MenuEffect::ToggleBool {
            key: "__no_plugins__".into(), // never toggles meaningfully; shown disabled natively
        }));
    }
    items.push(MenuItemModel::submenu("Plugins", plugins_menu));

    // --- Advanced options submenu ---
    // Electron-only advancedOptions dialog reduced to checkboxes + explicit restart.
    let advanced_menu = vec![
        MenuItemModel::checkbox(
            "Always on top",
            store.get_bool("options.alwaysOnTop").unwrap_or(false),
            MenuEffect::ToggleAlwaysOnTop,
        ),
        MenuItemModel::checkbox(
            "Disable hardware acceleration",
            store.get_bool("options.disableHardwareAcceleration").unwrap_or(false),
            MenuEffect::ToggleBool { key: "options.disableHardwareAcceleration".into() },
        ),
        MenuItemModel::separator(),
        MenuItemModel::click_normal("Restart app", MenuEffect::Restart),
    ];
    items.push(MenuItemModel::submenu("Advanced options", advanced_menu));

    // --- View / Edit menu (macOS predefined roles) ---
    items.push(MenuItemModel::submenu(
        "Edit",
        vec![
            MenuItemModel::predefined(PredefKind::Cut),
            MenuItemModel::predefined(PredefKind::Copy),
            MenuItemModel::predefined(PredefKind::Paste),
            MenuItemModel::predefined(PredefKind::SelectAll),
        ],
    ));

    // --- Navigation submenu ---
    items.push(MenuItemModel::submenu(
        "Navigation",
        vec![
            MenuItemModel::click_normal("Back", MenuEffect::NavBack),
            MenuItemModel::click_normal("Forward", MenuEffect::NavForward),
        ],
    ));

    MenuTree { items }
}

/// Walk the model, baking commandId -> effect (the "commandsMap").
fn bake_commands(items: &[MenuItemModel], map: &mut HashMap<u32, MenuEffect>) {
    for item in items {
        if let (Some(id), Some(effect)) = (item.command_id, item.effect.clone()) {
            map.insert(id, effect);
        }
        bake_commands(&item.submenu, map);
    }
}

/// Rebuild the model from the current config, bake it into `MenuState` (fields
/// + commands map), and return the fresh `Vec<MenuItemModel>`. The commandId
/// counter is RESET at the start of each rebuild so ids are deterministic for
/// the same config — the native menu's "menu-<commandId>" ids therefore remain
/// resolvable in later bakes.
fn rebuild(app: &AppHandle, state: &MenuState) -> Vec<MenuItemModel> {
    COMMAND_ID.store(1, Ordering::SeqCst);
    let model = {
        let shared = app.state::<SharedStore>();
        let guard = shared.lock().unwrap_or_else(|e| e.into_inner());
        build_menu_model(&guard, PLUGIN_STUBS).items
    };
    let mut map = HashMap::new();
    bake_commands(&model, &mut map);
    {
        let mut model_guard = state.model.lock().unwrap_or_else(|e| e.into_inner());
        *model_guard = model.clone();
        let mut cmd_guard = state.commands.lock().unwrap_or_else(|e| e.into_inner());
        *cmd_guard = map;
    }
    model
}

// ---------------------------------------------------------------------------
// JSON serialization for the in-app TitleBar menu
// ---------------------------------------------------------------------------

/// Interface MenuItemJson per the in-app-menu plugin contract:
/// { checked, commandId?, label, submenu?: { items: [...] }, type }.
/// Separators serialize as type "normal" with label "" so the renderer draws a
/// divider. `submenu` containers serialize as type "normal" with a submenu
/// object. Legacy 'commandsMap' / 'menu' keys are simply never emitted.
fn item_to_json(item: &MenuItemModel) -> Value {
    let type_str = match item.item_type {
        ItemType::Normal | ItemType::Separator | ItemType::Predefined(_) => "normal",
        ItemType::Checkbox => "checkbox",
        ItemType::Radio => "radio",
    };
    let mut obj = Map::new();
    obj.insert("checked".into(), Value::Bool(item.checked));
    if let Some(id) = item.command_id {
        obj.insert("commandId".into(), json!(id));
    }
    obj.insert("label".into(), Value::String(item.label.clone()));
    if !item.submenu.is_empty() {
        let items: Vec<Value> = item.submenu.iter().map(item_to_json).collect();
        let mut sub = Map::new();
        sub.insert("items".into(), Value::Array(items));
        obj.insert("submenu".into(), Value::Object(sub));
    }
    obj.insert("type".into(), Value::String(type_str.to_string()));
    Value::Object(obj)
}

/// The get-menu payload wraps top-level items in { "menu": { "items": [...] } }.
fn model_to_menu_json(items: &[MenuItemModel]) -> Value {
    let list: Vec<Value> = items.iter().map(item_to_json).collect();
    json!({ "menu": { "items": list } })
}

fn find_by_command_id<'a>(items: &'a [MenuItemModel], id: u32) -> Option<&'a MenuItemModel> {
    for item in items {
        if item.command_id == Some(id) {
            return Some(item);
        }
        if let Some(found) = find_by_command_id(&item.submenu, id) {
            return Some(found);
        }
    }
    None
}

/// In-app-menu `get-menu` payload built from the CURRENT config (fresh
/// rebuild so checked states are always live). Called by ipc.rs's
/// `"get-menu"` arm — the shim routes `ipcRenderer.invoke('get-menu')`
/// through peard_ipc_invoke with the channel name verbatim.
pub fn get_menu_json(app: &AppHandle) -> Value {
    let state = app.state::<MenuState>();
    let model = rebuild(app, state.inner());
    model_to_menu_json(&model)
}

/// In-app-menu `get-menu-by-id` payload for one commandId (fresh rebuild
/// so radio-group siblings refresh checked states naturally).
pub fn get_menu_item_json(app: &AppHandle, command_id: u32) -> Option<Value> {
    let state = app.state::<MenuState>();
    let model = rebuild(app, state.inner());
    find_by_command_id(&model, command_id).map(item_to_json)
}

// ---------------------------------------------------------------------------
// Effect executor (port of the electron click handlers)
// ---------------------------------------------------------------------------

pub(crate) fn emit_refresh_in_app_menu(app: &AppHandle) {
    // The renderer TitleBar listens 'refresh-in-app-menu' through the shim's
    // pear-ipc-event fan-out, so the legacy channel name must ride inside
    // the bridge payload — a bare `app.emit("refresh-in-app-menu")` is dead
    // (the webview never subscribes to that Tauri event directly).
    crate::ipc::echo_ipc(app, "refresh-in-app-menu", json!([]))
}

/// Config-mutating effects must also refresh the NATIVE menu bar: checkbox
/// checked-states bake in at native-build time and legacy rebuilt the
/// application menu after every config change. Best-effort — failures log,
/// and the in-app menu still refreshes via emit_refresh_in_app_menu.
pub(crate) fn reattach_native_menu(app: &AppHandle) {
    if let Err(e) = attach_native_menu(app) {
        log::warn!("menu: native menu re-attach failed: {e}");
    }
}

/// Port of the electron `t-menu-item` click handlers: setMenuOption / setOption /
/// togglePluginEnabled / history.back / history.forward / app.restart.
pub fn exec_effect(app: &AppHandle, effect: &MenuEffect) {
    match effect {
        MenuEffect::ToggleBool { key } => {
            let current = {
                let shared = app.state::<SharedStore>();
                let guard = shared.lock().unwrap_or_else(|e| e.into_inner());
                guard.get_bool(key).unwrap_or(false)
            };
            let shared = app.state::<SharedStore>();
            let mut guard = shared.lock().unwrap_or_else(|e| e.into_inner());
            // menu options semantics: setMenuOption triggers restart when
            // options.restartOnConfigChanges — restart_if_configured = true.
            apply_config_change(app, &mut guard, key, Value::Bool(!current), true);
            drop(guard);
            reattach_native_menu(app);
            emit_refresh_in_app_menu(app);
            log::info!("menu: toggled {key} -> {}", !current);
        }
        MenuEffect::SetRadioOption { key, value } => {
            let shared = app.state::<SharedStore>();
            let mut guard = shared.lock().unwrap_or_else(|e| e.into_inner());
            apply_config_change(app, &mut guard, key, Value::String(value.clone()), true);
            drop(guard);
            reattach_native_menu(app);
            emit_refresh_in_app_menu(app);
            log::info!("menu: set {key} -> {value}");
        }
        MenuEffect::TogglePlugin { plugin_id } => {
            // Port of togglePluginEnabled: merged = deepmerge(stub config ?? {enabled:false},
            // current stored config) with config::deepmerge semantics (stored/current wins
            // scalars, defaults fill missing keys), then toggle `enabled` inside the merged
            // object. Missing `enabled` in stored config is UNDEFINED (falsy) in legacy, so
            // the first toggle ENABLES — unwrap_or(false) mirrors that (F-6).
            // Defaults come from the manifest stub config (legacy allPluginStubs[id].config),
            // NOT from a stored plugins.<id>.defaults key that never existed.
            let (merged, new_enabled) = {
                let shared = app.state::<SharedStore>();
                let guard = shared.lock().unwrap_or_else(|e| e.into_inner());
                let defaults = crate::ipc::manifest_plugin_config(plugin_id);
                let defaults = if defaults.is_object() {
                    defaults
                } else {
                    json!({ "enabled": false })
                };
                let current = guard.get(&format!("plugins.{plugin_id}"));
                let merged = deepmerge(&defaults, &current);
                let mut obj = merged.as_object().cloned().unwrap_or_else(Map::new);
                let cur_enabled = obj.get("enabled").and_then(Value::as_bool).unwrap_or(false);
                obj.insert("enabled".into(), Value::Bool(!cur_enabled));
                (Value::Object(obj), !cur_enabled)
            };
            {
                let shared = app.state::<SharedStore>();
                let mut guard = shared.lock().unwrap_or_else(|e| e.into_inner());
                apply_config_change(
                    app,
                    &mut guard,
                    &format!("plugins.{plugin_id}"),
                    merged.clone(),
                    true,
                );
            }
            // Legacy per-plugin event: 'config-changed' with args [pluginId, config].
            crate::ipc::echo_ipc(app, "config-changed", json!([plugin_id, merged]));
            reattach_native_menu(app);
            emit_refresh_in_app_menu(app);
            log::info!("menu: plugin {plugin_id} enabled -> {new_enabled}");
        }
        MenuEffect::NavBack => eval_main(app, "history.back()"),
        MenuEffect::NavForward => eval_main(app, "history.forward()"),
        MenuEffect::Restart => {
            log::info!("menu: restarting app");
            app.restart();
        }
        MenuEffect::ToggleAlwaysOnTop => {
            let current = {
                let shared = app.state::<SharedStore>();
                let guard = shared.lock().unwrap_or_else(|e| e.into_inner());
                guard.get_bool("options.alwaysOnTop").unwrap_or(false)
            };
            let new_val = !current;
            {
                let shared = app.state::<SharedStore>();
                let mut guard = shared.lock().unwrap_or_else(|e| e.into_inner());
                apply_config_change(app, &mut guard, "options.alwaysOnTop", Value::Bool(new_val), true);
            }
            if let Some(win) = app.get_webview_window("main") {
                if let Err(e) = win.set_always_on_top(new_val) {
                    log::warn!("menu: set_always_on_top failed: {e}");
                }
            }
            reattach_native_menu(app);
            emit_refresh_in_app_menu(app);
            log::info!("menu: alwaysOnTop -> {new_val}");
        }
    }
}

fn eval_main(app: &AppHandle, js: &str) {
    if let Some(win) = app.get_webview_window("main") {
        if let Err(e) = win.eval(js) {
            log::warn!("menu: eval `{js}` failed: {e}");
        }
    } else {
        log::warn!("menu: no webview window `main` for eval `{js}`");
    }
}

// ---------------------------------------------------------------------------
// Tauri commands (renderer shim normalizes `peard:*` -> `peard_*`; the legacy
// non-prefixed channels `get-menu` / `get-menu-by-id` stay as-is -> `get_menu` /
// `get_menu_by_id`)
// ---------------------------------------------------------------------------

/// in-app-menu plugin `get-menu`: full menu tree JSON.
#[tauri::command]
pub fn get_menu(app: AppHandle, state: State<'_, MenuState>) -> Result<Value, String> {
    // Rebuild fresh from config — the model is cheap to build and config changes
    // change menu checked states. commandIds are stable per item position order.
    let model = rebuild(&app, state.inner());
    Ok(model_to_menu_json(&model))
}

/// in-app-menu plugin `get-menu-by-id`: single MenuItemJson, rebuilt from config
/// so radio-group siblings refresh checked states naturally.
#[tauri::command]
pub fn get_menu_by_id(
    app: AppHandle,
    state: State<'_, MenuState>,
    command_id: u32,
) -> Result<Value, String> {
    let model = rebuild(&app, state.inner());
    find_by_command_id(&model, command_id)
        .map(item_to_json)
        .ok_or_else(|| format!("no menu item with commandId {command_id}"))
}

/// Renderer click handler: the in-app TitleBar menu invokes `peard:menu-event`
/// with { commandId: number } (ipc.rs routes it here). Tauri 2 matches invoke
/// arguments leniently (camelCase JS keys map to snake_case Rust arg names).
/// Execute one in-app-menu commandId: look up the baked effect, and on a
/// miss rebuild once (mirrors the native-menu-event path — commandIds are
/// deterministic from the rebuild counter) before giving up. Shared by the
/// `peard_menu_event` tauri command and ipc.rs's `peard:menu-event` arm.
pub fn execute_by_command_id(app: &AppHandle, command_id: u32) {
    log::info!("menu: in-app event commandId={command_id}");
    let state = app.state::<MenuState>();
    let effect = {
        let commands = state.commands.lock().unwrap_or_else(|e| e.into_inner());
        commands.get(&command_id).cloned()
    };
    match effect {
        Some(effect) => exec_effect(app, &effect),
        None => {
            // Not yet baked by a get_menu / native build — rebuild once and retry.
            rebuild(app, state.inner());
            let effect = state
                .commands
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(&command_id)
                .cloned();
            match effect {
                Some(effect) => exec_effect(app, &effect),
                None => log::warn!("menu: unknown commandId {command_id}"),
            }
        }
    }
}

#[tauri::command]
pub fn peard_menu_event(app: AppHandle, command_id: u32) {
    execute_by_command_id(&app, command_id);
}

// ---------------------------------------------------------------------------
// Native macOS menu bar (port of Menu.setApplicationMenu)
// ---------------------------------------------------------------------------

/// Build a native tauri `Menu` (the macOS menu bar) from one level of the model.
/// Top-level model items become the bar's top-level `Submenu`s; their children
/// are appended recursively. Clickable items get native ids "menu-<commandId>";
/// predefined items use PredefinedMenuItem so macOS applies its roles. Radio
/// items are CheckMenuItems (no RadioMenuItem in tauri 2 core) — see module docs.
fn build_menu_r(app: &AppHandle, items: &[MenuItemModel]) -> tauri::Result<Menu<Wry>> {
    let root = Menu::new(app)?;
    for item in items {
        if item.submenu.is_empty() {
            // No top-level item in the current model is childless, but stay general:
            append_child(app, &root, item)?;
        } else {
            let sub = build_subtree(app, item)?;
            root.append(&sub)?;
        }
    }
    Ok(root)
}

/// Build one `Submenu` for a model item with children.
fn build_subtree(app: &AppHandle, item: &MenuItemModel) -> tauri::Result<Submenu<Wry>> {
    let id: String = match item.command_id {
        Some(c) => format!("menu-{c}"),
        None => format!("submenu-{}", slug(&item.label)),
    };
    let sub = Submenu::with_id(app, id, item.label.clone(), true)?;
    for child in &item.submenu {
        append_child(app, &sub, child)?;
    }
    Ok(sub)
}

/// Append one model child into a parent `Menu` or `Submenu` (both expose
/// `append(&dyn IsMenuItem)`, but as distinct types — hence the duplicated body).
fn append_child(app: &AppHandle, parent: &dyn ParentMenu, item: &MenuItemModel) -> tauri::Result<()> {
    match item.item_type {
        ItemType::Separator | ItemType::Predefined(PredefKind::Separator) => {
            parent.append_predef(&PredefinedMenuItem::separator(app)?)
        }
        ItemType::Predefined(kind) => {
            let pre = match kind {
                // Vendored 2.12.5 arities: `about(manager, Option<&str>, Option<AboutMetadata>)`;
                // the edit/copy family is `(manager, text: Option<&str>)` — pass None.
                PredefKind::About => PredefinedMenuItem::about(app, None, None)?,
                PredefKind::CloseWindow => PredefinedMenuItem::close_window(app, None)?,
                PredefKind::Quit => PredefinedMenuItem::quit(app, None)?,
                PredefKind::Cut => PredefinedMenuItem::cut(app, None)?,
                PredefKind::Copy => PredefinedMenuItem::copy(app, None)?,
                PredefKind::Paste => PredefinedMenuItem::paste(app, None)?,
                PredefKind::SelectAll => PredefinedMenuItem::select_all(app, None)?,
                PredefKind::Hide => PredefinedMenuItem::hide(app, None)?,
                PredefKind::HideOthers => PredefinedMenuItem::hide_others(app, None)?,
                PredefKind::ShowAll => PredefinedMenuItem::show_all(app, None)?,
                PredefKind::Minimize => PredefinedMenuItem::minimize(app, None)?,
                PredefKind::Separator => PredefinedMenuItem::separator(app)?,
            };
            parent.append_predef(&pre)
        }
        ItemType::Checkbox | ItemType::Radio => {
            let id = item
                .command_id
                .map(|c| format!("menu-{c}"))
                .unwrap_or_else(|| format!("check-{}", slug(&item.label)));
            // CheckMenuItem::with_id(manager, id, text, enabled, checked, accelerator)
            // Radio items are CheckMenuItems — tauri 2 core has no RadioMenuItem;
            // macOS auto-unchecks sibling checks only for radiogroups in muda, so
            // the JSON is refreshed from config in get_menu_by_id instead.
            let check = CheckMenuItem::with_id(app, id, item.label.clone(), true, item.checked, None::<&str>)?;
            parent.append_check(&check)
        }
        ItemType::Normal if !item.submenu.is_empty() => {
            let sub = build_subtree(app, item)?;
            parent.append_submenu(&sub)
        }
        ItemType::Normal if item.effect.is_some() && item.command_id.is_some() => {
            let id = format!("menu-{}", item.command_id.unwrap());
            let mi = MenuItem::with_id(app, id, item.label.clone(), true, None::<&str>)?;
            parent.append_menu_item(&mi)
        }
        ItemType::Normal => {
            // Non-clickable label item (e.g. "No plugins" / info rows): disabled.
            let mi = MenuItem::with_id(app, format!("info-{}", slug(&item.label)), item.label.clone(), false, None::<&str>)?;
            parent.append_menu_item(&mi)
        }
    }
}

/// Internal erasure over `Menu`/`Submenu` so `append_child` works for both parents.
/// Menu types are generic over the Runtime (`tauri::menu::Menu<R>`); this port
/// runs on `Wry`, so the concrete items are `MenuItem<Wry>` etc.
trait ParentMenu {
    fn append_predef(&self, item: &PredefinedMenuItem<Wry>) -> tauri::Result<()>;
    fn append_check(&self, item: &CheckMenuItem<Wry>) -> tauri::Result<()>;
    fn append_menu_item(&self, item: &MenuItem<Wry>) -> tauri::Result<()>;
    fn append_submenu(&self, item: &Submenu<Wry>) -> tauri::Result<()>;
}

impl ParentMenu for Menu<Wry> {
    fn append_predef(&self, item: &PredefinedMenuItem<Wry>) -> tauri::Result<()> {
        self.append(item)
    }
    fn append_check(&self, item: &CheckMenuItem<Wry>) -> tauri::Result<()> {
        self.append(item)
    }
    fn append_menu_item(&self, item: &MenuItem<Wry>) -> tauri::Result<()> {
        self.append(item)
    }
    fn append_submenu(&self, item: &Submenu<Wry>) -> tauri::Result<()> {
        self.append(item)
    }
}

impl ParentMenu for Submenu<Wry> {
    fn append_predef(&self, item: &PredefinedMenuItem<Wry>) -> tauri::Result<()> {
        self.append(item)
    }
    fn append_check(&self, item: &CheckMenuItem<Wry>) -> tauri::Result<()> {
        self.append(item)
    }
    fn append_menu_item(&self, item: &MenuItem<Wry>) -> tauri::Result<()> {
        self.append(item)
    }
    fn append_submenu(&self, item: &Submenu<Wry>) -> tauri::Result<()> {
        self.append(item)
    }
}

fn slug(s: &str) -> String {
    s.chars().map(|c| if c.is_alphanumeric() { c } else { '_' }).collect()
}

/// Ensure `MenuState` is managed, rebuild it from current config, and set the
/// native macOS menu bar. Call once from lib.rs setup. Wire the event handler via
/// `tauri::Builder::on_menu_event(|app, event| crate::menu::on_native_menu_event(app, &event))`.
pub fn attach_native_menu(app: &AppHandle) -> tauri::Result<()> {
    if app.try_state::<MenuState>().is_none() {
        app.manage(MenuState::default());
    }
    let menu_state = app.state::<MenuState>();
    let model = rebuild(app, menu_state.inner());
    let menu = build_menu_r(app, &model)?;
    app.set_menu(menu)?;
    log::info!("menu: native macOS menu bar attached");
    Ok(())
}

/// Entry point wired from lib.rs:
/// `tauri::Builder::on_menu_event(|app, event| crate::menu::on_native_menu_event(app, &event))`.
/// Parses ids of the form "menu-<commandId>" and routes through the same effect
/// executor as in-app clicks.
pub fn on_native_menu_event(app: &AppHandle, event: &tauri::menu::MenuEvent) {
    let id = event.id();
    let id_str: &str = id.as_ref();
    let Some(rest) = id_str.strip_prefix("menu-") else { return };
    let Ok(command_id) = rest.parse::<u32>() else {
        return;
    };
    log::info!("menu: native event id={id_str}");
    let effect = {
        let menu_state = app.state::<MenuState>();
        let commands = menu_state.commands.lock().unwrap_or_else(|e| e.into_inner());
        commands.get(&command_id).cloned()
    };
    match effect {
        Some(effect) => exec_effect(app, &effect),
        None => {
            // Native menu may pre-date the last model bake — rebuild once and retry.
            rebuild(app, &app.state::<MenuState>().inner());
            let menu_state = app.state::<MenuState>();
            let effect = menu_state.commands.lock().unwrap_or_else(|e| e.into_inner()).get(&command_id).cloned();
            match effect {
                Some(effect) => exec_effect(app, &effect),
                None => log::warn!("menu: native id {id_str} resolved to no effect"),
            }
        }
    }
}
