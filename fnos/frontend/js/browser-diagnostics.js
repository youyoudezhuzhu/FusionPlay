// Browser-local errors are available even while the receiver is offline.
(function () {
  'use strict';
  const key = 'fusionplay.browser-errors.v1';
  const lifetime = 7 * 86400000;
  function rows() {
    const value = JSON.parse(localStorage.getItem(key) || '[]');
    return Array.isArray(value) ? value.filter(r => r && Number.isFinite(r.time) &&
      Date.now() - r.time < lifetime && typeof r.text === 'string').slice(-32) : [];
  }
  window.fpBrowserErrors = () => {
    try {
      const records = rows();
      localStorage.setItem(key, JSON.stringify(records));
      return records.map(r => new Date(r.time).toISOString() + ' ' + r.text).join('\n');
    } catch (_) { return ''; }
  };
  window.fpRecordBrowserError = message => {
    try {
      const records = rows();
      const room = new URLSearchParams(location.search).get('instance') || 'default';
      const page = location.pathname.split('/').pop();
      const text = '[' + page + ' ' + (/^[a-z0-9-]{1,40}$/.test(room) ? room : '?') + '] ' +
        String(message).slice(0, 2400).replace(/(?:(?:https?|rtsp|file):\/\/|data:)\S+/gi, '<URL>')
          .replace(/(?:authorization|cookie)\s*[:=]\s*[^\r\n]+|(?:password|token|secret)\s*[:=]\s*[^\s,;]+/gi, '<redacted>');
      const last = records[records.length - 1];
      if (last && last.text === text && Date.now() - last.time < 30000) return;
      records.push({time: Date.now(), text});
      localStorage.setItem(key, JSON.stringify(records.slice(-32)));
    } catch (_) { /* Storage can be unavailable in private browsing. */ }
  };
  window.addEventListener('error', e => window.fpRecordBrowserError(e.error?.stack || e.message));
  window.addEventListener('unhandledrejection', e => window.fpRecordBrowserError(e.reason?.stack || e.reason));
  window.fpBrowserErrors();
  setInterval(window.fpBrowserErrors, 3600000);
})();
