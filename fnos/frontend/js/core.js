'use strict';
/*
 * FusionPlay fnOS management UI — shared helpers and HTTP API layer.
 *
 * Everything is provided by the local Rust host (see fnos/ARCHITECTURE.md and
 * fnos/backend/src/http.rs). No demo data, no fabricated progress. The UI works
 * both at the site root (development) and behind the fnOS gateway prefix
 * (`/app/fusionplay-fnos/`): all API URLs are derived from location.pathname.
 */
window.FP = window.FP || {};

(function (FP) {
  // Gateway prefix: `/app/fusionplay-fnos` behind fnOS, '' when served at root.
  const GATEWAY_BASE = (function () {
    let path = location.pathname || '/';
    path = path.replace(/\/(?:index|speakers|update)\.html$/i, '');
    if (path.length > 1) {
      path = path.replace(/\/+$/, '');
    }
    return path === '/' || path === '' ? '' : path;
  })();

  function clamp(value, low, high) {
    return Math.min(high, Math.max(low, value));
  }

  function round(value, digits) {
    const factor = Math.pow(10, digits == null ? 1 : digits);
    return Math.round(Number(value) * factor) / factor;
  }

  function finite(value, fallback) {
    const number = Number(value);
    return Number.isFinite(number) ? number : fallback;
  }

  function apiUrl(path) {
    if (!path) return GATEWAY_BASE || '/';
    const clean = path.charAt(0) === '/' ? path : '/' + path;
    return GATEWAY_BASE + clean;
  }

  function request(method, path, body, timeoutMs) {
    const controller = typeof AbortController !== 'undefined' ? new AbortController() : null;
    const timeout = timeoutMs || 10000;
    let timer = null;
    if (controller) {
      timer = setTimeout(function () { controller.abort(); }, timeout);
    }
    const options = { method: method, cache: 'no-store', headers: { Accept: 'application/json' } };
    if (controller) options.signal = controller.signal;
    if (body !== undefined) {
      options.headers['Content-Type'] = 'application/json';
      options.body = JSON.stringify(body);
    }
    return fetch(apiUrl(path), options).then(function (response) {
      return response.text().then(function (text) {
        let data = null;
        if (text) {
          try { data = JSON.parse(text); } catch (error) { data = null; }
        }
        if (!response.ok) {
          const message = data && (data.error || data.message)
            ? (data.error || data.message)
            : 'HTTP ' + response.status;
          const failure = new Error(message);
          failure.status = response.status;
          failure.data = data;
          throw failure;
        }
        return data;
      });
    }).catch(function (error) {
      if (error && error.name === 'AbortError') {
        throw new Error('请求超时（' + Math.round(timeout / 1000) + ' 秒）');
      }
      if (error instanceof TypeError && /fetch|network|load/i.test(error.message || '')) {
        throw new Error('无法连接后端服务');
      }
      throw error;
    }).finally(function () {
      if (timer) clearTimeout(timer);
    });
  }

  const api = {
    saveOutputs: (original, audio) => request('PUT', '/api/outputs', {original, audio}, 45000),
    instances: () => request('GET', '/api/instances'),
    createSpeaker: (name, device_id) => request('POST', '/api/instances', {name, device_id}, 45000),
    editSpeaker: (id, binding) => request('PUT', '/api/instances/' + encodeURIComponent(id) + '/binding', binding, 45000),
    saveSpeakerCurve: (id, edit) => request('PUT', '/api/instances/' + encodeURIComponent(id) + '/curve', edit, 45000),
    status: function (instance) {
      const path = instance ? '/api/instances/' + encodeURIComponent(instance) + '/status' : '/api/status';
      return request('GET', path, undefined, 9000);
    },
    config: function () { return request('GET', '/api/config', undefined, 9000); },
    devices: function () { return request('GET', '/api/devices', undefined, 10000); },
    refreshDevices: function () { return request('POST', '/api/devices/refresh', {}, 25000); },
    testTone: function (id) { return request('POST', '/api/devices/test', { id: id }, 12000); },
  };
  FP.confirmAction = (message, action) => {
    const dialog = document.getElementById('confirm-action');
    if (dialog.open) return Promise.resolve(false);
    document.getElementById('confirm-title').textContent = message;
    document.getElementById('confirm-accept').textContent = action;
    dialog.returnValue = 'cancel';
    const result = new Promise(resolve => dialog.addEventListener('close',
      () => resolve(dialog.returnValue === 'confirm'), {once:true}));
    dialog.showModal();
    return result;
  };
  FP.apiUrl = apiUrl;
  FP.api = api;
  FP.util = {clamp, round, finite};
})(window.FP);
