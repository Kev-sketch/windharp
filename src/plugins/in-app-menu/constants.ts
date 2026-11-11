export interface InAppMenuConfig {
  enabled: boolean;
  hideDOMWindowControls: boolean;
}
// Platform authority: prefer a real Electron `process`; in the Tauri webview
// there is none, so fall back to the user agent. Without this fallback the
// darwin check was vacuously true (`undefined !== 'darwin'`), which injected
// the in-app TitleBar on macOS under Tauri — an orphaned hamburger in the
// 74px traffic-light reserve squishing the UI (REVIEW-FINDINGS.md appendix,
// 2026-11-10). macOS has the native menu bar; the injected strip is redundant.
const userAgentMac =
  typeof window !== 'undefined' &&
  !!window.navigator?.userAgent?.toLowerCase().includes('mac');
const userAgentLinux =
  typeof window !== 'undefined' &&
  !!window.navigator?.userAgent?.toLowerCase().includes('linux');
const platform =
  typeof global !== 'undefined' ? global.process?.platform : undefined;

const notDarwin =
  platform !== undefined ? platform !== 'darwin' : !userAgentMac;
const notLinux =
  platform !== undefined ? platform !== 'linux' : !userAgentLinux;

export const defaultInAppMenuConfig: InAppMenuConfig = {
  enabled: notDarwin && notLinux,
  hideDOMWindowControls: false,
};
