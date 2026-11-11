'use strict';
/*
 * Pear runtime shim for the Tauri webview.
 *
 * The legacy app was Electron: the renderer bundle (42 plugins + providers)
 * spoke to the main process through window.ipcRenderer, window.mainConfig
 * and window.electronIs. In the Tauri rebuild the main process is Rust and
 * this shim, injected before the renderer IIFE, reproduces that surface on
 * top of:
 *   - a config snapshot embedded by src-tauri at window creation,
 *   - Tauri IPC (window.__TAURI_INTERNALS__),
 *   - a local event bus with Rust forwarding.
 */
(function () {
  var W = window;
  var internals = W.__TAURI_INTERNALS__ || {};
  var invoke = function (cmd, args) {
    // Rust command names cannot contain ':' — normalize 'peard:x-y' to
    // 'peard_x_y'. Plugin-prefixed commands ('plugin:event|listen') are
    // untouched: Tauri routes those to the owning plugin automatically.
    if (cmd.indexOf('peard:') === 0) {
      cmd = 'peard_' + cmd.slice(6).replace(/-/g, '_');
    }
    return internals.invoke(cmd, args || {});
  };

  var BOOT = W.__PEAR_BOOTSTRAP__ || {};
  var DEV = !!BOOT.dev;
  var storeConfig = BOOT.config || {};
  var pluginNames = BOOT.pluginNames || [];

  // Plugin stubs are published by the renderer bundle itself once it loads
  // (renderer.ts hydrates this before any plugin starts).
  var STUB = '__PEAR_PLUGIN_STUBS__';

  // ---------------------------------------------------------------- helpers

  function clone(v) {
    if (v === null || v === undefined) return v;
    return JSON.parse(JSON.stringify(v));
  }

  function getIn(obj, path) {
    var cur = obj;
    var parts = String(path).split('.');
    for (var i = 0; i < parts.length; i++) {
      if (cur === null || cur === undefined || typeof cur !== 'object') {
        return undefined;
      }
      cur = cur[parts[i]];
    }
    return cur;
  }

  function isPlainObject(o) {
    return o !== null && typeof o === 'object' &&
      Object.prototype.toString.call(o) === '[object Object]';
  }

  /* deepmerge-ts semantics with mergeArrays:false: plain objects merge
   * recursively; arrays and primitives are replaced. Later arguments win. */
  function deepmerge() {
    var args = Array.prototype.slice.call(arguments);
    var out = isPlainObject(args[0]) ? {} : (Array.isArray(args[0]) ? [] : args[0]);
    var isArrayResult = Array.isArray(out);
    for (var i = 0; i < args.length; i++) {
      var src = args[i];
      if (!isPlainObject(src)) continue;
      if (isArrayResult) continue;
      var keys = Object.keys(src);
      for (var k = 0; k < keys.length; k++) {
        var key = keys[k];
        var val = src[key];
        if (isPlainObject(val) && isPlainObject(out[key])) {
          out[key] = deepmerge(out[key], val);
        } else if (Array.isArray(val)) {
          out[key] = clone(val); // mergeArrays:false -> replace
        } else {
          out[key] = val === undefined ? undefined : clone(val);
        }
      }
    }
    return out;
  }

  // ------------------------------------------------------------ config api

  var WATCHERS = [];

  function notifyWatchers() {
    var snap = clone(storeConfig);
    for (var i = 0; i < WATCHERS.length; i++) {
      try { WATCHERS[i](clone(storeConfig), undefined); }
      catch (err) { console.error('[pear] config watcher error', err); }
    }
    return snap;
  }

  function getConfig(key) {
    return clone(getIn(storeConfig, key));
  }

  function setConfigRaw(key, value) {
    var parts = String(key).split('.');
    var cur = storeConfig;
    for (var i = 0; i < parts.length - 1; i++) {
      if (!isPlainObject(cur[parts[i]])) cur[parts[i]] = {};
      cur = cur[parts[i]];
    }
    var last = parts[parts.length - 1];
    if (value === undefined) delete cur[last];
    else cur[last] = clone(value);
  }

  function getStub(plugin) {
    var stubs = W[STUB] || {};
    return stubs[plugin];
  }

  function mergedPluginConfig(plugin) {
    var stubCfg = (getStub(plugin) || {}).config;
    var stored = ((storeConfig.plugins || {})[plugin]) || {};
    return deepmerge(isPlainObject(stubCfg) ? stubCfg : {}, stored);
  }

  var configApi = {
    get: function (key) { return getConfig(key); },
    set: function (key, value) {
      setConfigRaw(key, value);
      return invoke('peard:store-set', { key: key, value: value });
    },
    setPartial: function (key, value, defaultValue) {
      return invoke('peard:store-set-partial', {
        key: key,
        value: value,
        defaultValue: defaultValue,
      });
    },
    setMenuOption: function (key, value) {
      return invoke('peard:set-menu-option', { key: key, value: value });
    },
    watch: function (cb) {
      WATCHERS.push(cb);
      return function () {
        var idx = WATCHERS.indexOf(cb);
        if (idx !== -1) WATCHERS.splice(idx, 1);
      };
    },
  };
  Object.defineProperty(configApi, 'defaultConfig', {
    get: function () { return clone(getStub('__defaults__') || {}); },
    enumerable: true,
  });

  var pluginsApi = {
    getPlugins: function () { return clone(storeConfig.plugins || {}); },
    getOptions: function (plugin) { return mergedPluginConfig(plugin); },
    setOptions: function (plugin, options, exclude) {
      var opts = clone(options) || {};
      var ex = exclude === undefined ? ['enabled'] : exclude;
      for (var i = 0; i < ex.length; i++) delete opts[ex[i]];
      var stored = (storeConfig.plugins || {})[plugin] || {};
      var merged = deepmerge(stored, opts);
      setConfigRaw('plugins.' + plugin, merged);
      // F-5a — write the per-plugin key instead of the whole 'plugins' map
      // so the Rust side sees a `plugins.<id>` root change and can emit
      // plugin:enable / plugin:unload like the legacy per-id watcher
      // (electron_main_index_legacy.ts:209-243). The bare-map write is
      // still handled server-side for any direct store.set('plugins', …).
      return invoke('peard:store-set', {
        key: 'plugins.' + plugin,
        value: merged,
      });
    },
    setMenuOptions: function (plugin, options, exclude) {
      return pluginsApi.setOptions(plugin, options, exclude)
        .then(function () {
          if (getIn(storeConfig, 'options.restartOnConfigChanges')) {
            return invoke('peard:restart', {});
          }
          return null;
        });
    },
    enable: function (plugin) {
      return pluginsApi.setMenuOptions(plugin, { enabled: true }, []);
    },
    disable: function (plugin) {
      return pluginsApi.setMenuOptions(plugin, { enabled: false }, []);
    },
    isEnabled: function (plugin) {
      return !!mergedPluginConfig(plugin).enabled;
    },
  };

  var mainConfig = configApi;
  mainConfig.plugins = pluginsApi;

  // ---------------------------------------------------------------- ipc bus

  var LISTENERS = {};   // channel -> array of { fn, wantsEvent }
  var SUBSCRIBED = {};  // channel -> bool

  function fakeEvent() {
    return {
      sender: { send: function () {} },
      preventDefault: function () {},
      stopPropagation: function () {},
    };
  }

  function ensureSubscription(channel) {
    if (SUBSCRIBED[channel]) return;
    SUBSCRIBED[channel] = true;
    invoke('peard:subscribe', { channel: channel }).catch(function () {
      SUBSCRIBED[channel] = false;
    });
  }

  function dispatchLocal(channel, args) {
    var list = LISTENERS[channel];
    if (!list) return;
    var ev = fakeEvent();
    for (var i = 0; i < list.length; i++) {
      try {
        list[i].fn.apply(list[i].ctx || null, [ev].concat(args || []));
      } catch (err) {
        console.error('[pear] ipc listener error on', channel, err);
      }
    }
  }

  function tauriListen(eventName, handler) {
    var cbId = internals.transformCallback(handler);
    return invoke('plugin:event|listen', {
      event: eventName,
      target: { kind: 'Any' },
      handler: { __TAURI_CHANNEL__: cbId },
    });
  }

  var ipcRenderer = {
    on: function (channel, listener) {
      if (!LISTENERS[channel]) LISTENERS[channel] = [];
      LISTENERS[channel].push({ fn: listener, ctx: this });
      ensureSubscription(channel);
      return ipcRenderer;
    },
    once: function (channel, listener) {
      var fired = false;
      var wrapper = function () {
        if (fired) return;
        fired = true;
        ipcRenderer.removeListener(channel, wrapper);
        listener.apply(null, Array.prototype.slice.call(arguments));
      };
      wrapper.__pearOnce = listener;
      if (!LISTENERS[channel]) LISTENERS[channel] = [];
      LISTENERS[channel].push({ fn: wrapper, ctx: this });
      ensureSubscription(channel);
      return ipcRenderer;
    },
    off: function (channel, listener) {
      return ipcRenderer.removeListener(channel, listener);
    },
    removeListener: function (channel, listener) {
      var list = LISTENERS[channel];
      if (!list) return ipcRenderer;
      for (var i = list.length - 1; i >= 0; i--) {
        if (list[i].fn === listener || list[i].fn.__pearOnce === listener) {
          list.splice(i, 1);
        }
      }
      return ipcRenderer;
    },
    removeAllListeners: function (channel) {
      if (channel === undefined) {
        LISTENERS = {};
      } else {
        delete LISTENERS[channel];
      }
      return ipcRenderer;
    },
    send: function (channel) {
      var args = Array.prototype.slice.call(arguments, 1);
      invoke('peard:emit', { channel: channel, args: args }).catch(function (err) {
        console.warn('[pear] send failed on', channel, err);
      });
    },
    sendSync: function () {
      return [null, ''];
    },
    sendToHost: function () {
      ipcRenderer.send.apply(ipcRenderer, Array.prototype.slice.call(arguments));
    },
    invoke: function (channel) {
      // Everything — including peard:get-config / peard:set-config — goes to
      // Rust, which owns the store and the per-plugin default-config manifest.
      var args = Array.prototype.slice.call(arguments, 1);
      if (channel === 'peard:get-main-plugin-names') {
        return Promise.resolve(pluginNames.slice());
      }
      return invoke('peard:ipc-invoke', { channel: channel, args: args });
    },
  };

  // ------------------------------------------------ rust -> page plumbing

  tauriListen('pear-ipc-event', function (payload) {
    if (payload && typeof payload.channel === 'string') {
      dispatchLocal(payload.channel, payload.args || []);
    }
  }).catch(function (err) {
    console.warn('[pear] event pipe registration failed', err);
  });

  tauriListen('pear-config-changed', function (payload) {
    if (payload && isPlainObject(payload.snapshot)) {
      var prev = storeConfig;
      storeConfig = payload.snapshot;
      var snap = clone(storeConfig);
      for (var i = 0; i < WATCHERS.length; i++) {
        try { WATCHERS[i](snap, prev); }
        catch (err) { console.error('[pear] config watcher error', err); }
      }
    }
  }).catch(function (err) {
    console.warn('[pear] config pipe registration failed', err);
  });

  // ------------------------------------------------------------------ misc

  var isMac = navigator.platform === 'MacIntel' ||
    /Macintosh/.test(navigator.userAgent);
  var electronIs = {
    windows: function () { return false; },
    osx: function () { return isMac; },
    macOS: function () { return isMac; },
    linux: function () { return false; },
    freebsd: function () { return false; },
    dev: function () { return DEV; },
    production: function () { return !DEV; },
    main: function () { return false; },
    renderer: function () { return true; },
    localhost: function () { return DEV; },
  };

  // Install everything on BOTH window and globalThis: legacy code uses both.
  W.ipcRenderer = ipcRenderer;
  W.mainConfig = mainConfig;
  W.electronIs = electronIs;
  W.ELECTRON_RENDERER_URL = DEV ? (BOOT.devUrl || 'http://localhost:5173') : undefined;
  W.reload = function () { location.reload(); };
  W._lact = Date.now();
  W.download = function () { console.warn('[pear] download() is not ported to Tauri'); };
  if (W.globalThis) {
    W.globalThis.electronIs = electronIs;
  }

  // Page guard: remove YouTube's ES5 custom-elements adapter so Polymer 2
  // custom elements can be patched by plugins (legacy behavior).
  function stripEs5Adapter() {
    try {
      var scripts = document.querySelectorAll('script[src]');
      for (var i = 0; i < scripts.length; i++) {
        var src = scripts[i].src || '';
        if (src.indexOf('custom-elements-es5-adapter') !== -1) {
          scripts[i].remove();
        }
      }
    } catch (err) { /* page not ready yet */ }
  }
  stripEs5Adapter();
  var es5Observer = new MutationObserver(function () { stripEs5Adapter(); });
  if (document.documentElement) {
    es5Observer.observe(document.documentElement, { childList: true, subtree: true });
  }

  // Expose dispatch for debugging / tests.
  W.__PEAR_DISPATCH__ = dispatchLocal;

  console.debug('[pear] shim installed');
})();
