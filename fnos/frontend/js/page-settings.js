'use strict';
/*
 * Settings page: app info, config import/export and advanced diagnostics
 * (engine stats, network / ALSA, runtime logs with search + download, errors).
 */
window.FP = window.FP || {};

(function (FP) {
  const u = FP.util;

  function readFileText(file) {
    if (file && typeof file.text === 'function') return file.text();
    return new Promise(function (resolve, reject) {
      const reader = new FileReader();
      reader.onload = function () { resolve(String(reader.result || '')); };
      reader.onerror = function () { reject(reader.error || new Error('无法读取文件')); };
      reader.readAsText(file);
    });
  }

  function migrateLegacyImport(config) {
    const next = u.clone(config);
    if (!next.audio) next.audio = { devices: [] };
    if (!Array.isArray(next.audio.devices)) next.audio.devices = [];
    const legacy = next.audio.curve;
    const legacyHasData = !!(legacy && typeof legacy === 'object' &&
      ((legacy.kind && legacy.kind !== 'balanced') || (Array.isArray(legacy.points) && legacy.points.length)));
    const version = Number(next.config_version) || 0;
    if (!legacyHasData && version >= 2) return { config: next, migrated: false };
    if (legacyHasData) {
      // v1 had one global curve; move it to devices that still use the neutral
      // default (mirrors backend `Config::migrate`).
      next.audio.devices = next.audio.devices.map(function (device) {
        const curve = device.curve && typeof device.curve === 'object' ? device.curve : null;
        const isNeutral = !curve || (curve.kind === 'balanced' && (!Array.isArray(curve.points) || !curve.points.length));
        if (!isNeutral) return device;
        const migrated = u.clone(device);
        migrated.curve = u.clone(legacy);
        return migrated;
      });
    }
    next.audio.curve = { kind: 'balanced', points: [] };
    next.config_version = 2;
    return { config: next, migrated: true };
  }

  FP.createSettingsPage = function (ctx) {
    const store = ctx.store;
    const els = {};
    let bound = false;
    let active = false;
    let logLines = [];
    let logUpdatedAt = '';
    let logTimer = null;
    let importing = false;
    let exporting = false;

    function el(id) {
      if (!els[id]) els[id] = u.byId(id);
      return els[id];
    }

    function setConfigError(message) {
      const node = el('config-error');
      u.setText(node, message || '');
      u.show(node, !!message);
    }

    function fillKv(container, rows) {
      u.clearNode(container);
      rows.forEach(function (row) {
        const item = u.create('div');
        item.appendChild(u.create('dt', null, row[0]));
        const dd = u.create('dd', row[2] ? 'mono' : null, String(row[1]));
        item.appendChild(dd);
        container.appendChild(item);
      });
    }

    function renderAppInfo() {
      const health = store.state.health;
      const status = store.state.status;
      const version = (health && health.version) || (status && status.version) || '—';
      const versionPill = el('settings-version');
      u.setText(versionPill, version === '—' ? 'v—' : 'v' + version);
      versionPill.className = 'pill accent';
      const draft = store.getDraft();
      const level = (status && status.logging && status.logging.level) ||
        (draft && draft.logging && draft.logging.level) || 'info';
      fillKv(el('settings-kv'), [
        ['版本', version === '—' ? '—' : version],
        ['运行状态', ctx.isConnected() ? '运行中' :
          (ctx.isConnectionKnown && !ctx.isConnectionKnown() ? '连接中' : '不可用')],
        ['运行时长', status && status.uptime != null ? u.fmtUptime(status.uptime) : '—'],
        ['音频后端', status && status.audio ? u.audioModeLabel(status.audio.mode) : '—'],
        ['日志级别', level]
      ]);
      u.setSelect(el('settings-log-level'), level);
    }

    function renderConfigSummary() {
      const baseline = store.getBaseline();
      if (!baseline) return;
      const devices = (baseline.audio && Array.isArray(baseline.audio.devices)) ? baseline.audio.devices : [];
      const enabledNames = FP.PROTOCOLS.filter(function (meta) {
        return baseline.protocols && baseline.protocols[meta.key] && baseline.protocols[meta.key].enabled;
      }).map(function (meta) { return meta.label; });
      const rows = [
        ['接收器名称', baseline.receiver && baseline.receiver.name || '—'],
        ['已配置音箱', devices.length + ' 个'],
        ['已启用协议', enabledNames.length ? enabledNames.join('、') : '无'],
        ['配置版本', 'v' + (baseline.config_version || '—')],
        ['日志级别', baseline.logging && baseline.logging.level || '—']
      ];
      if (store.hasDirty()) {
        rows.push(['未保存修改', '有（导入会覆盖当前草稿）']);
      }
      fillKv(el('config-summary'), rows);
    }

    function renderEngineDiagnostics() {
      const status = store.state.status;
      const diagnostics = store.state.diagnostics;
      const devices = status && Array.isArray(status.devices) ? status.devices : [];
      const underruns = devices.reduce(function (total, device) {
        return total + (Number(device.underruns) || 0);
      }, 0);
      fillKv(el('diag-engine'), [
        ['音频后端', status && status.audio ? u.audioModeLabel(status.audio.mode) :
          (diagnostics && diagnostics.backend ? diagnostics.backend : '—')],
        ['主混音帧数', status && status.audio ? u.fmtCount(status.audio.master_frames) : '—'],
        ['丢弃帧数', status && status.audio ? u.fmtCount(status.audio.dropped_frames) : '—'],
        ['输出设备状态', devices.length ? devices.length + ' 个上报' : '未上报'],
        ['累计 underrun', devices.length ? u.fmtCount(underruns) : '—'],
        ['运行时长', status && status.uptime != null ? u.fmtUptime(status.uptime) : '—']
      ]);

      const chips = el('diag-interfaces');
      u.clearNode(chips);
      const interfaces = (diagnostics && Array.isArray(diagnostics.interfaces)) ? diagnostics.interfaces : [];
      if (!interfaces.length) {
        chips.appendChild(u.create('span', 'chip', '未上报'));
      } else {
        interfaces.forEach(function (name) {
          chips.appendChild(u.create('span', 'chip', String(name)));
        });
      }
      fillKv(el('diag-alsa'), [
        ['音频后端', diagnostics && diagnostics.backend ? diagnostics.backend : '—'],
        ['日志文件', diagnostics && diagnostics.log_path ? diagnostics.log_path : '—', true],
        ['网络接口数', interfaces.length ? String(interfaces.length) : '—']
      ]);
      u.setText(el('diag-cards'), diagnostics && diagnostics.alsa_cards ? diagnostics.alsa_cards : '未上报');
      u.setText(el('diag-pcm'), diagnostics && diagnostics.alsa_pcm ? diagnostics.alsa_pcm : '未上报');
      renderErrorList(el('diag-errors'));
    }

    function renderErrorList(container) {
      const status = store.state.status;
      const errors = (status && Array.isArray(status.errors)) ? status.errors.slice(-10) : [];
      const last = status && status.last_error ? String(status.last_error) : '';
      const entries = [];
      if (last) entries.push({ tone: 'danger', text: last });
      errors.forEach(function (message) {
        const text = String(message);
        if (text && text !== last) entries.push({ tone: 'warn', text: text });
      });
      u.clearNode(container);
      if (!entries.length) {
        entries.push({ tone: 'ok', text: '暂无故障记录。' });
      }
      entries.forEach(function (entry) {
        const node = u.create('div', 'issue ' + entry.tone);
        node.appendChild(u.svgIcon(entry.tone === 'ok' ? 'check' : 'alert'));
        node.appendChild(u.create('span', 'issue-body', entry.text));
        container.appendChild(node);
      });
    }

    function levelOf(line) {
      if (/\bERROR\b/.test(line)) return 'error';
      if (/\bWARN\b/.test(line)) return 'warn';
      return 'info';
    }

    function filteredLines() {
      const query = (el('log-search').value || '').trim().toLowerCase();
      const level = el('log-filter').value || '';
      const levelPattern = level ? new RegExp('\\b' + level + '\\b') : null;
      return logLines.filter(function (line) {
        if (query && line.toLowerCase().indexOf(query) < 0) return false;
        if (levelPattern && !levelPattern.test(line)) return false;
        return true;
      });
    }

    function appendHighlighted(row, line, query) {
      if (!query) {
        row.textContent = line;
        return;
      }
      const lower = line.toLowerCase();
      let index = 0;
      let found = lower.indexOf(query, index);
      while (found >= 0) {
        if (found > index) row.appendChild(document.createTextNode(line.slice(index, found)));
        const mark = document.createElement('mark');
        mark.textContent = line.slice(found, found + query.length);
        row.appendChild(mark);
        index = found + query.length;
        found = lower.indexOf(query, index);
      }
      if (index < line.length) row.appendChild(document.createTextNode(line.slice(index)));
    }

    function renderLogs() {
      const list = el('logs-list');
      const query = (el('log-search').value || '').trim().toLowerCase();
      const lines = filteredLines();
      u.clearNode(list);
      if (!lines.length) {
        list.appendChild(u.create('div', 'log-empty',
          logLines.length ? '没有匹配的日志行。' : '暂无日志。'));
      } else {
        const fragment = document.createDocumentFragment();
        lines.forEach(function (line) {
          const row = u.create('div', 'log-line ' + levelOf(line));
          appendHighlighted(row, line, query);
          fragment.appendChild(row);
        });
        list.appendChild(fragment);
      }
      u.setText(el('logs-hint'), logLines.length
        ? '显示 ' + lines.length + ' / ' + logLines.length + ' 行（最多 200 行）' +
          (logUpdatedAt ? ' · 更新于 ' + logUpdatedAt : '')
        : '暂无日志。后端日志达到上限会自动轮转。');
    }

    function refreshLogs(silent) {
      return FP.api.logs(200).then(function (result) {
        logLines = (result && Array.isArray(result.lines)) ? result.lines : [];
        const now = new Date();
        logUpdatedAt = [now.getHours(), now.getMinutes(), now.getSeconds()].map(function (value) {
          return String(value).padStart(2, '0');
        }).join(':');
        renderLogs();
      }).catch(function (error) {
        if (!silent) ctx.toast('日志获取失败：' + error.message, 'error');
      });
    }

    function loadDiagnostics(silent) {
      return FP.api.diagnostics().then(function (diagnostics) {
        store.setDiagnostics(diagnostics);
        renderEngineDiagnostics();
      }).catch(function (error) {
        if (!silent) ctx.toast('诊断信息获取失败：' + error.message, 'error');
      });
    }

    function applyLogLevel() {
      const level = el('settings-log-level').value;
      const button = el('settings-log-apply');
      button.disabled = true;
      FP.api.setLogLevel(level).then(function () {
        return FP.api.status().then(function (status) {
          store.applyRemote({ status: status });
          renderAppInfo();
          refreshLogs(true);
          ctx.toast('日志级别已切换为 ' + level + '。', 'ok');
        });
      }).catch(function (error) {
        ctx.toast('切换日志级别失败：' + error.message, 'error');
      }).finally(function () {
        button.disabled = false;
      });
    }

    function exportConfig() {
      if (exporting) return;
      exporting = true;
      const button = el('config-export');
      button.disabled = true;
      FP.api.exportConfig().then(function (result) {
        const config = result && result.config ? result.config : result;
        if (!config) throw new Error('后端未返回配置');
        const version = (store.state.health && store.state.health.version) || '';
        const filename = 'fusionplay-config' + (version ? '-' + version : '') + '-' +
          u.timestampForFile() + '.json';
        u.downloadText(filename, JSON.stringify(config, null, 2) + '\n');
        ctx.toast('配置已导出。', 'ok');
      }).catch(function (error) {
        ctx.toast('导出失败：' + error.message, 'error');
      }).finally(function () {
        exporting = false;
        button.disabled = false;
      });
    }

    function importFile(file) {
      if (!file || importing) return;
      setConfigError('');
      if (file.size > 1024 * 1024) {
        setConfigError('配置文件超过 1 MiB，后端会拒绝导入。');
        return;
      }
      readFileText(file).then(function (text) {
        let parsed = null;
        try {
          parsed = JSON.parse(text);
        } catch (error) {
          setConfigError('文件不是合法 JSON：' + error.message);
          return;
        }
        const config = parsed && parsed.config ? parsed.config : parsed;
        if (!config || typeof config !== 'object' || !config.receiver || !config.audio) {
          setConfigError('不是 FusionPlay 配置文件（缺少 receiver / audio 字段）。');
          return;
        }
        if (store.hasDirty()) {
          const proceed = window.confirm('导入会覆盖当前未保存的修改，确定继续？');
          if (!proceed) return;
        }
        const migrated = migrateLegacyImport(config);
        importing = true;
        el('config-import').disabled = true;
        FP.api.importConfig(migrated.config).then(function (result) {
          const applied = result && result.config ? result.config : migrated.config;
          store.adoptConfig(applied);
          ctx.toast(migrated.migrated
            ? '配置已导入；旧版全局曲线已迁移到每个已配置音箱。'
            : '配置已导入并应用。', 'ok');
          return Promise.all([
            FP.api.status().then(function (status) { store.applyRemote({ status: status }); }),
            FP.api.config().then(function (config2) { store.applyRemote({ config: config2 }); })
          ]).catch(function () { /* non-fatal */ }).then(function () {
            return ctx.confirmHostApplied(applied, 7000);
          }).then(function (check) {
            if (!check.ok) {
              ctx.toast('配置已导入，但协议宿主未在预期时间内确认生效：' + check.reason, 'warn');
            }
            if (check.warnings && check.warnings.length) {
              ctx.toast('配置已导入，但：' + check.warnings.join('；'), 'warn');
            }
            loadDiagnostics(true);
            refreshLogs(true);
            render();
          });
        }).catch(function (error) {
          setConfigError('导入失败：' + error.message);
          ctx.toast('导入失败：' + error.message, 'error');
        }).finally(function () {
          importing = false;
          el('config-import').disabled = false;
          el('config-file').value = '';
        });
      });
    }

    function copyLogs() {
      const lines = filteredLines();
      if (!lines.length) {
        ctx.toast('没有可复制的日志行。', 'info');
        return;
      }
      u.copyText(lines.join('\n')).then(function (ok) {
        ctx.toast(ok ? '已复制 ' + lines.length + ' 行日志。' : '复制失败，请手动选择文本。', ok ? 'ok' : 'error');
      });
    }

    function downloadLogs() {
      FP.api.logReport().then(function (report) {
        const browser = window.fpBrowserErrors ? window.fpBrowserErrors() : '';
        const text = report.report + (browser ? '\n--- 当前浏览器播放器与管理页 ---\n' + browser + '\n' : '');
        u.downloadText('fusionplay-logs-' + u.timestampForFile() + '.txt', text, 'text/plain;charset=utf-8');
        ctx.toast('已下载主接收器、服务和各音箱的诊断日志。', 'ok');
      }).catch(function (error) { ctx.toast('日志导出失败：' + error.message, 'error'); });
    }

    function render() {
      renderAppInfo();
      renderConfigSummary();
      renderEngineDiagnostics();
    }

    function bind() {
      if (bound) return;
      bound = true;
      el('settings-log-apply').addEventListener('click', applyLogLevel);
      el('config-export').addEventListener('click', exportConfig);
      el('config-import').addEventListener('click', function () {
        el('config-file').click();
      });
      el('config-file').addEventListener('change', function () {
        importFile(el('config-file').files && el('config-file').files[0]);
      });
      el('log-refresh').addEventListener('click', function () { refreshLogs(false); });
      el('log-search').addEventListener('input', renderLogs);
      el('log-filter').addEventListener('change', renderLogs);
      el('log-copy').addEventListener('click', copyLogs);
      el('log-download').addEventListener('click', downloadLogs);
      el('log-auto').addEventListener('change', function () {
        if (el('log-auto').checked) refreshLogs(true);
      });
      el('settings-advanced').addEventListener('toggle', function () {
        if (el('settings-advanced').open) {
          loadDiagnostics(true);
          refreshLogs(true);
        }
      });
    }

    return {
      enter: function () {
        active = true;
        bind();
        render();
        refreshLogs(true);
        loadDiagnostics(true);
        if (logTimer) clearInterval(logTimer);
        logTimer = setInterval(function () {
          if (!active || document.hidden) return;
          if (!el('log-auto').checked) return;
          if (!el('settings-advanced').open) return;
          refreshLogs(true);
        }, 4000);
      },
      leave: function () {
        active = false;
        if (logTimer) {
          clearInterval(logTimer);
          logTimer = null;
        }
      },
      render: render
    };
  };
})(window.FP);
