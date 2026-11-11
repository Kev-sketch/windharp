# Tauri Port Notes — What the Port Does and Does Not Do

This document audits the Electron→Tauri 2 port of Pear Desktop against the legacy Electron codebase, claim by claim, with `file:line` evidence. **The single most important finding colors every claim below: the Rust side of the port ([src-tauri/src/lib.rs](../src-tauri/src/lib.rs)) is a bare Tauri template — 16 lines containing only `tauri::Builder::default()` plus the log plugin in debug builds. There is no `invoke_handler`, no `peard:ipc-invoke` command, no menu/tray/window/config-store code, and none of the plugins declared in [src-tauri/Cargo.toml](../src-tauri/Cargo.toml) (single-instance, autostart, window-state, global-shortcut) are registered.** Every claim that "Rust implements X" is therefore aspirational: the real, working artifacts are the renderer bridge ([assets/pear-shim.js](../assets/pear-shim.js)), the renderer-only Vite build ([vite.config.ts](../vite.config.ts)), and [src-tauri/tauri.conf.json](../src-tauri/tauri.conf.json). The entire legacy TypeScript main process under [src/](../src) (loader, config, providers, plugin backends) is **dead code** in the Tauri build — Vite bundles only the renderer entry ([vite.config.ts](../vite.config.ts), IIFE output `dist/index.js`, `__IS_TAURI__: true` define).

Verification method: every legacy claim was checked by reading the cited legacy file at the cited lines. Claims about Rust behavior that could not be backed by code are marked **UNVERIFIED — assumed**. A repo-wide grep for `peard:ipc-invoke|invoke_handler|peard:subscribe|peard:emit|peard:store-set` across `*.rs`, `*.json`, `*.toml` returned **zero hits outside [assets/pear-shim.js](../assets/pear-shim.js)** — the shim invokes commands that nothing registers.

---

## 1. Claim-by-claim verdict table

| # | Claim (as given) | Verdict | Evidence / correction |
|---|---|---|---|
| 1 | Rust main process reproduces the Electron main process (window, menu, tray, IPC) | **FALSE as stated** | [src-tauri/src/lib.rs](../src-tauri/src/lib.rs) is 16 lines, bare `tauri::Builder::default()` + `tauri_plugin_log` (debug only). No `invoke_handler`, no window/menu/tray code. UNVERIFIED — assumed. |
| 2 | Renderer bundle is unchanged; only the preload is replaced by a shim | **Partly TRUE** | Vite builds a renderer-only IIFE (`PearRenderer` → `dist/index.js`) from [src/renderer.ts](../src/renderer.ts) with `__IS_TAURI__: true` defined ([vite.config.ts](../vite.config.ts)). The shim [assets/pear-shim.js](../assets/pear-shim.js) reproduces `ipcRenderer`/`mainConfig`/`electronIs`. But `window.ipcRenderer.sendSync` is stubbed to `return [null, '']` (shim ~L258), so code paths relying on `get-renderer-script` no-ops (see §3). |
| 3 | Single-instance lock ported via `tauri-plugin-single-instance` | **UNVERIFIED — assumed** | Legacy: `app.requestSingleInstanceLock()` + exit ([electron-legacy/electron_main_index_legacy.ts:67-70](../electron-legacy/electron_main_index_legacy.ts)). Cargo.toml declares the plugin; lib.rs never calls `.plugin(tauri_plugin_single_instance::init(...))`. Not wired. |
| 4 | Menu ported to native menu / in-app-menu | **UNVERIFIED — assumed** | Legacy menu: [electron-legacy/electron_menu_legacy.ts](../electron-legacy/electron_menu_legacy.ts) (757 lines, 113 `t()` i18n calls). No Rust menu code exists. In-app-menu plugin backend (which serves `get-menu` JSON, [src/plugins/in-app-menu/main.ts:28-34](../src/plugins/in-app-menu/main.ts)) is dead code in the Tauri build. |
| 5 | Tray ported with template icons | **UNVERIFIED — assumed; icon nuance corrected** | Legacy tray: [electron-legacy/electron_tray_legacy.ts:1-4](../electron-legacy/electron_tray_legacy.ts) imports `tray-white.png`/`tray-paused-white.png` (macOS) and `tray.png`/`tray-paused.png`; icon pick at L56-67. **Correction: filenames carry no `Template` suffix and no `setTemplateImage`/template-flag call exists in the file — macOS uses plain *white* PNGs, not NSImage template tinting.** No Rust tray code exists. |
| 6 | Config store ported to a Rust-backed store | **UNVERIFIED — assumed** | Shim calls `invoke('peard:store-set')`/`peard:store-set-partial`/`peard:set-menu-option` ([assets/pear-shim.js:124-134](../assets/pear-shim.js)) — no Rust command handles them. Legacy store with migrations: [src/config/store.ts:22-280](../src/config/store.ts) (adblocker→do-not-track ≥3.12.0 L23-32; lyrics-genius→synced-lyrics ≥3.10.0 L33-58; lastfm→scrobbler ≥3.3.0 L59-108; discord ≥3.0.0 L109+; ctor L282-289 with `clearInvalidConfig: false`). No Tauri-side migration equivalent. |
| 7 | IPC bridged via one Rust command `peard:ipc-invoke` | **Shim side TRUE, Rust side UNVERIFIED — assumed** | Shim: every `invoke(channel)` except `peard:get-main-plugin-names` goes to `invoke('peard:ipc-invoke', {channel, args})` ([assets/pear-shim.js:284-290](../assets/pear-shim.js)); `send` → `peard:emit`, `on` → `peard:subscribe` + local bus + `pear-ipc-event` Tauri event (L202, L273, L271-277). No Rust implementation of any of these commands exists. |
| 8 | Privileged scheme registration (`youtubemusic://`, http/https privileges) ported | **UNVERIFIED — assumed / dropped** | Legacy: `protocol.registerSchemesAsPrivileged` with `bypassCSP`, `allowServiceWorkers`, `codeCache` ([electron-legacy/electron_main_index_legacy.ts:72-98](../electron-legacy/electron_main_index_legacy.ts)). tauri.conf.json registers no custom protocol; `assetProtocol` is disabled. |
| 9 | CSP removal ported | **Behaviorally N/A in Tauri config, not implemented** | Legacy `removeContentSecurityPolicy()` deletes CSP headers on every https response and injects `access-control-allow-origin: https://music.youtube.com` when absent ([electron-legacy/electron_main_index_legacy.ts:927-965](../electron-legacy/electron_main_index_legacy.ts)). tauri.conf.json sets `security.csp: null` (no CSP injected for the app shell), but there is no webRequest hook to strip YouTube's own CSP — UNVERIFIED that playback works without it. |
| 10 | Tauri loads the same built renderer in dev and prod | **Correction: dev uses the Vite dev server** | tauri.conf.json `devUrl: http://localhost:5173` (Vite dev server, [vite.config.ts](../vite.config.ts) `server.port: 5173`), `frontendDist: ../dist` (Vite build output). So `tauri dev` does **not** read `dist/` — it hot-serves from Vite; only builds use `../dist`. |
| 11 | `image-path-to-data-url` intentionally unported | **TRUE (supported)** | Handler at [src/plugins/in-app-menu/main.ts:85-88](../src/plugins/in-app-menu/main.ts); repo-wide grep finds **zero** other references in `src/` — no renderer/plugin consumer exists, so dropping it is safe. |
| 12 | Tray template-icon tinting lost | **Directionally right, imprecisely stated** | See claim 5: legacy macOS icons are plain white PNGs without `Template` naming ([electron-legacy/electron_tray_legacy.ts:1-4](../electron-legacy/electron_tray_legacy.ts)); the "template" property was never explicit in code. Tauri `Image` has no template flag, so any port ships fixed-color icons; the practical loss vs legacy is none on macOS (legacy also used fixed white PNGs) — the claim overstated legacy behavior. |
| 13 | Backtick opens the in-app menu via a global shortcut gated in Rust | **Correction: legacy backtick is a window-local shortcut** | `register(win, '`', …)` from `electron-localshortcut` — window-scoped, not global — at [src/plugins/in-app-menu/main.ts:23](../src/plugins/in-app-menu/main.ts). Any Rust global-shortcut gating is UNVERIFIED (no code). |
| 14 | `overrideUserAgent` ported | **UNVERIFIED — assumed** | Legacy: per-platform Chrome/130 UA strings set on `webContents.userAgent` + `app.userAgentFallback`, with a Google-login retry that restores the original UA ([electron-legacy/electron_main_index_legacy.ts:534-559](../electron-legacy/electron_main_index_legacy.ts)). No Tauri-side UA code. |
| 15 | Proxy support (`options.proxy` + auth-proxy-adapter) ported | **UNVERIFIED — assumed** | Legacy: `app.commandLine.appendSwitch('proxy-server', …)` with socks5 from auth-proxy-adapter config ([electron-legacy/electron_main_index_legacy.ts:151-169](../electron-legacy/electron_main_index_legacy.ts)); menu prompt writes `options.proxy` ([electron-legacy/electron_menu_legacy.ts:727-751](../electron-legacy/electron_menu_legacy.ts)). No Tauri proxy code. |
| 16 | Window state (size/position/maximized) persistence ported via `tauri-plugin-window-state` | **UNVERIFIED — assumed** | Legacy: debounced `lateSave` on move/resize + `window-maximized` tracking ([electron-legacy/electron_main_index_legacy.ts:437-472](../electron-legacy/electron_main_index_legacy.ts)); offscreen-position guard L387-426. Plugin declared in Cargo.toml, never registered. |
| 17 | Auto-update ported | **UNVERIFIED — assumed (likely dropped)** | Legacy: `electronUpdater.autoUpdater.autoDownload = false` ([electron-legacy/electron_main_index_legacy.ts:65](../electron-legacy/electron_main_index_legacy.ts)) and a non-dev `autoUpdates` check at L814. No updater in Cargo.toml, no Tauri updater plugin, no `tauri.conf` updater block. |
| 18 | `disableHardwareAcceleration` ported | **UNVERIFIED — assumed** | Legacy: config read at L114-115, forced on for transparent-player plugin L126-130, applied L143-145 ([electron-legacy/electron_main_index_legacy.ts](../electron-legacy/electron_main_index_legacy.ts)). No Tauri equivalent (WKWebView/System WebView; no such switch exists). |
| 19 | MPRIS/media-key suppression on Linux ported | **Not ported (legacy-only behavior)** | Legacy: `MediaSessionService` feature disabled when shortcuts plugin is enabled ([electron-legacy/electron_main_index_legacy.ts:119-121](../electron-legacy/electron_main_index_legacy.ts)); plugin's own MPRIS backend at [src/plugins/shortcuts/mpris.ts](../src/plugins/shortcuts/mpris.ts) is dead code in the Tauri build. |
| 20 | `startAtLogin` ported via autostart plugin | **UNVERIFIED — assumed** | Legacy: `app.setLoginItemSettings({openAtLogin: …})` ([electron-legacy/electron_main_index_legacy.ts:810-811](../electron-legacy/electron_main_index_legacy.ts)); menu checkbox at [electron-legacy/electron_menu_legacy.ts:419-424](../electron-legacy/electron_menu_legacy.ts). Plugin declared, not registered. |
| 21 | Song-info provider chain ported | **Renderer half real, main-process half dead** | Renderer-side `song-info-front` mutations verified: album from `playerOverlays` (ytmdWatchNextResponse or first overlay-bearing entry), `elapsedSeconds = 0`, `isPaused = false`, then `send('peard:video-src-changed', data)` ([src/providers/song-info-front.ts:332-356](../src/providers/song-info-front.ts)). Legacy main-side `handleData` normalization ([src/providers/song-info.ts:62-117](../src/providers/song-info.ts)) — microformat parsing, `config.set('url', …)` at L101, `win.webContents.send('peard:update-song-info', …)` at L160 — has no Rust consumer; those channels die at the shim's local bus. |
| 22 | Plugin backends (api-server, discord, downloader, …) ported to Rust | **FALSE — dead code** | Vite bundles only the renderer; all backend code ([src/plugins/api-server/backend/main.ts](../src/plugins/api-server/backend/main.ts) Hono HTTP+WS server, discord, downloader at [src/plugins/downloader/main/index.ts:216-220](../src/plugins/downloader/main/index.ts), notifications, touchbar, lumiastream, tuna-obs, sponsorblock, synced-lyrics) is unreachable in the Tauri build. No Rust replacements. |
| 23 | Full IPC channel surface bridged | **Shim design covers it; runtime is a no-op** | Census in the Appendix. The shim forwards every channel generically, but with no Rust handlers, `invoke` rejects and `send` only loops back through the local bus (`pear-ipc-event` listener, [assets/pear-shim.js:271-277](../assets/pear-shim.js)). Renderer-only flows (e.g. `peard:search` handled in [src/renderer.ts:285-313](../src/renderer.ts) → `peard:search-results`) still work via the local bus. |
| 24 | `peard:video-src-changed` carries mutated song data | **TRUE, renderer-side** | Mutations exactly as claimed at [src/providers/song-info-front.ts:341-356](../src/providers/song-info-front.ts); the main-process enrichment half (claim 21) does not exist. |
| 25 | Migration of legacy config on first run | **Not implemented** | Legacy migration chain: [src/config/store.ts:22-280](../src/config/store.ts). No Rust/Tauri migration code; the shim even receives its config snapshot via `__PEAR_BOOTSTRAP__` rather than reading any store. |

---

## 2. Ported 1:1 (renderer-side, actually working)

| Feature | Legacy behavior (file:line) | Tauri port behavior | Impact |
|---|---|---|---|
| Renderer bundle logic | [src/renderer.ts](../src/renderer.ts) | Same source compiled by Vite as IIFE with `__IS_TAURI__: true` ([vite.config.ts](../vite.config.ts)) | None — renderer code is the real port |
| YT ES5-adapter strip | [electron-legacy/electron_preload_legacy.ts:23-42](../electron-legacy/electron_preload_legacy.ts) | Shim removes `custom-elements-es5-adapter` before it loads ([assets/pear-shim.js](../assets/pear-shim.js)) | None |
| In-page player controls (`peard:play`, `peard:seek-to`, queue ops, `peard:search` → `networkManager.fetch('/search')`) | [src/renderer.ts:285-313](../src/renderer.ts) | Same handlers run in the page; `send` loops back through the shim's local bus | Works, since both endpoints are renderer-side |
| Song-data mutation on video change | [src/providers/song-info-front.ts:332-356](../src/providers/song-info-front.ts) | Unchanged | None |
| deepmerge config semantics (`mergeArrays: false`) | [src/config/index.ts](../src/config/index.ts) | Shim reimplements deepmerge with the same array semantics | None |

## 3. Ported with behavioral differences

| Feature | Legacy behavior (file:line) | Tauri port behavior | Impact |
|---|---|---|---|
| Preload bridge | contextBridge `mainConfig`/`electronIs`/`ipcRenderer` incl. working `sendSync` ([electron-legacy/electron_preload_legacy.ts:72-107](../electron-legacy/electron_preload_legacy.ts): `sendSync('get-renderer-script')` → `webFrame.executeJavaScript`, isolated world at L93) | Shim's `sendSync` returns `[null, '']` unconditionally ([assets/pear-shim.js](../assets/pear-shim.js)) | Renderer-script injection path is a silent no-op; harmless today because the renderer bundle is the page script, but any `sendSync` consumer gets fake success |
| `get-renderer-script` asar/dev-server loader | `ipcMain.on('get-renderer-script')` dev-server/asar read ([electron-legacy/electron_main_index_legacy.ts:717+](../electron-legacy/electron_main_index_legacy.ts)) | No equivalent; not needed since Tauri loads the bundle as the page | None for the port; lost legacy capability only |
| Dev serving | Electron dev used `VITE_DEV_SERVER_URL` through preload injection | `tauri dev` serves from the Vite dev server at `localhost:5173` (tauri.conf `devUrl`); builds use `frontendDist: ../dist` | Claim 10's phrasing corrected — dev never reads `dist/` |
| CORS for renderer origin | Header injected via `onHeadersReceived` ([electron-legacy/electron_main_index_legacy.ts:949-960](../electron-legacy/electron_main_index_legacy.ts)) | Vite dev server `cors: {origin: 'https://music.youtube.com'}` ([vite.config.ts](../vite.config.ts)); production gets no equivalent (asset protocol, no CORS needed) | Dev parity only; production behavior depends on unimplemented webview hooks |
| Plugin-config plumbing | `peard:get-config`/`peard:set-config` handled in main ([electron-legacy/electron_main_index_legacy.ts:198-207](../electron-legacy/electron_main_index_legacy.ts)) with config-watch fan-out `plugin:enable`/`plugin:unload`/`config-changed` (L209-262) | Shim serves `peard:get-main-plugin-names` locally and forwards the rest to the nonexistent `peard:ipc-invoke` ([assets/pear-shim.js:287-290](../assets/pear-shim.js)) | Plugin enable/disable and config change notifications cannot work until Rust implements the store + watch |

## 4. Bridged generically (runtime no-op / graceful failure)

| Feature | Legacy behavior (file:line) | Tauri port behavior | Impact |
|---|---|---|---|
| All `ipcRenderer.send` channels | Delivered to `ipcMain.on` listeners in main/plugins | `invoke('peard:emit')` → no Rust command → `.catch` warns in console ([assets/pear-shim.js:273-275](../assets/pear-shim.js)) | Every renderer→main notification (play/pause state, volume, seeked, fullscreen, video-src-changed, …) silently dies at the bridge |
| All `ipcRenderer.on` channels | Main pushed via `webContents.send` | Local bus + `tauriListen('pear-ipc-event')`; nothing ever emits from Rust ([assets/pear-shim.js:271-277](../assets/pear-shim.js)) | Renderer listeners registered but never fire for main-originated events; renderer-loopback events do fire |
| `invoke` of any backend command (`peard:restart`, `peard:reload`, `peard:get-path`, `peard:get-downloads-folder`, `get-menu`, `window-maximize`, …) | Real handlers ([src/providers/app-controls.ts](../src/providers/app-controls.ts); [src/plugins/in-app-menu/main.ts:28-34](../src/plugins/in-app-menu/main.ts)) | Rejected promise from Tauri core (`command not found`) | UI actions fail; TitleBar buttons ([src/plugins/in-app-menu/renderer/TitleBar.tsx:195-218](../src/plugins/in-app-menu/renderer/TitleBar.tsx)) all dead-end |
| `music-together:prompt`, `synced-lyrics:fetch`, `sponsorblock-skip`, `audio-url`, plugin prompts | Handled in plugin backends ([src/plugins/music-together/index.ts:92](../src/plugins/music-together/index.ts); [src/plugins/synced-lyrics/backend.ts:23](../src/plugins/synced-lyrics/backend.ts); [src/plugins/sponsorblock/index.ts:84,107](../src/plugins/sponsorblock/index.ts)) | Forwarded to `peard:ipc-invoke` → rejected | Graceful failure (rejected promise), not a crash |

## 5. Dropped by design (or by omission)

| Feature | Legacy behavior (file:line) | Tauri port behavior | Impact |
|---|---|---|---|
| Full main-process plugin runtime (42 plugins) | [src/plugins/*/main.ts, backend.ts](../src/plugins) — api-server HTTP/WS, discord RPC, downloader, notifications, MPRIS, touchbar, do-not-track adblocker | Not bundled; vite builds renderer only; no Rust hosts | The app is a bare YTM webview + renderer scripts; every plugin whose value lives in the main process is gone |
| electron-store + migrations | [src/config/store.ts:22-289](../src/config/store.ts) | No store implementation; `peard:store-set*` unhandled | Settings cannot persist; legacy users' config not migrated |
| Auto-update | autoDownload=false + `autoUpdates` check ([electron-legacy/electron_main_index_legacy.ts:65,814](../electron-legacy/electron_main_index_legacy.ts)); menu checkbox [electron-legacy/electron_menu_legacy.ts:162-168](../electron-legacy/electron_menu_legacy.ts) | Nothing | No update mechanism |
| Native app menu + tray | [electron-legacy/electron_menu_legacy.ts](../electron-legacy/electron_menu_legacy.ts) (plugins menu L153-157, options L159+, set-proxy L520-529, customWindowTitle prompt L216-245, tray radios L429-478, language L481-517, macOS app menu L700-720); tray [electron-legacy/electron_tray_legacy.ts:80-152](../electron-legacy/electron_tray_legacy.ts) (click play/pause L80-90, i18n menu L92-128) | Nothing in Rust; tray-icon feature enabled in Cargo but unused | No menu, no tray, no settings UI reach |
| Proxy / UA override / HWA disable | [electron-legacy/electron_main_index_legacy.ts:151-169, 534-559, 114-145](../electron-legacy/electron_main_index_legacy.ts) | Nothing | Power-user options gone |
| CSP header stripping & CORS injection on YouTube responses | [electron-legacy/electron_main_index_legacy.ts:927-965](../electron-legacy/electron_main_index_legacy.ts) | `csp: null` covers only the app shell; no response-header hook | Potentially breaks renderer injection into YouTube pages if YT ships CSP (UNVERIFIED) |

## 6. Build/packaging differences

| Aspect | Legacy (Electron) | Tauri port | Evidence |
|---|---|---|---|
| Entry/bundle | electron-builder, asar, preload.cjs ([electron-legacy/electron_main_index_legacy.ts:359-372](../electron-legacy/electron_main_index_legacy.ts) webPreferences/preload) | Vite IIFE `dist/index.js`, tauri bundle `["app","dmg"]`, macOS min 10.15, `withGlobalTauri: true`, `windows: []` (no window defined in config — creation must come from Rust, which has none) | [vite.config.ts](../vite.config.ts); [src-tauri/tauri.conf.json](../src-tauri/tauri.conf.json) |
| Platform identity | WM_CLASS/AppUserModelID, Windows Start-Menu shortcut self-repair | Not addressed | [electron-legacy/electron_main_index_legacy.ts:133-142, 653-692](../electron-legacy/electron_main_index_legacy.ts) |
| Deep links | `youtubemusic://` protocol handler ([src/providers/protocol-handler.ts](../src/providers/protocol-handler.ts)) | No protocol registration in tauri.conf | tauri.conf.json |
| Version | 4.0.0 line | `4.0.0-tauri.0` | [package.json](../package.json); tauri.conf.json |
| macOS arch | electron-builder universal | `tauri:build:mac` targets `universal-apple-darwin` | [package.json](../package.json) scripts |

---

## 7. Top additional user-noticeable losses (not in the original claims list)

1. **Renderer-crash UX**: legacy shows an unresponsive-renderer dialog with Wait/Relaunch/Quit ([electron-legacy/electron_main_index_legacy.ts:905-924, 897-902](../electron-legacy/electron_main_index_legacy.ts)) and a load-failure fallback page (`did-fail-load` → `ErrorHtmlAsset` + `log` to renderer, L598-612). The port has neither.
2. **YouTube-restricted-region workaround**: `will-redirect` intercepts `youtube.com/premium` and reroutes to the Google music ServiceLogin ([electron-legacy/electron_main_index_legacy.ts:509-527](../electron-legacy/electron_main_index_legacy.ts)). Gone.
3. **macOS lifecycle semantics**: close-to-tray (window `close` → hide unless forceQuit, L886-897), dock hide when `appVisible` false (L876-878), `activate` re-show (L635-644), plus `autoResetAppCache` 20s cache clear (L659-669). None exist in the port.

---

## Appendix: IPC channel census

Direction as seen from the renderer. **Rust handling for every channel: none — [src-tauri/src/lib.rs](../src-tauri/src/lib.rs) registers no commands; `peard:ipc-invoke`, `peard:subscribe`, `peard:emit`, `peard:store-set`, `peard:store-set-partial`, `peard:set-menu-option`, `peard:restart` do not exist in Rust (repo-wide grep, zero hits).** The classification below therefore describes the shim's intended routing (per [assets/pear-shim.js](../assets/pear-shim.js)) and where the other endpoint actually lives.

| Channel | Direction | Legacy counterpart (file:line) | Shim routing | Effective runtime behavior |
|---|---|---|---|---|
| `peard:get-main-plugin-names` | renderer→invoke | [electron-legacy/electron_main_index_legacy.ts:191-194](../electron-legacy/electron_main_index_legacy.ts) | answered locally from `__PEAR_BOOTSTRAP__` | **works** |
| `peard:get-config` / `peard:set-config` | renderer↔invoke | [electron-legacy/electron_main_index_legacy.ts:198-207](../electron-legacy/electron_main_index_legacy.ts) | `peard:ipc-invoke` | no handler — rejected |
| `peard:store-set` / `peard:store-set-partial` / `peard:set-menu-option` / `peard:restart` | shim-internal | config set/setMenuOption ([src/config/index.ts](../src/config/index.ts)) | direct invoke | no handler — rejected; shim still updates its in-memory snapshot first |
| `peard:menu-event` | renderer→invoke | [src/plugins/in-app-menu/main.ts:54-64](../src/plugins/in-app-menu/main.ts) | `peard:ipc-invoke` | no handler |
| `peard:search` / `peard:search-results` | main→renderer / renderer→main | send [src/providers/song-controls.ts:147](../src/providers/song-controls.ts); handle [src/renderer.ts:285-313](../src/renderer.ts) | emit/subscribe + local bus | **works renderer↔renderer** (both endpoints in page) |
| `peard:video-src-changed` | renderer→main | [src/providers/song-info-front.ts:355](../src/providers/song-info-front.ts); listener [src/providers/song-info.ts:187-198](../src/providers/song-info.ts) | emit → no Rust; local bus only | main-process enrichment never runs |
| `peard:update-song-info` | main→renderer | [src/providers/song-info.ts:160](../src/providers/song-info.ts) | subscribe | never emitted from Rust |
| `peard:play/pause/toggle-play/previous-video/next-video/seek-to/seek-by/shuffle/get-shuffle/switch-repeat/toggle-mute/update-volume/update-like` | renderer→main (song controls) | [src/providers/song-controls.ts](../src/providers/song-controls.ts) | emit | no handler — media keys/remote control dead |
| `peard:get-queue/clear-queue/remove-from-queue/set-queue-index/add-to-queue/move-in-queue` | renderer↔renderer | handlers in [src/renderer.ts:230-282](../src/renderer.ts) | local bus | works when invoked from page |
| `peard:get-fullscreen/set-fullscreen`, `setup-*-changed-listener` family (autoplay/fullscreen/like/repeat/seeked/shuffle/time/volume) | mixed | [src/providers/song-info-front.ts:196-224](../src/providers/song-info-front.ts); api-server backend subscribes [src/plugins/api-server/backend/main.ts:39-53](../src/plugins/api-server/backend/main.ts) | local bus | renderer↔renderer OK; plugin backends gone |
| `peard:seeked/time-changed/play-or-paused/volume-changed/shuffle-changed/repeat-changed/fullscreen-changed/autoplay-changed/player-api-loaded` | renderer→main | [src/providers/song-info-front.ts](../src/providers/song-info-front.ts) | emit | no consumer (song-info.ts listeners are dead code) |
| `plugin:enable` / `plugin:unload` / `config-changed` / `log` | main→renderer | [electron-legacy/electron_main_index_legacy.ts:209-262](../electron-legacy/electron_main_index_legacy.ts) | subscribe | never emitted |
| `get-menu` / `toggle-in-app-menu` / `refresh-in-app-menu` / `window-maximize` / `window-unmaximize` / `window-minimize` / `window-close` / `window-is-maximized` / `image-path-to-data-url` | renderer→invoke | [src/plugins/in-app-menu/main.ts:23-34, 85-88](../src/plugins/in-app-menu/main.ts); TitleBar calls [src/plugins/in-app-menu/renderer/TitleBar.tsx:195-218](../src/plugins/in-app-menu/renderer/TitleBar.tsx) | `peard:ipc-invoke` | no handler — in-app menu fully nonfunctional |
| `music-together:prompt` | renderer→invoke | [src/plugins/music-together/index.ts:92, 736](../src/plugins/music-together/index.ts) | `peard:ipc-invoke` | no handler |
| `synced-lyrics:fetch` | renderer→invoke | [src/plugins/synced-lyrics/backend.ts:23](../src/plugins/synced-lyrics/backend.ts) | `peard:ipc-invoke` | no handler |
| `sponsorblock-skip` | main→renderer | [src/plugins/sponsorblock/index.ts:84, 107](../src/plugins/sponsorblock/index.ts) | subscribe | never emitted |
| `peard:restart` / `peard:reload` / `peard:get-path` / `peard:get-downloads-folder` | renderer→main | [src/providers/app-controls.ts](../src/providers/app-controls.ts) | `peard:ipc-invoke` | no handler |
| `get-renderer-script` (sendSync) | renderer→main sync | [electron-legacy/electron_preload_legacy.ts:72-107](../electron-legacy/electron_preload_legacy.ts); [electron-legacy/electron_main_index_legacy.ts:717+](../electron-legacy/electron_main_index_legacy.ts) | stubbed `[null, '']` | no-op by design |
| `setVolume` (MPRIS, linux) | main→renderer | [src/plugins/shortcuts/mpris.ts:314-316](../src/plugins/shortcuts/mpris.ts) | subscribe | never emitted |

**Bottom line:** the port, as committed, is a *renderer bridge with no bridge counterparty*. The Vite renderer bundle plus the shim faithfully reproduce the in-page half of the app — YT control channels that originate and terminate inside the page still work — but everything that made Pear a desktop app (main-process plugins, config persistence, menu/tray, updates, proxy/UA, crash UX) exists only as unregistered Cargo dependencies and comments in a 16-line `lib.rs`.

---

## Addendum 2026-11-10: Renderer plugin boot was killed by Tauri 2's ACL remote-origin gate (FIXED)

**Status of this document:** the tables above record the *early Phase-2 analysis* (pre-implementation,
when lib.rs was a 16-line stub). They do not reflect the completed Rust port: config.rs / ipc.rs /
menu.rs / song_info.rs / tray.rs and the command registration in lib.rs now exist, Rust-side menu
toggles persist, and the channels marked "no handler" are handled by Rust commands or the shim's
local bus. Read the tables as history, not current truth.

### The bug

At runtime the renderer bundle loaded (the YouTube Music UI fully worked) but **no plugin frontend
ever mounted** — no `#ytmd-title-bar-main-panel`, no renderer plugin of any kind, silently, with
zero diagnostics (release builds have no console piping).

Root cause chain:

1. The main window loads `https://music.youtube.com` — a **remote** origin under Tauri's IPC
   origin model (`Webview::on_message`, `is_local = webview.is_local_url(&request.url)`).
2. `build.rs` was a bare `tauri_build::build()` — no `app_manifest`, so **no application ACL
   manifest was generated** (`gen/schemas/acl-manifests.json` had no `__app-acl__` entry;
   `has_app_acl = false`).
3. Tauri 2's dispatch gate (tauri 2.12.5, `src/webview/mod.rs`, `on_message`):
   `if (plugin_command.is_some() || has_app_acl_manifest || !is_local) && … && invoke.acl.is_none()`
   → since the request comes from a remote origin, ACL resolution runs for **every** command;
   `resolve_access("peard_get_config-derived invokes", …)` finds no ResolvedCommand (no permission
   exists for app commands) → `invoke.acl.is_none()` → **reject "Command … not allowed by ACL"**.
   The code comment states it directly: *"remote content can never reach custom commands unless an
   explicit remote capability has been configured for them."*
4. Every shim `invoke()` (getConfig, store-set, subscriptions' echo plumbing) therefore rejected.
   `loadAllRendererPlugins()` → `forceLoadRendererPlugin()` → plugin `onRendererLoad` →
   `await getConfig()` threw, the throw propagated out of the plugin loop, and the whole
   `initObserver().then(preload).then(main)` chain died as an unhandled rejection — **before** the
   `plugin:enable` / `plugin:unload` / `config-changed` listeners in `main()` were ever registered.
   Result: dead renderer plugins, dead live-toggles, zero output. (Native-menu-driven Rust toggles
   still persisted config because they never cross the IPC boundary — which is exactly why the
   smoke tests passed while the page stayed inert.)

### The fix (three layers, precedent in that only the first is strictly required)

1. **`src-tauri/build.rs`**: `tauri_build::try_build(Attributes::new().app_manifest(
   AppManifest::new().commands(&[…13 command names…])))` — autogenerates `allow-<command-slug>`
   (snake_case → kebab-case: `_`→`-`) permissions for every pear bridge command:
   `allow-peard-store-set`, `allow-peard-store-set-partial`, `allow-peard-set-menu-option`,
   `allow-peard-emit`, `allow-peard-subscribe`, `allow-peard-ipc-invoke`, `allow-get-menu`,
   `allow-get-menu-by-id`, `allow-peard-menu-event`, `allow-peard-video-src-changed`,
   `allow-peard-play-or-paused`, `allow-peard-time-changed`, `allow-peard-restart`.
2. **`src-tauri/capabilities/default.json`**: all 13 `allow-*` permissions added to the existing
   capability, which already declares `remote.urls` (`https://*.youtube.com`, `*.google.com`,
   `*.doubleclick.net`, `*.gstatic.com`, `*.ytimg.com`, `*.googlevideo.com`) —
   `resolve_command` (tauri-utils) grants each allowed command a `Remote` execution context per
   remote URL plus `Local` from `capability.local: true`, so the commands are now valid from the
   music.youtube.com page. `UrlPattern` semantics: `https://*.youtube.com` matches hostname
   `music.youtube.com`, empty pathname/search/hash default to `*`.
3. **Defense in depth (renderer)**: `loadAllRendererPlugins` wraps each plugin load in
   try/catch (a single failing plugin can no longer abort the loop), and the
   `initObserver().then(preload).then(main)` chain gained a `.catch` so boot errors always
   surface on console.

Security note: granting the bridge commands to `*.youtube.com`-family origins is the explicit
Tauri-sanctioned equivalent of Electron's "preload runs on the remote page" model — the commands
are the app's own API surface (config store, menu JSON, song-info echo), no shell/fs/network
primitives, and the URL list deliberately mirrors the domains the webview actually loads.
