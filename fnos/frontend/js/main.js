'use strict';
/*
 * App shell: hash router, theme, polling, dirty-draft save bar and the save
 * flow (PUT /api/config → optional device refresh → protocol-host re-read).
 */
window.FP = window.FP || {};

(function (FP) {
  const u = FP.util;

  const ROUTES = ['overview', 'audio', 'receive', 'settings'];
  const TITLES = { overview: '概览', audio: '音频', receive: '接收', settings: '设置' };
  const THEMES = ['system', 'light', 'dark'];
  const THEME_LABELS = { system: '跟随系统', light: '浅色', dark: '深色' };
  const THEME_ICONS = { system: 'auto', light: 'sun', dark: 'moon' };
  const POLL_MS = 2000;
  const originId = new URLSearchParams(location.search).get('instance');

  const store = FP.createStore();
  const updateArtwork = FP.initGlass();
  const els = {};

  let connected = false;
  let connectionKnown = false;
  let saveError = '';
  let pages = null;
  let currentPage = null;
  let currentRoute = null;
  let pollTimer = null;
  let pollBusy = false;
  let toastTimer = null;

  const ctx = {
    store: store,
    toast: toast,
    isConnected: isConnected,
    isConnectionKnown: function () { return connectionKnown; },
    confirmHostApplied: confirmHostApplied,
    refreshNow: refreshCore
  };

  function el(id) {
    if (!els[id]) els[id] = u.byId(id);
    return els[id];
  }

  // ------------------------------------------------------------------- toast
  function toast(message, type) {
    if (type === 'error' && window.fpRecordBrowserError) window.fpRecordBrowserError(message);
    const node = el('toast');
    if (!node) return;
    node.textContent = message;
    node.className = 'toast ' + (type || 'info');
    node.hidden = false;
    if (toastTimer) clearTimeout(toastTimer);
    const duration = type === 'error' ? 6500 : type === 'warn' ? 5500 : 4000;
    toastTimer = setTimeout(function () {
      node.hidden = true;
      toastTimer = null;
    }, duration);
  }

  // -------------------------------------------------------------- connection
  function isConnected() {
    return connected;
  }

  function setConnected(value) {
    connectionKnown = true;
    if (connected !== value) connected = value;
    updateServiceStatus();
    const banner = el('offline-banner');
    if (banner) banner.hidden = value;
    if (banner && !value) {
      const text = banner.querySelector('span');
      if (text) text.textContent = '无法连接后端服务，正在自动重试…';
    }
  }

  function updateServiceStatus() {
    const wrapper = el('service-status');
    const text = el('service-text');
    if (!wrapper || !text) return;
    wrapper.classList.remove('ok', 'warn', 'bad');
    if (!connectionKnown) {
      u.setText(text, '连接中…');
      wrapper.title = '正在连接后端服务';
      return;
    }
    if (!connected) {
      wrapper.classList.add('bad');
      u.setText(text, '服务不可用');
      wrapper.title = '无法连接后端服务';
      return;
    }
    wrapper.classList.add('ok');
    u.setText(text, '服务正常');
    const version = (store.state.health && store.state.health.version) || '';
    wrapper.title = '后端服务运行正常' + (version ? '（v' + version + '）' : '');
  }

  // -------------------------------------------------------------------- theme
  function readTheme() {
    try {
      const saved = localStorage.getItem('fusionplay.theme');
      return THEMES.indexOf(saved) >= 0 ? saved : 'system';
    } catch (error) {
      return 'system';
    }
  }

  function applyTheme(theme, persist) {
    const next = THEMES.indexOf(theme) >= 0 ? theme : 'system';
    document.documentElement.setAttribute('data-theme', next);
    u.setText(el('theme-label'), THEME_LABELS[next]);
    const button = el('theme-toggle');
    if (button) {
      button.setAttribute('aria-label', '切换主题（当前：' + THEME_LABELS[next] + '）');
      button.title = '主题：' + THEME_LABELS[next] + '（点击切换）';
    }
    const use = el('theme-icon-use');
    if (use) use.setAttribute('href', '#icon-' + THEME_ICONS[next]);
    if (persist) {
      try { localStorage.setItem('fusionplay.theme', next); } catch (error) { /* ignore */ }
    }
    document.dispatchEvent(new CustomEvent('fp:theme'));
  }

  function cycleTheme() {
    const current = readTheme();
    const next = THEMES[(THEMES.indexOf(current) + 1) % THEMES.length];
    applyTheme(next, true);
    toast('主题：' + THEME_LABELS[next], 'info');
  }

  // --------------------------------------------------------------- save bar
  function updateSavebar() {
    const bar = el('savebar');
    if (!bar) return;
    const dirty = store.hasDirty();
    const saving = store.isSaving();
    if (!dirty && !saving) {
      bar.hidden = true;
      document.body.classList.remove('has-savebar');
      return;
    }
    bar.hidden = false;
    document.body.classList.add('has-savebar');
    const wrapper = el('savebar-text');
    const message = el('savebar-message');
    const saveButton = el('savebar-save');
    const undoButton = el('savebar-undo');
    wrapper.classList.remove('saving', 'error');
    if (saving) {
      wrapper.classList.add('saving');
      const scopes = store.dirtyScopes();
      u.setText(message, scopes.receive ? '正在保存并重启接收服务…' : '正在保存并应用…');
      message.title = '';
    } else if (saveError) {
      wrapper.classList.add('error');
      u.setText(message, saveError);
      message.title = saveError;
    } else {
      const scopes = store.dirtyScopes();
      const parts = [];
      if (scopes.audio) parts.push('音频');
      if (scopes.receive) parts.push('接收');
      u.setText(message, '有未保存修改' + (parts.length ? ' · ' + parts.join('、') : ''));
      message.title = '';
    }
    if (saveButton) saveButton.disabled = saving;
    if (undoButton) undoButton.disabled = saving;
  }

  function undoAll() {
    if (store.isSaving()) return;
    saveError = '';
    store.revert();
    if (currentPage) currentPage.render();
    updateSavebar();
    toast('已撤销未保存修改。', 'info');
  }

  // ------------------------------------------------------------------ saving
  function protocolEnabled(config, key) {
    return !!(config && config.protocols && config.protocols[key] && config.protocols[key].enabled);
  }

  function configLooksSaved(recovered, payload) {
    return u.deepEqual(recovered.receiver, payload.receiver) &&
      u.deepEqual(recovered.protocols, payload.protocols) &&
      u.deepEqual(recovered.audio, payload.audio);
  }

  function confirmHostApplied(expectedConfig, timeoutMs) {
    const expected = {
      name: String((expectedConfig && expectedConfig.receiver && expectedConfig.receiver.name) || '').trim(),
      interface: String((expectedConfig && expectedConfig.receiver && expectedConfig.receiver.interface) || ''),
      protocols: {}
    };
    FP.PROTOCOLS.forEach(function (meta) {
      expected.protocols[meta.key] = protocolEnabled(expectedConfig, meta.key);
    });
    const deadline = Date.now() + (timeoutMs || 7000);
    let reason = '未在预期时间内确认生效';
    return (async function loop() {
      while (Date.now() < deadline) {
        await u.delay(500);
        try {
          const results = await Promise.all([FP.api.health(), FP.api.config(), FP.api.status()]);
          const health = results[0];
          const config = results[1];
          const status = results[2];
          store.applyRemote({ health: health, config: config, status: status });
          const problems = [];
          const warnings = [];
          if (!health || health.status !== 'ok') problems.push('健康检查未返回 ok');
          if (!config) {
            problems.push('配置回读失败');
          } else {
            const name = String((config.receiver && config.receiver.name) || '').trim();
            const iface = String((config.receiver && config.receiver.interface) || '');
            if (name !== expected.name) problems.push('接收器名称未生效');
            if (iface !== expected.interface) problems.push('网络接口未生效');
          }
          FP.PROTOCOLS.forEach(function (meta) {
            const want = expected.protocols[meta.key];
            const configured = protocolEnabled(config, meta.key);
            if (configured !== want) problems.push(meta.label + '开关未生效');
            const runtime = status && status.protocols ? status.protocols[meta.key] : null;
            if (runtime) {
              if (!!runtime.enabled !== want) problems.push(meta.label + '运行时状态未更新');
              if (want && (runtime.state === 'error' || runtime.state === 'failed')) {
                warnings.push(meta.label + '运行异常（详见设置页日志）');
              }
            }
          });
          if (!problems.length) return { ok: true, warnings: warnings };
          reason = problems.join('；');
        } catch (error) {
          reason = '协议宿主重启期间服务不可达：' + error.message;
        }
      }
      return { ok: false, reason: reason, warnings: [] };
    })();
  }

  async function saveAll() {
    if (store.isSaving()) return;
    const validation = store.validate();
    if (!validation.ok) {
      saveError = validation.message;
      updateSavebar();
      toast(validation.message, 'error');
      return;
    }
    const scopes = store.dirtyScopes();
    const payload = store.buildSavePayload();
    if (!payload) return;
    saveError = '';
    store.setSaving(true);
    updateSavebar();
    let refreshWarning = '';
    try {
      const response = await FP.api.saveConfig(payload);
      const saved = response && response.config ? response.config : payload;
      store.adoptConfig(saved);

      if (scopes.audio) {
        try {
          await FP.api.refreshDevices();
        } catch (error) {
          refreshWarning = '音频引擎重建失败：' + error.message;
        }
        try {
          const discovery = await FP.api.devices();
          store.setDiscovered((discovery && discovery.devices) || []);
        } catch (error) { /* keep previous list */ }
      }

      try {
        const results = await Promise.all([FP.api.status(), FP.api.config()]);
        store.applyRemote({ status: results[0], config: results[1] });
      } catch (error) { /* poll will recover */ }

      const messages = [];
      if (refreshWarning) messages.push(refreshWarning);
      if (scopes.receive) {
        const check = await confirmHostApplied(saved, 7000);
        if (!check.ok) messages.push('协议宿主未在预期时间内确认生效（' + check.reason + '）');
        if (check.warnings && check.warnings.length) messages.push(check.warnings.join('；'));
      }
      if (messages.length) toast('已保存，但：' + messages.join('；'), 'warn');
      else toast(scopes.receive ? '已保存并生效（协议宿主已重启）。' : '已保存并应用。', 'ok');
    } catch (error) {
      // The host writes config.json before restarting protocol hosts; a failed
      // restart returns HTTP 400 after the config was already persisted.
      let recovered = null;
      try {
        const config = await FP.api.config();
        if (config && configLooksSaved(config, payload)) recovered = config;
      } catch (readError) { /* still offline */ }
      if (recovered) {
        store.adoptConfig(recovered);
        saveError = '';
        toast('配置已写入，但保存请求返回错误：' + error.message +
          '。协议宿主可能未生效，请查看「设置 → 高级诊断」日志。', 'warn');
      } else {
        saveError = '保存失败：' + error.message;
        toast('保存失败：' + error.message + '（草稿已保留）', 'error');
      }
    } finally {
      store.setSaving(false);
      updateSavebar();
      if (currentPage) currentPage.render();
    }
  }

  // ----------------------------------------------------------------- polling
  async function refreshCore() {
    if (pollBusy || store.isSaving()) return;
    pollBusy = true;
    try {
      const fetches = [FP.api.health(), FP.api.status(), FP.api.config()];
      const dedicated = originId && originId !== 'default';
      if (dedicated && /^[a-z0-9-]{1,40}$/.test(originId)) fetches.push(FP.api.status(originId));
      const results = await Promise.allSettled(fetches);
      const health = results[0].status === 'fulfilled' ? results[0].value : null;
      const status = results[1].status === 'fulfilled' ? results[1].value : null;
      const config = results[2].status === 'fulfilled' ? results[2].value : null;
      const scene = dedicated ? results[3]?.value : status;
      updateArtwork(health ? scene : null, dedicated ? originId : null);
      if (!health) {
        setConnected(false);
      } else {
        const payload = { health: health };
        if (status) payload.status = status;
        if (config) payload.config = config;
        store.applyRemote(payload);
        setConnected(true);
      }
      if (currentPage) currentPage.render();
    } catch (error) {
      setConnected(false);
    } finally {
      pollBusy = false;
    }
  }

  // ------------------------------------------------------------------ router
  function routeFromHash() {
    const raw = String(location.hash || '').replace(/^#\/?/, '');
    return ROUTES.indexOf(raw) >= 0 ? raw : 'overview';
  }

  function navigate() {
    const route = routeFromHash();
    if (location.hash !== '#/' + route) {
      try {
        history.replaceState(null, '', '#/' + route);
      } catch (error) { /* ignore */ }
    }
    if (route !== currentRoute) {
      if (currentPage && currentPage.leave) currentPage.leave();
      currentRoute = route;
      currentPage = pages[route] || null;
    }
    ROUTES.forEach(function (name) {
      const section = el('page-' + name);
      if (section) section.hidden = name !== route;
    });
    u.qsa('.nav-link').forEach(function (link) {
      if (link.dataset.route === route) link.setAttribute('aria-current', 'page');
      else link.removeAttribute('aria-current');
    });
    document.title = TITLES[route] + ' · FusionPlay';
    if (currentPage) currentPage.enter();
    updateSavebar();
  }

  // -------------------------------------------------------------------- init
  function bind() {
    el('theme-toggle').addEventListener('click', cycleTheme);
    el('savebar-save').addEventListener('click', saveAll);
    el('savebar-undo').addEventListener('click', undoAll);
    el('offline-retry').addEventListener('click', function () { refreshCore(); });
    const skipLink = document.querySelector('.skip-link');
    if (skipLink) {
      skipLink.addEventListener('click', function (event) {
        event.preventDefault();
        const main = el('main');
        if (main) main.focus();
      });
    }
    window.addEventListener('hashchange', navigate);
    document.addEventListener('visibilitychange', function () {
      if (!document.hidden) refreshCore();
    });
    store.subscribe(function (reason) {
      if (reason === 'dirty' || reason === 'revert') saveError = '';
      updateSavebar();
    });
    if (window.matchMedia) {
      const media = window.matchMedia('(prefers-color-scheme: dark)');
      const listener = function () {
        if (readTheme() === 'system') document.dispatchEvent(new CustomEvent('fp:theme'));
      };
      if (media.addEventListener) media.addEventListener('change', listener);
      else if (media.addListener) media.addListener(listener);
    }
  }

  function init() {
    el('return-player').href = FP.playerUrl(originId);
    store.subscribe(function () {
      const base = store.state.baseline;
      if (!base || !originId || originId === 'default') return;
      const instance = (base.audio.instances || []).find(item => item.id === originId);
      el('return-player').href = FP.playerUrl(instance && instance.id);
      const mainActive = FP.primaryActive(base.audio);
      u.setText(el('return-player'), instance ? '返回' + instance.name : mainActive ? '返回主播放器' : '返回播放器');
    });
    applyTheme(readTheme(), false);
    bind();
    pages = {
      overview: FP.createOverviewPage(ctx),
      audio: FP.createAudioPage(ctx),
      receive: FP.createReceivePage(ctx),
      settings: FP.createSettingsPage(ctx)
    };
    updateServiceStatus();
    navigate();
    updateSavebar();
    refreshCore();
    pollTimer = setInterval(function () {
      if (document.hidden) return;
      refreshCore();
    }, POLL_MS);
  }

  init();
})(window.FP);
