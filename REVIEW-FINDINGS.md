# Windharp Tauri port — reviewer findings

Date: 2026-11-10
Scope: src-tauri/src/{lib,menu,ipc,tray,song_info,config}.rs, assets/pear-shim.js, dist/renderer contracts (dist/index.js, dist/plugins-manifest.json), vendored tauri-2.12.5.
Method: contract comparison against legacy sources (src/providers/*, src/config/*, electron-legacy/*, src/plugins/in-app-menu/*), renderer bundle channel enumeration (all `invoke()`/`send()`/`.on()` channel literals extracted from dist/index.js), vendored-API signature checks. No builds executed.

## Channel matrix

| channel | direction | route | verdict | evidence |
|---|---|---|---|---|
| `peard:ipc-invoke` | renderer→Rust→JS | shim `invoke()` → `peard_ipc_invoke` | OK (arms below) | shim:296; ipc.rs:189 |
| `window-is-maximized` | renderer invoke | `peard_ipc_invoke` | **FAIL — no arm, resolves null** | ipc.rs:316-317 catch-all; bundle TitleBar requires it (dist: `invoke(\`window-is-maximized\`)`) |
| `window-maximize` / `window-unmaximize` / `window-minimize` / `window-close` | renderer invoke | `peard_ipc_invoke` | **FAIL — no arm, resolves null** (buttons do nothing) | ipc.rs:316-317; bundle: `h`/`g`/`_` handlers in TitleBar `hzs` component |
| `window-maximize` / `window-unmaximize` (events) | Rust→renderer | `pear-ipc-event` echo | **FAIL — never emitted by Rust** (grep src-tauri: zero hits) | TitleBar.tsx:287-288 listens; legacy in-app-menu/main.ts:81,83 sent on win maximize/unmaximize |
| `get-menu` / `get-menu-by-id` | renderer invoke | `peard_ipc_invoke` | OK | ipc.rs:293-297; menu.rs get_menu_json/get_menu_item_json fresh-rebuild |
| `peard:menu-event` | renderer invoke | `peard_ipc_invoke` | OK | ipc.rs:300-307 → execute_by_command_id; args positional per PanelItem.tsx:281 |
| `peard:get-config` | renderer invoke | `peard_ipc_invoke` | **DEFECTIVE — wrong default source** (F-2) | ipc.rs:202-221; manifest_default_for (F-2) |
| `peard:set-config` | renderer invoke | `peard_ipc_invoke` | OK | ipc.rs:231-252; deepmerge(stored,value), `config-changed` [id, merged] echo — matches legacy setPartial-no-default + win.send |
| `config-changed` [id, merged] | Rust→renderer | `pear-ipc-event` | OK (emitted by set-config arm and TogglePlugin) | ipc.rs:251; menu.rs:564; renderer.ts:472 consumes |
| `plugin:enable` / `plugin:unload` | Rust→renderer | `pear-ipc-event` | **FAIL — never emitted by Rust** (F-5) | grep src-tauri: zero hits; legacy electron_main:229-237 sends on enabled flip |
| `pear-config-changed` {snapshot} | Rust→shim | Tauri event | OK | config.rs:186; shim replaces storeConfig + runs WATCHERS |
| `peard:store-set` / `peard_store_set_partial` / `peard:set-menu-option` | shim→direct commands | direct (normalized) | OK | config.rs:204-230; restart flag matches legacy setMenuOption |
| `peard:emit` (`ipcRenderer.send`) | shim→Rust | direct `peard_emit` | OK | ipc.rs:44-120; arms + universal echo |
| `pear-ipc-event` {channel,args} | Rust→shim fan-out | Tauri event | OK | ipc.rs:34-39; shim dispatchLocal→`.on` listeners |
| `peard:video-src-changed` | renderer send echo + Rust side-effect | `peard_emit` | OK | ipc.rs:71-84; parse+canonical url persist; tail echo dead-but-harmless (no in-page listener for raw channel) |
| `peard:play-or-paused` | renderer send + Rust re-emit | `peard_emit` → `song_info::peard_play_or_paused` | OK w/ note | song_info.rs:453-481 emits merged SongInfo on `peard:play-or-paused` + tray update; raw tail echo is dead noise (F-8) |
| `peard:time-changed` | renderer send + cache | `peard_emit` → cache-only | OK | ipc.rs:107-110; matches legacy (no emit in legacy either) |
| `peard:update-song-info` [songInfo] | Rust→renderer | `pear-ipc-event` | OK | song_info.rs:446; registerCallback consumers in bundle ON list |
| song-control family (`peard:play`, `pause`, `toggle-play`, `next/previous-video`, `seek-to/by`, `shuffle`, `get-shuffle`+`-response`, `switch-repeat`, `update-like`, queue channels, `like/repeat/volume/shuffle-changed`, setup-*-listener, `peard:get-fullscreen`, `override-progress-bar`, `sponsorblock-skip` …) | renderer→echo→renderer | `peard_emit` echo-only | OK by design | in-page listeners/listeners all in bundle ON/SEND lists; echo fans out |
| tray: `peard:toggle-play`, `peard:next-video`, `peard:previous-video` | Rust→renderer | `pear-ipc-event` emit_to main | OK | tray.rs:92+; renderer.ts:84/87 listeners prefixed — match |
| `toggle-in-app-menu` | Rust (backtick shortcut) / echo | `pear-ipc-event` | OK | lib.rs:270-277; TitleBar listens |
| `refresh-in-app-menu` | Rust→renderer | `pear-ipc-event` | PARTIAL — emitted only by exec_effect (menu.rs), not on renderer-side config writes (F-7) | TitleBar ON list includes it |
| `peard:restart` / `peard:reload` | shim/direct, `peard_emit` arm | direct + ipc.rs:68 arm | OK | config.rs peard_restart; lib.rs do_reload respects resumeOnStart |
| `peard:get-path` / `peard:get-downloads-folder` | renderer invoke | `peard_ipc_invoke` | OK | ipc.rs:257-277; electron path names mapped |
| `peard:captions-selector`, `peard:quality-changer`, `music-together:prompt`, `audio-url`, `download-song`, `download-playlist-request` | renderer invoke | `peard_ipc_invoke` → null | ACCEPTED (plugin main-side backends dormant) — see non-findings | bundle INVOKE list; catch-all null-resolve |

## Findings

### F-1 — BLOCKER — TitleBar window-control invokes are dead (no arms in `peard_ipc_invoke`)
- **File:** `src-tauri/src/ipc.rs:189-321` (arm list); contract: `src/plugins/in-app-menu/renderer/TitleBar.tsx:199-214`, legacy handler `src/plugins/in-app-menu/main.ts:76-84`.
- **Problem:** `peard_ipc_invoke` has no arms for `window-is-maximized`, `window-maximize`, `window-unmaximize`, `window-minimize`, `window-close`. The catch-all (`other => … Ok(Value::Null)`, ipc.rs:316-317) resolves them null. The compiled TitleBar (verified in dist/index.js: `invoke('window-is-maximized')`, and click handlers calling maximize/unmaximize/minimize/close via `await e.ipc.invoke(...)`) therefore renders a title bar whose max/min/close buttons do nothing, and whose max state read is always null (falsy → the maximize path would unmaximize-never). Additionally Rust never emits `window-maximize`/`window-unmaximize` on `pear-ipc-event` (grep src-tauri: 0 hits) although TitleBar.tsx:287-288 registers those listeners — legacy sent them from the main-process `win.on('maximize'|'unmaximize')` (in-app-menu/main.ts:81,83).
- **Required fix:** In `peard_ipc_invoke`, add arms: `window-is-maximized` → `Value::Bool(win.is_maximized())`; `window-close` → trigger close through the same CloseRequested path (`win.close()` will route through `on_window_event` and the tray logic); `window-minimize` → `win.minimize()`; `window-maximize`/`window-unmaximize` → `win.maximize()`/`win.unmaximize()`, and after each state change (or on Tauri window events) echo `window-maximize`/`window-unmaximize` on `pear-ipc-event` so TitleBar's `refetchMaximize` listeners fire.
- **Evidence:** ipc.rs:316-317 catch-all; grep "window-maximize" src-tauri → empty; bundle `hzs` TitleBar component quoted in Channel matrix; TitleBar.tsx:287-288.

### F-2 — HIGH — `manifest_default_for` reads a manifest path that does not exist: defaults always Null
- **File:** `src-tauri/src/ipc.rs:153-170` (function), used at ipc.rs:202-221 (`peard:get-config`).
- **Problem:** The function reads `__defaults__.plugins.<id>`. In the actual manifest (dist/plugins-manifest.json, confirmed by parse: `{"__defaults__": {window-size, window-maximized, window-position, url, options{…}}, "plugins": [41 × {id, restartNeeded, config}]}`), `__defaults__.plugins` is an empty object — plugin default configs live in `plugins[i].config`. The doc comment on the function says the correct legacy source is `allPluginStubs[id].config`. Result: for all 28 plugins with non-trivial default configs, `peard:get-config` returns `deepmerge({enabled:false}, stored)` — default option values are silently dropped. Legacy (electron_main_index_legacy.ts:198-203): `deepmerge(allPluginStubs[name].config ?? {enabled:false}, config.get(plugins.<id>) ?? {})`.
- **Required fix:** Parse `plugins` array, find entry with `id == plugin_id`, and use its `config` field as the defaults value (fall back to `{enabled:false}` when the entry or its config is missing). One bundled manifest parse per invoke is fine (same cost class as today).
- **Evidence:** ipc.rs:160-170 `.get("__defaults__").and_then(|d| d.get("plugins"))`; manifest parse shows `__defaults__.plugins` absent/empty and `plugins[0].config = {"enabled":false}`.

### F-3 — HIGH — Native menu reads/writes config key `options.autoUpdate`; every other surface uses `options.autoUpdates`
- **File:** `src-tauri/src/menu.rs:314-315`.
- **Problem:** `store.get_bool("options.autoUpdate")` / `MenuEffect::ToggleBool { key: "options.autoUpdate" }`. The manifest defaults (`__defaults__.options`), legacy main (electron_main_index_legacy.ts:814 `config.get('options.autoUpdates')`), defaults.ts:74 and all renderer consumers use `options.autoUpdates` (plural). The checkbox therefore always shows the `unwrap_or(true)` default (checked), and toggling it writes a *new* orphan key `options.autoUpdate` into config.json — two adjacent-but-disjoint keys.
- **Required fix:** s/autoUpdate/autoUpdates/ on both lines. (In this port nothing consumes autoUpdates at runtime, but the config-key integrity and the visible checkbox state are still contract.)
- **Evidence:** defaults.ts autoUpdates; manifest `__defaults__.options.autoUpdates: true`; bundle single hit `autoUpdates`.

### F-4 — HIGH — Podcast artist logic inverted in polarity AND wrong source field; OtherVideo/ipcc fallback not ported
- **File:** `src-tauri/src/song_info.rs:355-362` (and the parsed-artist context circa :349-352).
- **Problem:** Rust does `if media_type == PodcastEpisode && use_podcast_participant_as_artist { artist = videoDetails.ownerChannelName }`. Legacy (src/providers/song-info.ts:129-152) does the opposite on both axes:
  1. Polarity: the artist swap happens when `!config.get('options.usePodcastParticipantAsArtist')` (option OFF), not when it's on.
  2. Source: the replacement value is `cleanupName(data.microformat.microformatDataRenderer.pageOwnerDetails.name)`, not `videoDetails.ownerChannelName` (different data — the page owner, cleaned by the suffix-stripping regex).
  3. The legacy default branch (mediaType OtherVideo) applies the same pageOwnerDetails swap gated on the `ipcc` responseContext param (song-info.ts:141-150) — Rust has no equivalent at all.
  Net effect: with the option at its default (off), every podcast episode shows the wrong artist (participant kept); with the option on, it swaps to a different (owner channel) name it shouldn't touch.
- **Required fix:** Invert the condition and read `microformat.microformatDataRenderer.pageOwnerDetails.name`, run it through the `cleanupName`-equivalent suffix stripper; add the OtherVideo branch with the `responseContext.serviceTrackingParams[0].params['ipcc'] != '0'` gate. The file's own comment admits this was "UNCERTAIN legacy behavior" — it is now verified against song-info.ts:129-152.
- **Evidence:** song-info.ts:131-136 (polarity + pageOwnerDetails), :141-150 (ipcc branch); song_info.rs:355-362 (`ownerChannelName`, positive polarity).

### F-5 — HIGH — `plugin:enable` / `plugin:unload` are never emitted: enabling a plugin at runtime never loads its renderer code
- **File:** absent from all of src-tauri (grep `plugin:enable|plugin:unload` src-tauri → 0 hits). Contract: legacy `electron_main_index_legacy.ts:209-244`; renderer `src/renderer.ts:454-462`.
- **Problem:** Legacy main ran a config watch: whenever a plugin's merged `enabled` flipped, it sent `plugin:enable` / `plugin:unload` to the window (renderer.ts:454-462 force-loads/unloads the renderer plugin and calls `onPlayerApiReady` when the player API is up), and showed the restart-needed dialog for `restartNeeded` plugins. The Rust port emits only the legacy `config-changed` [id, merged] (set-config arm, ipc.rs:251; TogglePlugin, menu.rs:564) plus the shim-snapshot event. The renderer's `plugin:enable`/`plugin:unload` listeners are live in the bundle (ON list) but nothing ever fires them. Consequences: enabling e.g. in-app-menu or any plugin from the settings page or the native Plugins menu does nothing until an app restart, and only if `options.restartOnConfigChanges` is set (a separate path); the restart-needed dialog is silently dropped.
- **Required fix:** In `apply_config_change` (config.rs) or at the two plugin-config write sites, diff the previous merged `plugins.<id>.enabled` against the new one (legacy deep-equal check, electron_main:219-227) and `echo_ipc` `plugin:enable`/`plugin:unload` with `[id]` on `pear-ipc-event`; optionally emit a `need-to-restart` marker for `restartNeeded` plugins if a dialog surface exists.
- **Evidence:** grep zero hits in src-tauri; electron_main_index_legacy.ts:229-243; renderer.ts:454-462; bundle ON list contains both channels.

### F-6 — MEDIUM — TogglePlugin merges against a nonexistent `plugins.<id>.defaults` store key and defaults `enabled` to true; first-time plugin toggle is inverted
- **File:** `src-tauri/src/menu.rs:333` (checkbox state) and `menu.rs:540-550` (TogglePlugin effect).
- **Problem:** Two compounding defects vs legacy `pluginEnabledMenu` / `config.plugins.enable/disable`:
  1. `store.get_bool("plugins.<id>.enabled").unwrap_or(true)` — legacy `isEnabled` = `deepmerge(allPlugins[id].config ?? {enabled:false}, stored)`, and every manifest stub's `config` carries `enabled:false` (verified: `plugins[0].config = {"enabled":false}`). So a never-configured plugin renders checked and its merged-default is `enabled:true`, whereas legacy renders unchecked with default false.
  2. In `MenuEffect::TogglePlugin`, the merge base is `guard.get("plugins.{plugin_id}.defaults")` — that key is never seeded into the store (config.rs `seeded_root` only inserts `__defaults__`) and no manifest defaults are written under it. It's always Null, so `deepmerge(Null, current)` = current: stub defaults never fill missing keys.
  Net: on a fresh config, every Plugins-menu item shows enabled, and the first click writes `enabled:false` (disabling a plugin the user believes they are enabling). After the key exists the toggle self-heals, but the initial state is upside-down for all 41 plugins.
- **Required fix:** Read the stub default from the manifest (`plugins[].config`, same lookup style as F-2 — ideally share one helper) for both the checkbox state and the TogglePlugin merge base; default `enabled` from that stub (false) rather than `true`.
- **Evidence:** menu.rs:333, :544-548; config.rs seeded_root inserts only `__defaults__`; manifest entries carry `config.enabled:false`; legacy electron_menu_legacy.ts:43-49 (checked = isEnabled), config/plugins.ts enable/disable.

### F-7 — MEDIUM — Renderer-side config writes never refresh the native menu or the in-app menu panel
- **File:** `src-tauri/src/config.rs` (apply_config_change, :175-190 region) / absence in menu.rs.
- **Problem:** `exec_effect` correctly calls `reattach_native_menu(app)` + `emit_refresh_in_app_menu(app)` after a native-menu toggle, but the settings-page path (`pluginsApi.setOptions` → `peard:store-set` → `apply_config_change`) emits only `pear-config-changed`. After a settings-page toggle (e.g. alwaysOnTop, tray, autoUpdates, plugin enabled), the native menu keeps stale checked states until restart, and the in-app menu panel (TitleBar listens `refresh-in-app-menu`, verified in bundle ON list) is not told to refetch. Legacy rebuilt the whole app menu on every config change (electron main menu refresh).
- **Required fix:** Cheap, targeted option: in `apply_config_change`, after persist+emit, also drive `menu::reattach_native_menu` and `echo_ipc(app, "refresh-in-app-menu", ...)`. Alternatively keep it to option/plugin keys. (COMMAND_ID is reset deterministically in `rebuild()`, and `execute_by_command_id` re-bakes on miss, so clicks keep resolving; this is cosmetic staleness, not a click hazard — hence MEDIUM.)
- **Evidence:** menu.rs exec_effect body (ToggleBool/SetRadioOption/TogglePlugin/ToggleAlwaysOnTop each reattach+refresh); config.rs apply_config_change does neither; TitleBar.tsx ON list.

### F-8 — LOW — Double-delivery of raw `peard:play-or-paused` (dead-echo noise)
- **File:** `src-tauri/src/ipc.rs:107-105 region` (play-or-paused arm) + universal tail echo at ipc.rs:116-120; `song_info.rs:481`.
- **Problem:** `peard_play_or_paused` itself emits the merged-SongInfo form on `peard:play-or-paused` (song_info.rs:481), then the universal tail echo re-sends the raw `[{isPaused, elapsedSeconds}]` on the same channel. Today this is harmless — the bundle's `.on` list contains no `peard:play-or-paused`/`peard:time-changed`/raw `peard:video-src-changed` consumer (song-info consumers all subscribe to `peard:update-song-info`; sponsorblock/downloader use registerCallback) — but it is a deviation from the documented "song_info emits its own echo" design and a landmine for any future plugin that does `.on('peard:play-or-paused')` expecting the SongInfo payload (it would instead get the raw object second). Same applies to `peard:time-changed` (cache-only arm + tail echo of `[seconds]`).
- **Required fix:** In `peard_emit`, skip the tail echo for the three song channels that have Rust side-effect handlers (or at least `peard:play-or-paused`), matching the comment-level design that song_info owns those echoes. If Lead rules the raw echoes intentional for HMR-era parity, downgrade to non-finding.
- **Evidence:** ipc.rs:83-120 arm list + unconditional echo; song_info.rs:481; bundle ON list lacks both raw channels.

### F-9 — LOW — `elapsedSeconds`/`isPaused` not seeded from videoDetails on video-src-changed
- **File:** `src-tauri/src/song_info.rs` (parse_player_response; defaults left None/0, :355 region comments).
- **Problem:** Legacy sets `songInfo.elapsedSeconds = data.videoDetails.elapsedSeconds` and `isPaused` during handleData (src/providers/song-info.ts:157-168 region), so the first `update-song-info` carries a live position. Rust leaves both at defaults (isPaused set `Some(false)` only later inside the play-or-paused command), so until the first play/pause tick, consumers (discord/scrobbler/tuna) see elapsed 0 and a playing state that may be wrong.
- **Required fix:** Parse `videoDetails.elapsedSeconds` and (if present) an initial paused state in `parse_player_response`.
- **Evidence:** song-info.ts:154-168 reads videoDetails.elapsedSeconds; Rust comment at song_info.rs:355 concedes the omission.

### F-10 — LOW — `playlistId` is the raw `list` query value without percent-decoding
- **File:** `src-tauri/src/song_info.rs` (query_param extraction).
- **Problem:** Legacy: `new URL(...).searchParams.get('list')` percent-decodes. Rust keeps the raw substring, so encoded ids (`%3D` etc.) leak into songInfo.playlistId (consumers: downloader filename templating etc.). Cosmetic for typical ids (most are plain base64-ish), so LOW.
- **Required fix:** percent-decode the `list` param value (a tiny decode fn is enough for the query-value charset).
- **Evidence:** URL.parse/searchParams semantics vs raw string slice in Rust.

### F-11 — LOW — Close-to-tray additionally hides the macOS dock icon (legacy only hid the window)
- **File:** `src-tauri/src/tray.rs:287-293` (`handle_close`, "hide window + hide dock").
- **Problem:** Legacy close handler (electron_main_index_legacy.ts:884-892) is only `event.preventDefault(); mainWindow.hide()` — the dock icon stays. Rust `handle_close` also hides the dock, so closing the window removes the app from the Dock/cmd-tab until the tray is used. This may be a deliberate port choice, but it differs from the legacy behavior it names in its own comment.
- **Required fix:** Either drop the dock-hide from `handle_close` (keep dock-hide only in the tray-click toggle path, which does parity electron_tray_legacy.ts:82-88) or annotate it as an accepted deviation in the docs comment.
- **Evidence:** tray.rs:287-293; electron_legacy:885-891 (no dock call).

## Explicit non-findings

Verified-correct (with evidence):

1. **invoke_handler registration** — all 13 commands present exactly once in `tauri::Builder::invoke_handler` (lib.rs), mapping 1:1 to shim direct commands + tray/menu/config commands.
2. **COMMAND_ID stability & reset suspicion** — ruled fixed: `rebuild()` does `COMMAND_ID.store(1, SeqCst)` at entry (menu.rs:397 region); model→commands lock order in `rebuild` is model→commands; `on_native_menu_event` drops the commands guard before the rebuild-retry, so no re-entrant deadlock. (Lead's "reset to 400" number was wrong but the substance — deterministic reset exists — is right. Non-finding.)
3. **Build-then-hook close flow** — `builder.build()?` first, then `win.on_window_event(CloseRequested → tray::handle_close + api.prevent_close)` (lib.rs:157-171); FORCE_QUIT is set both by tray-quit path and `RunEvent::ExitRequested` run-loop handler (lib.rs:291-294), parity with legacy `before-quit` flag.
4. **Echo pipe wiring** — `echo_ipc` = single `pear-ipc-event` `{"channel": <verbatim colon form>, "args": [...]}`; shim `tauriListen` → dispatchLocal → `.on` listeners; `peard_subscribe` no-op is intentional and correct given the shim registers listeners locally.
5. **Tray port** — channel names prefixed (`peard:toggle-play`, `peard:next-video`, `peard:previous-video`) matching renderer.ts:84/87; `update_tray(&AppHandle, Option<bool>)` signature matches sole caller song_info.rs:479; `show_menu_on_left_click(false)` is the correct current tauri-2.12.5 API name; icon files are genuinely 16×16 on disk (PIL verified) so `Image::from_path` without resize is fine; play-pause/next/previous/show/restart/quit ids and `tray-quit → app.exit(0)` (relaunch+exit comment matches legacy) all check out.
6. **Deepmerge parity** — Rust `deepmerge` (ipc.rs:128-142, and config.rs variant) = objects-merge-recursive-else-source-wins ≡ `deepmerge-ts` mergeArrays:false. Fall-through for Null base is correct for the legacy `deepmerge(x ?? {}, y ?? {}, v)` chains.
7. **`peard:set-config` arm** — exactly matches legacy `setPartial('plugins.<id>', obj)` **with no default** (electron_main:205-206): `deepmerge(deepmerge({}, stored), obj)`; the `config-changed` [id, merged] echo is present (ipc.rs:251). No restart (correct — restarts only happen via setMenuOption paths).
8. **Config funnel** — every write path funnels through `apply_config_change` (insert → persist → `pear-config-changed` {snapshot} → optional `app.restart()` when `restart_if_configured && options.restartOnConfigChanges`); `peard_store_set_partial` computes `deepmerge(default ?? {}, stored)` then `deepmerge(base, value)` — exact port of config utils. `peard:set-menu-option` passes restart flag true = legacy setMenuOption (config/index.ts:29-34).
9. **exec_effect gating** — `restart_if_configured` is set true only where legacy `setMenuOption/setMenuOptions` would restart (All ToggleBool/SetRadioOption/TogglePlugin/ToggleAlwaysOnTop), false where legacy wouldn't (renderer store-sets) — verified in the exec_effect body against src/config/index.ts:29-34 and plugins.ts setMenuOptions. `AppHandle::restart()` diverging (`-> !`) used correctly at all three restart sites.
10. **Menu JSON contract** — `item_to_json` shape `{checked, commandId?, label, submenu?:{items}, type}` matches Interface MenuItemJson; separators as type:"normal" label:"" is documented parity.
11. **Init-script order** — bootstrap → SHIM_JS → stubs seed → renderer (lib.rs:141-145); bootstrap carries full config snapshot + pluginNames; stubs seed script correctly publishes `__defaults__` and the full plugins list into `W.__PEAR_PLUGIN_STUBS__` (lib.rs:84-89) so shim `getStub('__defaults__')` and `getStub(plugin).config` work before renderer.ts re-publishes.
12. **peard:get-path / peard:get-downloads-folder** — correct electron→tauri path-api mapping (downloads/desktop/documents/appData→app_config_dir/home), unknown names → null with warn, parity with app-controls.ts:11-15.
13. **Unresolved plugin-backend invokes resolve null** — `peard:captions-selector`, `peard:quality-changer`, `music-together:prompt`, `audio-url`, `download-song`, `download-playlist-request` hit the catch-all and resolve null. These are all plugin main-process backends, which are dormant by the accepted port limitation; the null-resolve (rather than reject) is the right degradation. Not counted against F-1 (those are live in-app-menu surface, not plugin backends).
14. **Tray icon config + tooltips static** — accepted limitation, verified static ("YouTube Music"), with-song-info variant skipped.
15. **startAtLogin wiring** — menu ToggleBool writes `options.startAtLogin`; lib.rs scoped-guard section re-applies autostart plugin enablement on startup; legacy applied `app.setLoginItemSettings` at startup only too (electron_main:809-812). Parity within the accepted autostart-plugin design.
16. **Backtick shortcut** — emits `pear-ipc-event` `toggle-in-app-menu` when window focused, matches TitleBar listener.

## Plan review

**Verdict on the 12-fix plan: LEADING — all 12 items verified landed and correctly shaped** (invoke_handler registration, build-first window-hook, menu-event sync command, PredefinedMenuItem arities, tray API name, update_tray signature, MenuState single-manage, E0507/E0502 fixes, store-guard scoping, config-changed emission, deepmerge funnel, restart funnel). The plan's suspicion list was also the right instinct — commandId determinism confirmed working (non-finding #2). Two plan-adjacent notes: (a) the documented "song_info echoes song channels" design is not yet enforced in `peard_emit` (F-8 — cheap fix); (b) the plan's coverage was confined to previously-flagged defects — none of F-1..F-7 above are regressions from the 12 fixes; they are gap-level defects (missing arms / wrong manifest path / wrong config key / inverted-podcast logic / missing plugin lifecycle events) that no plan item targeted.

**commandId suspicion ruling: fixed, non-finding.** `rebuild()` resets COMMAND_ID to 1 deterministically (menu.rs `fn rebuild` head), and `execute_by_command_id` rebuilds once on a missed lookup (on_native_menu_event tail), so native menu ids `menu-<id>` stay resolvable across re-bakes. Lock discipline: `rebuild` takes model then commands in order; the execute path takes commands only, drops it before any rebuild; no cycle, no re-entrancy. Corollary cosmetic gap tracked as F-7 (stale checked states after renderer-side writes) — not a click hazard.

Severity counts: 1 BLOCKER, 4 HIGH, 2 MEDIUM, 4 LOW (F-8, F-9, F-10, F-11).

---

## Re-review verdict (round 2, 2026-11-10)

Scope: re-audit of all F-1..F-11 fixes + total-build confirmation (no cargo run; read-only verification against the same legacy/renderer contracts).

**Overall verdict: PASS with 1 residual (MEDIUM, F-5a). Fix F-1..F-10 and F-11 are all correctly implemented and verified against the legacy sources. Fix F-5 works for the Rust-initiated paths it was designed for, but the renderer-side settings-page toggle path bypasses the detection — one residual.**

Per-finding verification:

- **F-1 VERIFIED FIXED** (ipc.rs:332-357): all five invoke arms present. `window-is-maximized` → bool, maximize/unmaximize/minimize → `win.maximize()/unmaximize()/minimize()`, `window-close` → `win.close()` (routes through `on_window_event` CloseRequested → tray logic — correct). Event emission (lib.rs:179-199): `WindowEvent::Resized` + transition dedupe via `REPORTED_MAXIMIZED` — correct. Tauri-2 has no Maximize/Unmaximize WindowEvent variants, so Resized-based detection is the right call; the args-are-ignored listener contract (TitleBar refetches) is honored by sending `"args": []`. Transition-only emission avoids spam on ordinary window resizes. Sound, edge cases handled (window-gone → falls back to last reported state).
- **F-2 VERIFIED FIXED** (ipc.rs:163-188): `manifest_plugin_config` now parses `PLUGIN_STUBS`, iterates the top-level `plugins` array, matches `id`, returns `entry.config` (Null → caller falls back to `{enabled:false}`). Doc comment matches the actual manifest shape (verified previously: `__defaults__.plugins` is empty; only the array carries config). get-config arm (ipc.rs:224-240) uses it with the correct `{enabled:false}` base and `deepmerge(defaults, stored)`. Legacy `allPluginStubs[name].config ?? {enabled:false}` parity ≡.
- **F-3 VERIFIED FIXED** (menu.rs:314-315): both read and ToggleBool key are now `options.autoUpdates`. Singular-form key eliminated repo-wide (grep confirms zero hits left).
- **F-4 VERIFIED FIXED** (song_info.rs:408-434): PodcastEpisode swap now gated on `!use_podcast_participant_as_artist` (inverted polarity restored), source is `microformat.pageOwnerDetails.name` with `cleanup_name` applied, `OtherVideo` branch added with the `responseContext.serviceTrackingParams[0].params key=="ipcc"` lookup, absent → `'1'` (legacy `?? '1'`), fires when `!= '0'` and option disabled. Graceful fallback keeps the parsed author when microformat is absent (legacy would throw; keeping is a defensible improvement, noted in the code). cleanup_name is a hand-rolled regex port (suffix/tail-bracket stripping with bounded 8-pass fixpoint), verified against a sam­ple of the legacy cleanups — acceptable, though it may not cover every regex form exactly (residual risk is a slightly-less-clean name in rare cases, not a behavioral break).
- **F-5 PARTIALLY FIXED** — Rust-side detection hoisted into apply_config_change (config.rs:190-220) covers every write path that goes through the funnel with a `plugins.<id>` root key. Old-vs-new `enabled` comparison is faithful (None == legacy undefined vs Some(false) still counts as a transition). BUT see residual F-5a below.
- **F-6 VERIFIED FIXED** (menu.rs:336-345, menu.rs:546-569): checkbox and TogglePlugin both use `deepmerge(manifest_plugin_config(id) ?? {enabled:false}, stored)`; missing `enabled` → `unwrap_or(false)` = legacy undefined-falsy semantics. Bogus `plugins.<id>.defaults` store key eliminated. First-toggle-now-ENABLES matches legacy exactly. TogglePlugin writes through `apply_config_change(key="plugins.<id>", restart=true)` so it also feeds the F-5 funnel — correct (native plugin toggles will emit plugin:enable/unload). `config-changed [id, merged]` echo preserved.
- **F-7 VERIFIED FIXED** (config.rs:237-278): `store_set`, `store_set_partial`, `set_menu_option` all drop the store guard before `reattach_native_menu` + `emit_refresh_in_app_menu` (both pub(crate), correctly documented why: std Mutex non-re-entrancy). The refresh-in-app-menu echo rides inside `pear-ipc-event` (menu.rs:499-505) so the shim fan-out delivers it — correct route.
- **F-8 VERIFIED FIXED** (ipc.rs:118-128): universal echo tail skips `peard:play-or-paused` and `peard:time-changed` only; all other channels still echo (video-src-changed echo intact for sponsorblock/downloader consumers).
- **F-9 VERIFIED FIXED** (song_info.rs:363-373, 492-498): `videoDetails.elapsedSeconds` and `isPaused` mapped in `parse_player_response`; `peard_video_src_changed` only falls back to `Some(false)` when the payload omitted isPaused, so payload values win — matches the sum quoted.
- **F-10 VERIFIED FIXED** (song_info.rs:102-126): `query_param` percent-decodes via byte-slice hex decode, `'+'→' '` (searchParams space semantics), UTF-8-safe (cannot panic on valid input; malformed %XX passes through raw, which is acceptable for this charset).
- **F-11 VERIFIED FIXED** (tray.rs:278-294, lib.rs:328-339): dock-hide removed from `handle_close`; `RunEvent::Reopen { has_visible_windows, .. }` (with the non-exhaustive `..` fix) re-shows + focuses the main window when all are hidden. Comments accurately document why (dock-hide → LSUIElement behavior broke Dock-click reopen). Note the parity consequence: with tray enabled, closing the window leaves the Dock icon present (matches legacy main behavior in this configuration — legacy only hid the window when tray was on, and the Dock icon was never hidden by close in legacy either; correct call given the spaces semantics).

Both compile-error fixes (cleanup_name taking String not &String; RunEvent::Reopen non-exhaustive match) verified as landed correctly in the final shape.

### F-5a — NEW/RESIDUAL (MEDIUM) — renderer-side plugin toggles (settings page, pluginsApi enable/disable) write the whole top-level `plugins` map and bypass the F-5 detection

- **File:** assets/pear-shim.js `pluginsApi.setOptions` (:159-170); src-tauri/src/config.rs apply_config_change (:190-195).
- **Problem:** The renderer's `pluginsApi.setOptions(plugin, options, exclude)` (which `enable`/`disable`/`setMenuOptions` funnel through) calls `invoke('peard:store-set', { key: 'plugins', value: <entire plugins map> })`. In `apply_config_change` the F-5 branch filters on `key.strip_prefix("plugins.")` with a **single segment left over** — for the key `'plugins'` itself (no dot, no remainder), `plugin_root` is None, so no `plugin:enable`/`plugin:unload` is emitted and Rust's stored config changes silently. Legacy main caught this because its config.watch diffed the ENTIRE `newValue.plugins` map against `oldValue.plugins` entry-by-entry after EVERY store.set, not just the single plugin whose key happened to be written (electron_main_index_legacy.ts:211-254). This is the primary user-facing toggle path (the settings UI's enable/disable switch), so most plugin toggles still do nothing until restart — F-5's fix only covers native-menu toggles and the shells that already write coarse-grained keys.
- **Required fix (two options, one-line-ish either way):**
  1. In `apply_config_change`, when key == `"plugins"` and value is an object, iterate its top-level entries (each is a plugin id → config object); for each, compare old `enabled` (from previous stored map) vs new merged enabled (deepmerge stub config, new value) and emit `plugin:enable`/`plugin:unload` for changed ones. This mirrors legacy's whole-map watch exactly. (This is a Rust mirror of legacy config.watch's entry-by-entry diff — moderately involved but correct for arbitrary writers.)
  2. Simpler/safer: change the shim's `pluginsApi.setOptions` to invoke `peard:store-set` with `key: 'plugins.' + plugin` and the per-plugin merged value directly (the shim already computes and `setConfigRaw`s this exact per-plugin merged value at :159-163: `deepmerge(stored, opts)` — just send that value under the per-plugin key instead of the whole map).
- **Impact:** without this, toggling e.g. blur-navbar in the settings page still emits nothing; toggling in the native Plugins menu works (that path writes `plugins.<id>` per the funnel). Native-menu path is the primary desktop-menu surface, so this is MEDIUM, not HIGH — but it is the settings-page UX parity that started this thread.
- **Evidence:** pear-shim.js:159-170 (whole-map write); config.rs:190-195 (`!rest.contains('.')` filter excludes the bare 'plugins' key); electron_main_index_legacy.ts:211-254 (legacy watch diffs the whole map).

### Updated channel-matrix deltas (vs round 1)

- `window-maximize`/`window-unmaximize`/`window-minimize`/`window-close`/`window-is-maximized` invoke: FAIL → OK (F-1 fixed).
- `window-maximize`/`window-unmaximize` events: FAIL → OK (Resized-transition dedupe).
- `window-maximize`/`window-unmaximize` event args: legacy sent `[]` (no payload) — Rust `[]` matches. OK.
- `plugin:enable`/`plugin:unload`: FAIL → **PARTIAL OK** — works for Rust-funneled `plugins.<id>` writes (native menu toggles), missing for renderer whole-map writes (F-5a).
- `peard:play-or-paused` double raw echo: eliminated (F-8).
- Fresh-config plugin default state: fresh-toggle inverted — FIXED (F-6).
- First settings-page config change not refreshing native menu: FIXED (F-7).

### Summary

All 11 findings plus 2 compile issues verified fixed and contract-correct, with one new residual (F-5a) on the renderer settings-page toggle path, introduced because the shim writes the whitelist-key form the Rust detection was scoped to. Recommend applying the shim-side per-plugin-key change (option 2 above, ~2 lines) or extending apply_config_change to handle the whole-map form (option 1). Everything else: no new findings, no regressions on the previously-verified non-findings (re-confirmed: invoke_handler list unchanged, commandId reset in rebuild, echo pipe, tray shapes, config funnel, deepmerge semantics, exec_effect gating).

**Round-2 score: 0 BLOCKER, 0 HIGH, 1 MEDIUM residual (F-5a), 0 LOW. Approve for release build** (which is already running) — F-5a is UX-parity on the settings page, native-menu and Rust internal paths are unaffected.


---

## Re-review verdict (round 3, 2026-11-10) — F-5a fix

**Verdict: VERIFIED FIXED — RESOLVED.** Both sides of the F-5a fix audited read-only against shim contract and the legacy watcher (electron_main_index_legacy.ts:209-243):

1. **Shim side (pear-shim.js:160-172) ✓** — `setOptions` now computes `merged = deepmerge(stored, opts)`, applies it locally via `setConfigRaw('plugins.' + plugin, merged)`, and invokes `peard:store-set` with the per-plugin root key `key: 'plugins.' + plugin, value: merged`. The whole-map write is gone from the primary settings-page path (enable/disable/setMenuOptions all funnel through setOptions). `restartOnConfigChanges` handling in `setMenuOptions` unchanged and still correct.

2. **Rust side (config.rs apply_config_change, bare-`plugins` branch) ✓** — when key == `"plugins"`, the entire old map is snapshotted before the write (`.as_object().cloned()` — non-object/missing → None → branch inert), and after insert/persist/emit the NEW map's entries are iterated (only-new-entries iteration is faithful to legacy `Object.entries(newPluginConfigList)`; deleted ids never visited). Per id: null/non-object new entry → `{}` (`??` parity), old enabled from the old map (missing root → None = legacy undefined), defaults from `manifest_plugin_config` with `{enabled:false}` fallback, merged-new `deepmerge(stub, new)`, transition on `new_enabled != old_enabled` including the None↔Some(false) case — matching legacy `config.enabled !== oldConfig?.enabled`. Emission via `echo_ipc(plugin:enable|plugin:unload, [id])` — correct channel and args shape.

Both detection branches (single-`plugins.<id>` root and bare-`plugins` map) coexist correctly: a per-plugin write takes the `plugin_root` branch; a whole-map write takes the snapshot-diff branch; neither double-fires (the shim no longer produces the map form, and the Rust map branch only runs for the literal `"plugins"` key).

**Semantics delta, documented (not a finding):** legacy's watcher additionally sent `config-changed [id, config]` to the renderer for every non-deep-equal plugin id (even when `enabled` didn't change). The Rust funnel does not emit that per-id echo for generic store-set writes — only TogglePlugin and `peard:set-config` do. This matches the F-5/F-5a scope agreed across rounds (renderer `config-changed` consumers re-read via the snapshot update), and the settings page re-renders from `pear-config-changed` snapshots, so no functional path is lost. Recorded as a known deviation.

**Final score across all rounds: 0 BLOCKER, 0 HIGH, 0 MEDIUM, 0 LOW open. No new findings.** All round-1 findings (F-1..F-11), the F-5a residual, and both mid-batch compile fixes are verified landed and contract-correct. Approve for release pending the in-flight build's own result; recommend a single smoke pass at runtime for the four user-visible F-1/F-4/F-5/F-6 surfaces (window controls + maximize events, a podcast episode's artist, settings-page plugin toggle force-load, fresh-profile first plugin toggle) — automated verification of those fixes was static/read-only.

---

## Appendix — "Out-of-place button" investigation (2026-11-10 ~19:50, post-deploy)

**User report:** a button in the running app looked out of place; top section of the UI squished.

**Root cause (verified live, pixel + OCR + vision-model evidence):** the `in-app-menu` plugin was
enabled in `config.json` and injected its Electron-era TitleBar into the Tauri webview:
- a 32px `position:fixed; top:0` #030303 strip ([titlebar.css](src/plugins/in-app-menu/titlebar.css))
- with `padding-left: 74px` on macOS (`data-macos`, [TitleBar.tsx:56-58](src/plugins/in-app-menu/renderer/TitleBar.tsx)) reserved for traffic lights that **do not exist** in this Tauri build (window has native decorations, no `hiddenInset` overlay) → the lone hamburger floated at x≈75-95 in dead space;
- the `get-menu` row never rendered (native menu serves those menus), so the strip was an empty black band with one orphaned hamburger, visually duplicated against YTM's own guide toggle below-left of it (x≈33 vs x≈81, 48px misaligned);
- the same stylesheet forced `height: calc(100vh - 32px)`, `#content { padding-top: 32px }`, nav-bar/guide/player offsets → the whole top section was squished 32px.

**Why it was on (corrected after shim re-check):** NOT a default bug. The shim defines neither `global` nor `process`, so `defaultInAppMenuConfig.enabled` evaluates FALSE on macOS/Tauri (UA-mac clause fails AND the `global` clause is undefined-false). The strip was on because `config.json` carried a persisted `"in-app-menu": {"enabled": true}` from earlier port testing. A fresh profile would never show it. The constants.ts touch-up shipped alongside (UA fallback refactor) is behavior-neutral in all four environments (Tauri-mac/Electron-mac false, Electron-win/Linux true) — a clarity guard, not a fix.

**Fix applied (config-level, no code change needed):** set `plugins.in-app-menu.enabled = false` in
`~/Library/Application Support/com.github.th-ch.youtube-music/config.json` (backup:
`config.json.bak-inappmenu-*`), graceful quit + relaunch. The native macOS menu bar
(Apple/YouTube Music/Options/Plugins/Advanced options/Edit/Navigation) is attached unconditionally
(`menu::attach_native_menu`, lib.rs:289) and unaffected.

**Verified after relaunch (pid 1969, 1366x831 capture):**
- the #030303 strip and the floating hamburger are gone (logo row moved up ~30px: logo bbox y19-41 vs y49-73 before);
- search pill / Sign in now at y≈20-23 (were y≈41-59 under the strip);
- single hamburger only, left-aligned with the YouTube Music logo (x25-42);
- OCR confirms normal layout; native menu bar intact.

**Known deviation (documented, accepted):** the in-app-menu plugin's TitleBar (and its backtick
collapse toggle target) is now off on macOS; the Rust backtick shortcut remains registered and
emits `toggle-in-app-menu`, which no visible UI consumes while the plugin is disabled — harmless.
If traffic-light overlay + real menu row is ever wanted in-webview, the correct port fix is
`title_bar_style(TitleBarStyle::Overlay)` + `hide_menu` default true on macOS — not pursued
(native menu bar already covers it).

**Residual recommendation:** `defaultInAppMenuConfig.enabled` should be `false` on darwin when
`__IS_TAURI__` (one-line guard) so fresh profiles don't re-introduce the strip; left as-is because
the deployed config now pins it off and a rebuild was not required for this fix.
