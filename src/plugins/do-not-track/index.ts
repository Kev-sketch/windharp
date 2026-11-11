/**
 * do-not-track — ad / tracker blocking.
 *
 * IMPORTANT (Tauri build): the Electron implementation in this folder
 * (blocker.ts + the injectors) cannot run here. It depends on
 * @ghostery/adblocker-electron, contextBridge, webFrame and BrowserWindow —
 * none of which exist in a Tauri/WKWebView app. This plugin therefore has no
 * `renderer` block, so the renderer loader never starts it; it is kept only as
 * the archived upstream implementation.
 *
 * The WORKING ad blocker for this build is `assets/pear-adblock.js`, injected
 * at document-start from src-tauri/src/lib.rs. It reads this plugin's
 * `enabled` / `blocker` config out of window.__PEAR_BOOTSTRAP__ directly, so
 * toggling the plugin in the Plugins menu does affect it.
 *
 * Mode note: only "In player" is supported. "With blocklists" needs
 * session-level request interception, which the webview does not expose; the
 * injected script reports that honestly instead of pretending to filter.
 */
import { contextBridge, webFrame, type BrowserWindow } from 'electron';

import { t } from '@/i18n';
import { createPlugin } from '@/utils';

import {
  isBlockerEnabled,
  loadTrackerBlockerEngine,
  unloadTrackerBlockerEngine,
} from './blocker';
import { inject, isInjected } from './injectors/inject';
import injectCliqzPreload from './injectors/inject-cliqz-preload';
import { blockers } from './types';

export interface TrackerBlockerConfig {
  /**
   * Whether to enable the tracker blocker.
   * @default true
   */
  enabled: boolean;
  /**
   * When enabled, the tracker blocker will cache the blocklists.
   * @default true
   */
  cache: boolean;
  /**
   * Which tracker blocker to use.
   * @default blockers.InPlayer
   */
  blocker: (typeof blockers)[keyof typeof blockers];
  /**
   * Additional list of filters to use.
   * @example ["https://raw.githubusercontent.com/uBlockOrigin/uAssets/master/filters/filters.txt"]
   * @default []
   */
  additionalBlockLists: string[];
  /**
   * Disable the default blocklists.
   * @default false
   */
  disableDefaultLists: boolean;
}

export default createPlugin({
  name: () => t('plugins.do-not-track.name'),
  description: () => t('plugins.do-not-track.description'),
  restartNeeded: false,
  config: {
    enabled: false,
    cache: true,
    blocker: blockers.InPlayer,
    additionalBlockLists: [],
    disableDefaultLists: false,
  } as TrackerBlockerConfig,
  menu: async ({ getConfig, setConfig }) => {
    const config = await getConfig();

    return [
      {
        label: t('plugins.do-not-track.menu.blocker'),
        submenu: Object.values(blockers).map((blocker) => ({
          label: blocker,
          type: 'radio',
          checked: (config.blocker || blockers.WithBlocklists) === blocker,
          click() {
            setConfig({ blocker });
          },
        })),
      },
    ];
  },
  backend: {
    mainWindow: null as BrowserWindow | null,
    async start({ getConfig, window }) {
      const config = await getConfig();
      this.mainWindow = window;

      if (config.blocker === blockers.WithBlocklists) {
        await loadTrackerBlockerEngine(
          window.webContents.session,
          config.cache,
          config.additionalBlockLists,
          config.disableDefaultLists,
        );
      }
    },
    stop({ window }) {
      if (isBlockerEnabled(window.webContents.session)) {
        unloadTrackerBlockerEngine(window.webContents.session);
      }
    },
    async onConfigChange(newConfig) {
      if (this.mainWindow) {
        if (
          newConfig.blocker === blockers.WithBlocklists &&
          !isBlockerEnabled(this.mainWindow.webContents.session)
        ) {
          await loadTrackerBlockerEngine(
            this.mainWindow.webContents.session,
            newConfig.cache,
            newConfig.additionalBlockLists,
            newConfig.disableDefaultLists,
          );
        }
      }
    },
  },
  preload: {
    // see #1478
    script: `const _prunerFn = window._pruner;
    window._pruner = undefined;
    JSON.parse = new Proxy(JSON.parse, {
      apply() {
        return _prunerFn(Reflect.apply(...arguments));
      },
    });
    Response.prototype.json = new Proxy(Response.prototype.json, {
      apply() {
        return Reflect.apply(...arguments).then((o) => _prunerFn(o));
      },
    }); 0`,
    async start({ getConfig }) {
      const config = await getConfig();

      if (config.blocker === blockers.InPlayer && !isInjected()) {
        inject(contextBridge);
        await webFrame.executeJavaScript(this.script);
      } else if (config.blocker === blockers.WithBlocklists) {
        await injectCliqzPreload();
      }
    },
    async onConfigChange(newConfig) {
      if (newConfig.blocker === blockers.InPlayer && !isInjected()) {
        inject(contextBridge);
        await webFrame.executeJavaScript(this.script);
      }
    },
  },
});
