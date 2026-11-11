<div align="center">

# :wind_chime: Windharp

**A lightweight, fast unofficial YouTube Music client — rebuilt on Tauri 2 + Rust.**

</div>

> [!IMPORTANT]
> ⚠️ Disclaimer
>
> **No Affiliation**
>
> This project, and its contributors, are not affiliated with, authorized by, endorsed by, or in any way officially connected with Google LLC, YouTube, or any of their subsidiaries or affiliates. **This is an independent, non-profit, and unofficial client developed by volunteers with the goal of providing a desktop experience.**
>
> **Trademarks**
>
> The names "Google" and "YouTube Music", as well as related names, marks, emblems, and images, are registered trademarks of their respective owners. Any use of these trademarks is for identification and reference purposes only and does not imply any association with the trademark holder. We have no intention of infringing upon these trademarks or causing harm to the trademark holders.
>
> **Limitation of Liability**
>
> This application is provided "AS IS", and you use it at your own risk. In no event shall the developers or contributors be liable for any claim, damages, or other liability, including any legal consequences, arising from, out of, or in connection with the software or the use or other dealings in the software. The responsibility for any and all outcomes of using this software rests entirely with the user.

## About

Windharp loads the YouTube Music web player in a native window backed by a
small Rust core that provides the window, tray, menu, config store and IPC
bridge — without shipping Chromium + Node.js, so the app is a fraction of the
size and uses far less memory.

The renderer is SolidJS, bundled by Vite, and runs inside the system webview
(macOS WKWebView on Apple silicon and Intel). A compatibility shim maps the
renderer's IPC calls onto Tauri commands, which is what lets the existing
plugin ecosystem, styles and translations work unchanged.

## Build from source

Requirements: Node 22+, pnpm 9+, Rust toolchain, and the
[Tauri 2 prerequisites](https://tauri.app/start/prerequisites/)
(platform webview; on macOS that's just Xcode CLT).

```bash
git clone https://github.com/Kev-sketch/windharp
cd windharp
pnpm install --frozen-lockfile

# Development (vite + rust, hot reload)
pnpm tauri:dev

# Production build (app + dmg)
pnpm tauri:build
```

The built app lands in `src-tauri/target/release/bundle/`.

## What runs where

| Layer | Tech | Notes |
|---|---|---|
| Window/tray/menu/config/IPC | Rust (`src-tauri/`) | Tauri 2.12, tray icon, native menu, JSON config store |
| Renderer | SolidJS + mdui (`src/`) | Bundled by Vite |
| Bridge | Compatibility shim | Maps renderer IPC calls onto Tauri commands |
| Legacy reference code | `electron-legacy/` | Archived, not built; kept for reference |

## Signing in

Sign-in works inside the app: Windharp presents the User-Agent of the webview
engine it actually runs (WebKit on macOS), which is what Google's embedded
browser check requires.

## Themes

You can load CSS files to change the look of the application (Options > Visual
Tweaks > Themes). Some predefined themes are available at
https://github.com/kerichdev/themes-for-ytmdesktop-player.

## Ad blocking

In-player ad blocking is available via Plugins > do-not-track ("In player"
mode). Network-level blocklist filtering is not available in this build,
because the system webview does not expose request interception.

## Attribution

Windharp is a fork of two MIT-licensed community projects, preserved in this
codebase's history:

- **[pear-desktop](https://github.com/pear-devs/pear-desktop)** (pear-devs) — the Electron app this repository was forked from
- **[youtube-music](https://github.com/th-ch/youtube-music)** (th-ch) — the original YouTube Music desktop app

All credit for the original design, plugin ecosystem, and renderer goes to
their contributors.

## License

MIT — see [license](./license). © original authors th-ch / pear-devs;
Windharp modifications © Kev-sketch.
