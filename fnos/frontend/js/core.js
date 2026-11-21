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
    path = path.replace(/\/index\.html$/i, '');
    if (path.length > 1) {
      path = path.replace(/\/+$/, '');
    }
    return path === '/' || path === '' ? '' : path;
  })();

  const SVG_NS = 'http://www.w3.org/2000/svg';

  // ------------------------------------------------------------------ utils
  const byId = function (id) { return document.getElementById(id); };
  const qs = function (selector, root) { return (root || document).querySelector(selector); };
  const qsa = function (selector, root) {
    return Array.prototype.slice.call((root || document).querySelectorAll(selector));
  };

  function create(tag, className, text) {
    const node = document.createElement(tag);
    if (className) node.className = className;
    if (text != null) node.textContent = text;
    return node;
  }

  function svgIcon(name, extraClass) {
    const svg = document.createElementNS(SVG_NS, 'svg');
    svg.setAttribute('class', 'icon' + (extraClass ? ' ' + extraClass : ''));
    svg.setAttribute('aria-hidden', 'true');
    svg.setAttribute('focusable', 'false');
    const use = document.createElementNS(SVG_NS, 'use');
    use.setAttribute('href', '#icon-' + name);
    svg.appendChild(use);
    return svg;
  }

  function clearNode(node) {
    if (!node) return;
    while (node.firstChild) node.removeChild(node.firstChild);
  }

  function setText(node, text) {
    if (node && node.textContent !== text) node.textContent = text;
  }

  function setTextById(id, text) {
    setText(byId(id), text);
  }

  function show(node, visible) {
    if (node) node.hidden = !visible;
  }

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

  function clone(value) {
    return value == null ? value : JSON.parse(JSON.stringify(value));
  }

  function deepEqual(a, b) {
    if (a === b) return true;
    if (a === null || b === null || typeof a !== typeof b) return false;
    if (Array.isArray(a) || Array.isArray(b)) {
      if (!Array.isArray(a) || !Array.isArray(b) || a.length !== b.length) return false;
      for (let i = 0; i < a.length; i += 1) {
        if (!deepEqual(a[i], b[i])) return false;
      }
      return true;
    }
    if (typeof a === 'object') {
      const keysA = Object.keys(a);
      const keysB = Object.keys(b);
      if (keysA.length !== keysB.length) return false;
      for (let i = 0; i < keysA.length; i += 1) {
        const key = keysA[i];
        if (!Object.prototype.hasOwnProperty.call(b, key)) return false;
        if (!deepEqual(a[key], b[key])) return false;
      }
      return true;
    }
    return false;
  }

  function getPath(obj, path) {
    let current = obj;
    for (let i = 0; i < path.length; i += 1) {
      if (current == null || typeof current !== 'object') return undefined;
      current = current[path[i]];
    }
    return current;
  }

  function setPath(obj, path, value) {
    let current = obj;
    for (let i = 0; i < path.length - 1; i += 1) {
      const key = path[i];
      if (current[key] == null || typeof current[key] !== 'object') {
        current[key] = /^\d+$/.test(path[i + 1]) ? [] : {};
      }
      current = current[key];
    }
    current[path[path.length - 1]] = value;
  }

  function debounce(fn, wait) {
    let timer = null;
    return function debounced() {
      const args = arguments;
      const context = this;
      if (timer) clearTimeout(timer);
      timer = setTimeout(function () {
        timer = null;
        fn.apply(context, args);
      }, wait);
    };
  }

  function delay(ms) {
    return new Promise(function (resolve) { setTimeout(resolve, ms); });
  }

  function pad2(value) {
    return String(value).padStart(2, '0');
  }

  function fmtUptime(seconds) {
    const total = Math.max(0, Math.floor(finite(seconds, 0)));
    const days = Math.floor(total / 86400);
    const hours = Math.floor((total % 86400) / 3600);
    const minutes = Math.floor((total % 3600) / 60);
    const secs = total % 60;
    const parts = [];
    if (days) parts.push(days + ' 天');
    if (hours) parts.push(hours + ' 小时');
    if (minutes) parts.push(minutes + ' 分');
    if (!parts.length || secs) parts.push(secs + ' 秒');
    return parts.join(' ');
  }

  function fmtPosition(ms) {
    if (ms == null || !Number.isFinite(Number(ms))) return '—';
    const total = Math.max(0, Math.round(Number(ms) / 1000));
    const hours = Math.floor(total / 3600);
    const minutes = Math.floor((total % 3600) / 60);
    const secs = total % 60;
    return hours ? hours + ':' + pad2(minutes) + ':' + pad2(secs) : minutes + ':' + pad2(secs);
  }

  function fmtRate(rate) {
    const value = finite(rate, 0);
    if (!value) return '—';
    const khz = value / 1000;
    return (Number.isInteger(khz) ? khz : khz.toFixed(1)) + ' kHz';
  }

  function fmtDb(value) {
    return value == null || !Number.isFinite(Number(value)) ? '—' : round(value, 1) + ' dB';
  }

  function fmtMs(value) {
    return value == null || !Number.isFinite(Number(value)) ? '—' : Math.round(Number(value)) + ' ms';
  }

  function fmtPpm(value) {
    return value == null || !Number.isFinite(Number(value)) ? '—' : round(value, 1) + ' ppm';
  }

  function fmtCount(value) {
    return value == null || !Number.isFinite(Number(value)) ? '—' : Number(value).toLocaleString('zh-CN');
  }

  function fmtPercent(value) {
    return value == null || !Number.isFinite(Number(value)) ? '—' : Math.round(Number(value)) + '%';
  }

  function fmtChannels(channels) {
    const value = finite(channels, 0);
    if (!value) return '—';
    return value === 1 ? '单声道' : value === 2 ? '立体声' : value + ' 声道';
  }

  function timestampForFile() {
    const now = new Date();
    return now.getFullYear() + pad2(now.getMonth() + 1) + pad2(now.getDate()) + '-' +
      pad2(now.getHours()) + pad2(now.getMinutes()) + pad2(now.getSeconds());
  }

  function syncRange(input) {
    if (!input) return;
    const min = finite(input.min, 0);
    const max = finite(input.max, 100);
    const value = finite(input.value, min);
    const pct = max > min ? ((value - min) / (max - min)) * 100 : 0;
    input.style.setProperty('--fill', pct.toFixed(2) + '%');
  }

  function setRange(input, value) {
    if (!input) return;
    if (document.activeElement === input) return;
    const min = finite(input.min, 0);
    const max = finite(input.max, 100);
    const next = clamp(finite(value, min), min, max);
    if (Number(input.value) !== next) input.value = String(next);
    syncRange(input);
  }

  function setInput(input, value) {
    if (!input) return;
    if (document.activeElement === input) return;
    const next = value == null ? '' : String(value);
    if (input.value !== next) input.value = next;
  }

  function setSelect(select, value) {
    if (!select) return;
    if (document.activeElement === select) return;
    const next = value == null ? '' : String(value);
    if (select.value !== next) select.value = next;
  }

  function setSwitch(button, checked) {
    if (button) button.setAttribute('aria-checked', checked ? 'true' : 'false');
  }

  function downloadText(filename, text, mime) {
    const blob = new Blob([text], { type: mime || 'application/json;charset=utf-8' });
    const url = URL.createObjectURL(blob);
    const anchor = document.createElement('a');
    anchor.href = url;
    anchor.download = filename;
    document.body.appendChild(anchor);
    anchor.click();
    anchor.remove();
    setTimeout(function () { URL.revokeObjectURL(url); }, 2000);
  }

  function fallbackCopy(text) {
    const area = document.createElement('textarea');
    area.value = text;
    area.setAttribute('readonly', '');
    area.style.position = 'fixed';
    area.style.top = '-1000px';
    area.style.opacity = '0';
    document.body.appendChild(area);
    area.select();
    let ok = false;
    try { ok = document.execCommand('copy'); } catch (error) { ok = false; }
    area.remove();
    return ok;
  }

  function copyText(text) {
    if (navigator.clipboard && window.isSecureContext) {
      return navigator.clipboard.writeText(text).then(function () { return true; }, function () {
        return fallbackCopy(text);
      });
    }
    return Promise.resolve(fallbackCopy(text));
  }

  // ----------------------------------------------------------- label tables
  const PROTOCOLS = [
    { key: 'airplay', label: 'AirPlay 2', icon: 'airplay', desc: 'Apple 设备投放：iPhone、iPad、Mac' },
    { key: 'dlna', label: 'DLNA', icon: 'dlna', desc: '通用媒体协议：音乐 App 与第三方控制端' },
    { key: 'miplay', label: '小米妙播', icon: 'miplay', desc: '小米 / 妙播协议，HyperOS 设备投放' }
  ];

  // Canonical protocol states (see fnos/backend/src/protocols.rs). The backend
  // uses one vocabulary for AirPlay/DLNA/MiPlay, and legacy aliases are mapped
  // here as a defensive fallback so the overview and receive page can never
  // show different words for the same state (e.g. 就绪/监听中/运行中).
  const STATE_LABELS = {
    waiting: '等待投放', ready: '等待投放', listening: '等待投放',
    running: '等待投放', idle: '等待投放',
    starting: '启动中', loading: '加载中', buffering: '加载中',
    connecting: '启动中', reconnecting: '启动中',
    streaming: '播放中', playing: '播放中', active: '播放中',
    paused: '已暂停',
    stopping: '正在停止', stopped: '已关闭', disabled: '已关闭',
    offline: '未连接', disconnected: '未连接', connected: '已连接',
    error: '异常', failed: '异常'
  };

  // MiPlay device identities, mirroring the Android app (车机/电视/平板/音响).
  const MIPLAY_DEVICE_TYPES = [
    { value: 'vehicle', label: '车机', icon: 'device-vehicle' },
    { value: 'television', label: '电视', icon: 'device-television' },
    { value: 'tablet', label: '平板', icon: 'device-tablet' },
    { value: 'speaker', label: '音响', icon: 'device-speaker' }
  ];

  const CURVE_LABELS = { linear: '线性', soft: '柔和', balanced: '均衡', custom: '自定义' };
  const LOG_LEVEL_LABELS = { error: 'error', warn: 'warn', info: 'info', debug: 'debug', trace: 'trace' };

  function protocolMeta(key) {
    return PROTOCOLS.find(function (item) { return item.key === key; }) ||
      { key: key, label: key || '未知', icon: 'speaker', desc: '' };
  }

  function stateLabel(state) {
    if (!state) return '未上报';
    return STATE_LABELS[state] || state;
  }

  function stateTone(state) {
    switch (state) {
      case 'waiting': case 'ready': case 'listening': case 'running':
      case 'idle': case 'streaming': case 'playing': case 'active':
        return 'ok';
      case 'paused': return 'warn';
      case 'error': case 'failed': return 'danger';
      default: return 'neutral';
    }
  }

  function miplayDeviceType(value) {
    const normalized = String(value || '').trim().toLowerCase();
    if (normalized === 'display_speaker') return MIPLAY_DEVICE_TYPES[3];
    return MIPLAY_DEVICE_TYPES.find(function (item) { return item.value === normalized; }) ||
      MIPLAY_DEVICE_TYPES[1];
  }

  function miplayDeviceIcon(value) {
    return miplayDeviceType(value).icon;
  }

  function miplayDeviceLabel(value) {
    return miplayDeviceType(value).label;
  }

  function curveKindLabel(kind) {
    return CURVE_LABELS[kind] || '均衡';
  }

  function audioModeLabel(mode) {
    if (!mode) return '未上报';
    if (mode === 'alsa') return 'ALSA 声卡';
    if (mode === 'null') return '无音频后端（null）';
    return mode;
  }

  function deviceKind(name, id, card) {
    const text = ((name || '') + ' ' + (id || '') + ' ' + (card || '')).toLowerCase();
    if (text.indexOf('hdmi') >= 0) return { icon: 'hdmi', label: 'HDMI 输出' };
    if (text.indexOf('usb') >= 0) return { icon: 'usb', label: 'USB 声卡' };
    if (text.indexOf('headphone') >= 0 || text.indexOf('analog') >= 0 || text.indexOf('line') >= 0) {
      return { icon: 'headphones', label: '模拟输出' };
    }
    return { icon: 'speaker', label: '音频设备' };
  }

  // ------------------------------------------------------------- API layer
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
    health: function () { return request('GET', '/healthz', undefined, 6000); },
    status: function () { return request('GET', '/api/status', undefined, 9000); },
    config: function () { return request('GET', '/api/config', undefined, 9000); },
    saveConfig: function (config) { return request('PUT', '/api/config', config, 45000); },
    devices: function () { return request('GET', '/api/devices', undefined, 10000); },
    refreshDevices: function () { return request('POST', '/api/devices/refresh', {}, 25000); },
    testTone: function (id) { return request('POST', '/api/devices/test', { id: id }, 12000); },
    preview: function (curve) { return request('POST', '/api/calibration/preview', { curve: curve }, 8000); },
    logs: function (lines) {
      return request('GET', '/api/logs?lines=' + (lines || 200), undefined, 10000);
    },
    setLogLevel: function (level) { return request('POST', '/api/logs/level', { level: level }, 10000); },
    exportConfig: function () { return request('GET', '/api/export', undefined, 10000); },
    importConfig: function (config) { return request('POST', '/api/import', config, 45000); },
    diagnostics: function () { return request('GET', '/api/diagnostics', undefined, 10000); }
  };

  FP.playerUrl = function (id) {
    const url = new URL('player/index.html', document.baseURI);
    if (id && id !== 'default') url.searchParams.set('instance', id);
    return url.href;
  };
  FP.GATEWAY_BASE = GATEWAY_BASE;
  FP.PROTOCOLS = PROTOCOLS;
  FP.MIPLAY_DEVICE_TYPES = MIPLAY_DEVICE_TYPES;
  FP.apiUrl = apiUrl;
  FP.api = api;
  FP.util = {
    byId: byId, qs: qs, qsa: qsa, create: create, svgIcon: svgIcon,
    clearNode: clearNode, setText: setText, setTextById: setTextById, show: show,
    clamp: clamp, round: round, finite: finite, clone: clone, deepEqual: deepEqual,
    getPath: getPath, setPath: setPath, debounce: debounce, delay: delay,
    fmtUptime: fmtUptime, fmtPosition: fmtPosition, fmtRate: fmtRate, fmtDb: fmtDb,
    fmtMs: fmtMs, fmtPpm: fmtPpm, fmtCount: fmtCount, fmtPercent: fmtPercent,
    fmtChannels: fmtChannels, timestampForFile: timestampForFile,
    miplayDeviceType: miplayDeviceType, miplayDeviceIcon: miplayDeviceIcon,
    miplayDeviceLabel: miplayDeviceLabel,
    syncRange: syncRange, setRange: setRange, setInput: setInput, setSelect: setSelect,
    setSwitch: setSwitch, downloadText: downloadText, copyText: copyText,
    protocolMeta: protocolMeta, stateLabel: stateLabel, stateTone: stateTone,
    curveKindLabel: curveKindLabel, audioModeLabel: audioModeLabel, deviceKind: deviceKind,
    LOG_LEVEL_LABELS: LOG_LEVEL_LABELS
  };
})(window.FP);
