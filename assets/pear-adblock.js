/*
 * Windharp ad blocker — in-page ("In player") implementation for the Tauri
 * webview.
 *
 * History: this app descends from an Electron project whose do-not-track
 * plugin blocked ads at the *session* layer via @ghostery/adblocker-electron
 * (ElectronBlocker.enableBlockingInSession) plus an Electron `preload` script
 * that used contextBridge. Neither exists in a Tauri/WKWebView build: there is
 * no Electron session, no contextBridge and no webRequest hook. That plugin
 * therefore shipped as dead code — registered and toggleable, but never doing
 * any blocking.
 *
 * This file is the working replacement for the default mode. It runs at
 * document-start (Tauri initialization_script), before YouTube's own scripts,
 * and removes the ad payloads from player responses as they are parsed.
 *
 * Scope: "In player" mode only. Network-level blocklist filtering
 * ("With blocklists") needs webRequest/session interception that the Tauri
 * webview does not expose; that mode remains unsupported rather than silently
 * pretending to work.
 *
 * Safety: every step is individually guarded. The page must never break
 * because of this script, so any failure degrades to "no blocking".
 */
(function () {
  'use strict';

  try {
    var BOOT =
      (window.__PEAR_BOOTSTRAP__ && window.__PEAR_BOOTSTRAP__.config) || {};
    var pluginCfg =
      (BOOT.plugins && BOOT.plugins['do-not-track']) || {};

    // Default-on safety: the plugin's own manifest default is disabled, so an
    // absent key must mean "do nothing".
    if (!pluginCfg || pluginCfg.enabled !== true) return;

    var mode = pluginCfg.blocker || 'In player';
    if (mode !== 'In player') {
      // "With blocklists" requires network interception: not available here.
      console.info(
        '[pear] do-not-track: "%s" mode is not supported in the Tauri build; ' +
          'no network-level blocking was applied',
        mode,
      );
      return;
    }

    // Ad payload keys carried on player responses / ytInitialData.
    var AD_KEYS = ['playerAds', 'adPlacements', 'adSlots'];

    var prune = function (obj, depth) {
      if (!obj || typeof obj !== 'object') return obj;
      if (depth > 3) return obj;

      try {
        for (var i = 0; i < AD_KEYS.length; i += 1) {
          var key = AD_KEYS[i];
          if (key in obj) {
            try {
              delete obj[key];
            } catch (e) {
              /* non-configurable: leave it */
            }
          }
        }
      } catch (e) {}

      // The nested wrappers YouTube also uses.
      try {
        if (obj.playerResponse && typeof obj.playerResponse === 'object') {
          prune(obj.playerResponse, depth + 1);
        }
      } catch (e) {}
      try {
        if (
          obj.ytInitialPlayerResponse &&
          typeof obj.ytInitialPlayerResponse === 'object'
        ) {
          prune(obj.ytInitialPlayerResponse, depth + 1);
        }
      } catch (e) {}

      return obj;
    };

    var installed = 0;

    // 1) Trap the global so a late assignment is pruned on write, and reads
    //    always see a cleaned object.
    try {
      var current;
      var existing = Object.getOwnPropertyDescriptor(
        window,
        'ytInitialPlayerResponse',
      );
      if (existing && existing.value && typeof existing.value === 'object') {
        // Already assigned by an inline script: prune in place.
        prune(existing.value, 0);
        current = existing.value;
      }
      if (!existing || existing.configurable) {
        Object.defineProperty(window, 'ytInitialPlayerResponse', {
          configurable: true,
          get: function () {
            return current;
          },
          set: function (value) {
            current = prune(value, 0);
          },
        });
        installed += 1;
      }
    } catch (e) {}

    // 2) JSON.parse — the player response and bootstrap data both arrive this
    //    way, including from the inline <script> config.
    try {
      var nativeParse = JSON.parse;
      JSON.parse = function () {
        var parsed = nativeParse.apply(this, arguments);
        try {
          return prune(parsed, 0);
        } catch (e) {
          return parsed;
        }
      };
      // Keep the original accessible/identical-looking; some sites compare it.
      try {
        Object.defineProperty(JSON.parse, 'name', { value: 'parse' });
      } catch (e) {}
      installed += 1;
    } catch (e) {}

    // 3) fetch() responses read via .json() (the /youtubei/v1/player calls).
    try {
      var ResponseProto = window.Response && window.Response.prototype;
      var nativeJson = ResponseProto && ResponseProto.json;
      if (nativeJson) {
        ResponseProto.json = function () {
          return nativeJson.apply(this, arguments).then(function (body) {
            try {
              return prune(body, 0);
            } catch (e) {
              return body;
            }
          });
        };
        installed += 1;
      }
    } catch (e) {}

    // 4) XHR responses (older surfaces still use these).
    try {
      var XHR = window.XMLHttpRequest;
      var nativeSend = XHR && XHR.prototype && XHR.prototype.send;
      if (nativeSend) {
        XHR.prototype.send = function () {
          var self = this;
          self.addEventListener('readystatechange', function () {
            if (self.readyState !== 4) return;
            try {
              if (self.responseType === 'json' && self.response) {
                prune(self.response, 0);
              } else if (self.responseType === '' || self.responseType === 'text') {
                var text = self.responseText;
                if (text && text.indexOf('"ad') !== -1) {
                  // Do not rewrite the DOM-visible text; only clean if the
                  // caller would parse it. Left intentionally conservative.
                }
              }
            } catch (e) {}
          });
          return nativeSend.apply(this, arguments);
        };
        installed += 1;
      }
    } catch (e) {}

    console.info(
      '[pear] do-not-track: in-player ad blocking active (%d hooks)',
      installed,
    );
  } catch (error) {
    // Never let the blocker take the page down.
    console.error('[pear] do-not-track: init failed', error);
  }
})();
