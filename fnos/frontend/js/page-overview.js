'use strict';
/* Overview page: current reception, outputs, protocols, service health. */
window.FP = window.FP || {};

(function (FP) {
  const u = FP.util;

  FP.createOverviewPage = function (ctx) {
    const store = ctx.store;
    const els = {};
    let bound = false;
    let coverUrl = '';
    let coverFailed = '';

    function el(id) {
      if (!els[id]) els[id] = u.byId(id);
      return els[id];
    }

    function applyPill(node, text, tone) {
      if (!node) return;
      u.setText(node, text);
      node.className = 'pill ' + (tone || 'neutral');
    }

    function isDeviceOnline(id) {
      const runtime = store.getDeviceStatus(id);
      if (runtime) return !!runtime.online;
      const info = store.getDiscoveredDevice(id);
      return info ? !!info.online : false;
    }

    function renderNow() {
      const status = store.state.status;
      const media = (status && status.media) || {};
      const protocols = (status && status.protocols) || {};
      const source = status && status.current_source ? status.current_source : null;
      const sourceRuntime = source ? protocols[source] : null;
      const sourceState = sourceRuntime ? sourceRuntime.state : '';

      let pillText = '待机';
      let pillTone = 'neutral';
      if (source) {
        if (sourceState === 'paused') {
          pillText = '已暂停';
          pillTone = 'warn';
        } else if (sourceState === 'playing' || sourceState === 'streaming') {
          pillText = '播放中';
          pillTone = 'ok';
        } else if (sourceState === 'loading' || sourceState === 'buffering') {
          pillText = '加载中';
          pillTone = 'neutral';
        } else if (sourceState === 'error' || sourceState === 'failed') {
          pillText = '异常';
          pillTone = 'danger';
        } else {
          pillText = '接收中';
          pillTone = 'ok';
        }
      }
      applyPill(el('now-state'), pillText, pillTone);

      u.setText(el('now-title'), media.title || (source ? '正在接收音频' : '等待音频投放'));
      u.setText(el('now-artist'), media.artist ||
        (source ? '发送端未提供曲目信息' : '使用手机选择 FusionPlay'));

      // Tags: source protocol + codec when the sender reports them.
      const tags = el('now-tags');
      u.clearNode(tags);
      if (source) {
        const meta = u.protocolMeta(source);
        const chip = u.create('span', 'tag accent');
        chip.appendChild(u.svgIcon(meta.icon, 'small'));
        const label = u.create('span', null, meta.label);
        label.style.marginLeft = '4px';
        chip.appendChild(label);
        tags.appendChild(chip);
      }
      if (media.detail_codec) {
        tags.appendChild(u.create('span', 'tag', String(media.detail_codec)));
      }

      // Cover art: only the real URL reported by the sender, otherwise a
      // neutral placeholder icon (no fabricated artwork).
      const image = el('now-cover');
      const placeholder = el('now-cover-empty');
      const url = media.cover_url ? String(media.cover_url) : '';
      const backdrop = el('now-backdrop');
      if (url !== coverUrl) {
        backdrop.hidden = true;
        backdrop.removeAttribute('src');
      }
      if (!url) {
        coverUrl = '';
        coverFailed = '';
        image.hidden = true;
        image.removeAttribute('src');
        placeholder.hidden = false;
      } else {
        if (coverUrl !== url) {
          coverUrl = url;
          coverFailed = '';
          image.src = url;
        }
        const broken = coverFailed === url;
        image.hidden = broken;
        placeholder.hidden = !broken;
      }

      // Progress only when both position and duration are reported.
      const progress = el('now-progress');
      const duration = Number(media.duration_ms);
      const position = Number(media.position_ms);
      const hasProgress = Number.isFinite(duration) && duration > 0 &&
        Number.isFinite(position) && position >= 0;
      u.show(progress, hasProgress);
      if (hasProgress) {
        const percent = u.clamp(position / duration * 100, 0, 100);
        el('now-progress-fill').style.width = percent.toFixed(2) + '%';
        u.setText(el('now-position'), u.fmtPosition(position));
        u.setText(el('now-duration'), u.fmtPosition(duration));
      } else {
        el('now-progress-fill').style.width = '0%';
      }

      const metaList = el('now-meta');
      u.clearNode(metaList);
      const metaRows = [];
      if (media.album) metaRows.push({ label: '专辑', value: String(media.album) });
      if (media.detail_codec) metaRows.push({ label: '音频格式', value: String(media.detail_codec) });
      if (Number.isFinite(duration) && duration > 0) {
        metaRows.push({ label: '时长', value: u.fmtPosition(duration) });
      }
      metaRows.forEach(function (row) {
        const item = u.create('div');
        item.appendChild(u.create('dt', null, row.label));
        item.appendChild(u.create('dd', null, row.value));
        metaList.appendChild(item);
      });
      u.show(metaList, metaRows.length > 0);

      // Sender logical main volume (read-only).
      const volume = (status && status.sender_volume) || {};
      const volumeRows = el('sender-volume-rows');
      u.clearNode(volumeRows);
      const airplayDb = volume.airplay_db;
      const airplayPercent = volume.airplay_percent;
      let airplayText = '未上报';
      if (airplayDb != null && Number.isFinite(Number(airplayDb))) {
        airplayText = u.fmtDb(airplayDb);
        if (airplayPercent != null && Number.isFinite(Number(airplayPercent))) {
          airplayText += '（' + Math.round(Number(airplayPercent)) + '%）';
        }
      } else if (airplayPercent != null && Number.isFinite(Number(airplayPercent))) {
        airplayText = Math.round(Number(airplayPercent)) + '%';
      }
      const percentText = function (value) {
        return value != null && Number.isFinite(Number(value)) ? Math.round(Number(value)) + '%' : '未上报';
      };
      [
        { label: 'AirPlay 2', value: airplayText },
        { label: 'DLNA', value: percentText(volume.dlna_percent) },
        { label: '小米妙播', value: percentText(volume.miplay_percent) }
      ].forEach(function (row) {
        const item = u.create('div');
        item.appendChild(u.create('dt', null, row.label));
        item.appendChild(u.create('dd', null, row.value));
        volumeRows.appendChild(item);
      });
    }

    function renderOutputs() {
      const draft = store.getDraft();
      if (!draft) {
        applyPill(el('ov-output-summary'), '加载中', 'neutral');
        u.clearNode(el('ov-devices'));
        u.setText(el('ov-output-hint'), '正在加载配置…');
        u.show(el('ov-output-hint'), true);
        return;
      }
      const configDevices = (draft && draft.audio && Array.isArray(draft.audio.devices))
        ? draft.audio.devices : [];
      const enabled = configDevices.filter(function (device) { return device.enabled; });
      const onlineCount = enabled.filter(function (device) { return isDeviceOnline(device.id); }).length;

      const summary = el('ov-output-summary');
      if (!configDevices.length) {
        applyPill(summary, '未配置', 'neutral');
      } else if (!enabled.length) {
        applyPill(summary, '未启用', 'neutral');
      } else {
        applyPill(summary, enabled.length + ' 个已启用' + (onlineCount ? '' : ' · 全部离线'),
          onlineCount ? 'ok' : 'warn');
      }

      const list = el('ov-devices');
      u.clearNode(list);
      enabled.forEach(function (device) {
        const runtime = store.getDeviceStatus(device.id);
        const info = store.getDiscoveredDevice(device.id);
        const kind = u.deviceKind(device.name, device.id, info && info.card);
        const item = u.create('li');
        const iconWrap = u.create('span', 'device-mini-icon');
        iconWrap.appendChild(u.svgIcon(kind.icon, 'small'));
        item.appendChild(iconWrap);
        item.appendChild(u.create('span', 'device-mini-name', device.name || device.id));
        const online = isDeviceOnline(device.id);
        let text = '在线';
        let tone = 'ok';
        if (!online) {
          text = '离线';
          tone = 'warn';
        } else if (runtime && (runtime.state === 'error' || runtime.state === 'failed')) {
          text = '异常';
          tone = 'danger';
        } else if (runtime && runtime.state === 'running') {
          text = '输出中';
        } else if (runtime && runtime.state === 'starting') {
          text = '启动中';
        }
        const pill = u.create('span', 'pill ' + tone, text);
        item.appendChild(pill);
        list.appendChild(item);
      });

      const hint = el('ov-output-hint');
      const disabledCount = configDevices.length - enabled.length;
      let hintText = '';
      if (!configDevices.length) {
        hintText = '尚未配置输出音箱，前往「音频」页添加。';
      } else if (enabled.length && onlineCount < enabled.length) {
        hintText = enabled.length - onlineCount + ' 个已启用音箱当前离线。';
      } else if (disabledCount > 0) {
        hintText = '共 ' + configDevices.length + ' 个已配置，' + disabledCount + ' 个未启用。';
      }
      u.setText(hint, hintText);
      u.show(hint, !!hintText);
    }

    function renderProtocols() {
      const status = store.state.status;
      const protocols = (status && status.protocols) || {};
      const list = el('ov-protocols');
      u.clearNode(list);
      let enabledCount = 0;
      let errorCount = 0;
      const draft = store.getDraft();
      const miplayType = draft && draft.protocols && draft.protocols.miplay
        ? draft.protocols.miplay.device_type : '';
      FP.PROTOCOLS.forEach(function (meta) {
        const runtime = protocols[meta.key] || null;
        const item = u.create('li');
        const nameWrap = u.create('div', 'protocol-mini-name');
        const iconWrap = u.create('span', 'device-icon');
        iconWrap.appendChild(u.svgIcon(meta.key === 'miplay' ? u.miplayDeviceIcon(miplayType) : meta.icon));
        nameWrap.appendChild(iconWrap);
        const textWrap = u.create('div');
        textWrap.appendChild(u.create('div', null, meta.label));
        if (runtime && runtime.detail) {
          const sub = u.create('div', 'protocol-mini-sub', String(runtime.detail));
          sub.title = String(runtime.detail);
          textWrap.appendChild(sub);
        }
        nameWrap.appendChild(textWrap);
        item.appendChild(nameWrap);

        let text = '未上报';
        let tone = 'neutral';
        if (runtime) {
          if (runtime.enabled) enabledCount += 1;
          if (runtime.enabled) {
            if (runtime.state === 'error' || runtime.state === 'failed') {
              text = '异常';
              tone = 'danger';
              errorCount += 1;
            } else if (runtime.state) {
              text = u.stateLabel(runtime.state);
              tone = u.stateTone(runtime.state);
            } else {
              text = '未上报';
            }
          } else {
            text = '已停用';
            tone = 'neutral';
          }
        }
        item.appendChild(u.create('span', 'pill ' + tone, text));
        list.appendChild(item);
      });

      const summary = el('ov-protocols-summary');
      if (!status) {
        applyPill(summary, '加载中', 'neutral');
      } else if (errorCount) {
        applyPill(summary, errorCount + ' 个异常', 'danger');
      } else if (!enabledCount) {
        applyPill(summary, '全部停用', 'neutral');
      } else {
        applyPill(summary, enabledCount + '/3 已启用', 'ok');
      }
    }

    function fillServiceKv(version, uptime, engine, speakers, protocols) {
      const kv = el('ov-service-kv');
      u.clearNode(kv);
      [version, uptime, engine, speakers, protocols].forEach(function (row) {
        const item = u.create('div');
        item.appendChild(u.create('dt', null, row[0]));
        item.appendChild(u.create('dd', null, row[1]));
        kv.appendChild(item);
      });
    }

    function renderService() {
      const connected = ctx.isConnected();
      const known = ctx.isConnectionKnown ? ctx.isConnectionKnown() : true;
      if (!known) {
        applyPill(el('ov-service-pill'), '连接中', 'neutral');
        fillServiceKv(['版本', '—'], ['运行时长', '—'], ['音频引擎', '—'],
          ['已启用音箱', '—'], ['协议', '—']);
        const loading = el('ov-issues');
        u.clearNode(loading);
        const node = u.create('div', 'issue');
        node.appendChild(u.svgIcon('info'));
        node.appendChild(u.create('span', 'issue-body', '正在连接后端服务…'));
        loading.appendChild(node);
        return;
      }
      const health = store.state.health;
      const status = store.state.status;
      const version = (health && health.version) || (status && status.version) || '—';
      const draft = store.getDraft();
      const devices = (draft && draft.audio && Array.isArray(draft.audio.devices)) ? draft.audio.devices : [];
      const enabled = devices.filter(function (device) { return device.enabled; });
      const offline = enabled.filter(function (device) { return !isDeviceOnline(device.id); });
      const protocols = (status && status.protocols) || {};
      const issues = [];

      if (!connected) {
        issues.push({ tone: 'danger', text: '无法连接后端服务。', icon: 'alert' });
      }
      FP.PROTOCOLS.forEach(function (meta) {
        const runtime = protocols[meta.key];
        if (runtime && runtime.enabled && (runtime.state === 'error' || runtime.state === 'failed')) {
          issues.push({ tone: 'warn', text: meta.label + ' 运行异常。', icon: 'alert' });
        }
      });
      if (offline.length) {
        issues.push({
          tone: 'warn',
          text: offline.length + ' 个已启用音箱当前离线：' +
            offline.map(function (device) { return device.name || device.id; }).join('、') + '。',
          icon: 'alert'
        });
      }
      if (status && status.audio && status.audio.mode === 'null') {
        issues.push({ tone: 'warn', text: '当前为 null 音频后端（无实际声卡），不会输出声音。', icon: 'info' });
      }
      if (status && status.last_error) {
        const message = String(status.last_error);
        issues.push({
          tone: 'danger',
          icon: 'alert',
          text: '最近错误：' + (message.length > 180 ? message.slice(0, 180) + '…' : message),
          title: message
        });
      } else if (status && Array.isArray(status.errors) && status.errors.length) {
        issues.push({ tone: 'warn', icon: 'alert', text: '有 ' + status.errors.length + ' 条错误记录，详见「设置 → 高级诊断」。' });
      }
      if (!issues.length) {
        issues.push({ tone: 'ok', icon: 'check', text: '未发现需要关注的问题。' });
      }

      const hasDanger = issues.some(function (issue) { return issue.tone === 'danger'; });
      const hasWarn = issues.some(function (issue) { return issue.tone === 'warn'; });
      const pill = el('ov-service-pill');
      if (!connected) applyPill(pill, '服务不可用', 'danger');
      else if (hasDanger) applyPill(pill, '服务异常', 'danger');
      else if (hasWarn) applyPill(pill, '需关注', 'warn');
      else applyPill(pill, '运行正常', 'ok');

      fillServiceKv(
        ['版本', version === '—' ? '—' : 'v' + version],
        ['运行时长', status && status.uptime != null ? u.fmtUptime(status.uptime) : '—'],
        ['音频引擎', status && status.audio ? u.audioModeLabel(status.audio.mode) : '—'],
        ['已启用音箱', devices.length ? enabled.length + ' / ' + devices.length : '未配置'],
        ['协议', status ? FP.PROTOCOLS.filter(function (meta) {
          const runtime = protocols[meta.key];
          return runtime && runtime.enabled;
        }).length + ' / 3 已启用' : '—']
      );

      const list = el('ov-issues');
      u.clearNode(list);
      issues.forEach(function (issue) {
        const node = u.create('div', 'issue ' + issue.tone);
        node.appendChild(u.svgIcon(issue.icon || 'info'));
        const body = u.create('span', 'issue-body', issue.text);
        if (issue.title) body.title = issue.title;
        node.appendChild(body);
        list.appendChild(node);
      });
    }

    function render() {
      renderNow();
      renderOutputs();
      renderProtocols();
      renderService();
    }

    function bind() {
      if (bound) return;
      bound = true;
      const image = el('now-cover');
      if (image) {
        image.addEventListener('load', function () {
          if (!coverUrl || image.getAttribute('src') !== coverUrl) return;
          const backdrop = el('now-backdrop');
          backdrop.src = image.currentSrc;
          backdrop.hidden = false;
          coverFailed = '';
          image.hidden = false;
          const placeholder = el('now-cover-empty');
          if (placeholder) placeholder.hidden = true;
        });
        image.addEventListener('error', function () {
          el('now-backdrop').hidden = true;
          coverFailed = coverUrl;
          image.hidden = true;
          const placeholder = el('now-cover-empty');
          if (placeholder) placeholder.hidden = false;
        });
      }
    }

    return {
      enter: function () { bind(); render(); },
      leave: function () {},
      render: render,
      renderLive: renderNow
    };
  };
})(window.FP);
