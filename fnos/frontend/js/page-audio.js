'use strict';
/*
 * Audio page: output speaker list, per-device calibration (gain / delay /
 * independent volume curve) and advanced runtime parameters.
 */
window.FP = window.FP || {};

(function (FP) {
  const u = FP.util;
  const NARROW = '(max-width: 920px)';

  FP.createAudioPage = function (ctx) {
    const store = ctx.store;
    const els = {};
    const speakerSettings = FP.createSpeakerSettings(ctx, render);
    let bound = false;
    let active = false;
    let chart = null;
    let selectedId = null;
    let pendingDeleteId = null;
    let deleteTimer = null;
    let refreshing = false;
    let previewSeq = 0;
    let previewTimer = null;
    let previewSamples = null;
    let previewNote = '';
    let chartKey = '';
    let rateSig = '';

    function el(id) {
      if (!els[id]) els[id] = u.byId(id);
      return els[id];
    }

    function toast(message, type) { ctx.toast(message, type); }

    function isNarrow() {
      return window.matchMedia && window.matchMedia(NARROW).matches;
    }

    function draftDevices() {
      const draft = store.getDraft();
      return draft && draft.audio && Array.isArray(draft.audio.devices) ? draft.audio.devices : [];
    }

    function deviceOnline(device) {
      const runtime = store.getDeviceStatus(device.id);
      if (runtime) return !!runtime.online;
      const info = store.getDiscoveredDevice(device.id);
      return info ? !!info.online : false;
    }

    function assignedSpeaker(id) {
      const audio = (store.getDraft() || {}).audio;
      return audio && (audio.instances || []).find(instance => instance.device_id === id);
    }

    function deviceState(device) {
      if (assignedSpeaker(device.id)) return {text: '独立接收', tone: 'neutral'};
      const online = deviceOnline(device);
      if (!online) return { text: '离线', tone: 'warn' };
      if (!device.enabled) return { text: '未启用', tone: 'neutral' };
      const runtime = store.getDeviceStatus(device.id);
      const state = runtime && runtime.state;
      if (state === 'error' || state === 'failed') return { text: '异常', tone: 'danger' };
      if (state === 'running') return { text: '输出中', tone: 'ok' };
      if (state === 'starting') return { text: '启动中', tone: 'neutral' };
      if (state === 'idle') return { text: '空闲', tone: 'neutral' };
      if (state) return { text: u.stateLabel(state), tone: u.stateTone(state) };
      return { text: '在线', tone: 'ok' };
    }

    function deviceInfo(device) {
      return store.getDiscoveredDevice(device.id);
    }

    function deviceSubtitle(device) {
      const info = deviceInfo(device);
      const runtime = store.getDeviceStatus(device.id);
      const kind = u.deviceKind(device.name, device.id, info && info.card);
      const parts = [kind.label];
      if (deviceOnline(device)) {
        const rate = runtime && runtime.sample_rate ? runtime.sample_rate :
          (info && info.sample_rates && info.sample_rates.length ? info.sample_rates[0] : 0);
        const channels = runtime && runtime.channels ? runtime.channels : (info && info.channels ? info.channels : 0);
        if (rate) parts.push(u.fmtRate(rate));
        if (channels) parts.push(u.fmtChannels(channels));
      } else {
        parts.push('当前离线');
      }
      const speaker = assignedSpeaker(device.id);
      if (speaker) parts.push(speaker.name);
      else if (!device.enabled) parts.push('主接收器未启用');
      return parts.join(' · ');
    }

    function ensureSelection() {
      const devices = draftDevices();
      if (!devices.some(function (device) { return device.id === selectedId; })) {
        const preferred = devices.find(function (device) { return device.enabled; }) || devices[0];
        selectedId = preferred ? preferred.id : null;
        previewSamples = null;
        previewNote = '';
        chartKey = '';
      }
    }

    function resetPreview() {
      previewSamples = null;
      previewNote = '';
      chartKey = '';
    }

    // ------------------------------------------------------------ device list
    function createDeviceCard() {
      const item = u.create('li', 'device-card');
      const icon = u.create('span', 'device-icon');
      const main = u.create('button', 'device-card-main');
      main.type = 'button';
      const titleLine = u.create('span', 'device-card-title');
      const name = u.create('span', 'device-name');
      const pill = u.create('span', 'pill neutral');
      titleLine.appendChild(name);
      titleLine.appendChild(pill);
      const sub = u.create('span', 'device-card-sub');
      main.appendChild(titleLine);
      main.appendChild(sub);
      const controls = u.create('div', 'device-card-controls');
      const toggle = u.create('button', 'switch');
      toggle.type = 'button';
      toggle.setAttribute('role', 'switch');
      toggle.setAttribute('aria-checked', 'false');
      toggle.appendChild(u.create('span'));
      const remove = u.create('button', 'icon-btn danger device-delete');
      remove.type = 'button';
      remove.appendChild(u.svgIcon('trash'));
      controls.appendChild(toggle);
      controls.appendChild(remove);
      item.appendChild(icon);
      item.appendChild(main);
      item.appendChild(controls);
      return item;
    }

    function updateDeviceCard(item, device) {
      const online = deviceOnline(device);
      const kind = u.deviceKind(device.name, device.id, (deviceInfo(device) || {}).card);
      const state = deviceState(device);
      item.dataset.id = device.id;
      item.classList.toggle('selected', device.id === selectedId);
      item.classList.toggle('offline', !online);
      const icon = item.querySelector('.device-icon');
      u.clearNode(icon);
      icon.appendChild(u.svgIcon(kind.icon));
      const name = item.querySelector('.device-name');
      u.setText(name, device.name || device.id);
      name.title = device.id;
      const pill = item.querySelector('.pill');
      u.setText(pill, state.text);
      pill.className = 'pill ' + state.tone;
      u.setText(item.querySelector('.device-card-sub'), deviceSubtitle(device));
      const main = item.querySelector('.device-card-main');
      main.setAttribute('aria-pressed', device.id === selectedId ? 'true' : 'false');
      main.setAttribute('aria-label', '选择音箱 ' + (device.name || device.id));
      const toggle = item.querySelector('.switch');
      const assigned = assignedSpeaker(device.id);
      toggle.disabled = !!assigned;
      u.setSwitch(toggle, !!device.enabled);
      toggle.setAttribute('aria-label', (device.enabled ? '停用' : '启用') + '音箱 ' + (device.name || device.id));
      const remove = item.querySelector('.device-delete');
      remove.disabled = !!assigned;
      remove.setAttribute('aria-label', '删除音箱配置 ' + (device.name || device.id));
      const pending = pendingDeleteId === device.id;
      remove.classList.toggle('confirm', pending);
      u.clearNode(remove);
      if (pending) {
        remove.appendChild(document.createTextNode('确认删除'));
      } else {
        remove.appendChild(u.svgIcon('trash'));
      }
    }

    function renderDeviceList() {
      const list = el('audio-devices');
      const devices = draftDevices();
      const existing = new Map();
      Array.prototype.slice.call(list.children).forEach(function (item) {
        existing.set(item.dataset.id, item);
      });
      devices.forEach(function (device) {
        let item = existing.get(device.id);
        if (!item) item = createDeviceCard();
        updateDeviceCard(item, device);
        list.appendChild(item);
        existing.delete(device.id);
      });
      existing.forEach(function (item) { item.remove(); });

      const hint = el('audio-devices-hint');
      if (!store.getDraft()) {
        u.setText(hint, '正在加载配置…');
        u.show(hint, true);
      } else if (!devices.length) {
        u.setText(hint, '尚未配置输出音箱。可在下方「可添加的输出设备」中添加，或点击「刷新设备」。');
        u.show(hint, true);
      } else {
        u.show(hint, false);
      }
      if (!isNarrow()) {
        document.body.classList.remove('audio-detail-open');
      }
    }

    function renderAddList() {
      const wrap = el('audio-add-wrap');
      const list = el('audio-add-list');
      const configured = new Set(draftDevices().map(function (device) { return device.id; }));
      const addable = store.state.discovered.filter(function (device) {
        return device.online && !configured.has(device.id);
      });
      u.clearNode(list);
      if (!addable.length) {
        u.show(wrap, false);
        return;
      }
      u.show(wrap, true);
      addable.forEach(function (info) {
        const kind = u.deviceKind(info.name, info.id, info.card);
        const item = u.create('li');
        const icon = u.create('span', 'device-mini-icon');
        icon.appendChild(u.svgIcon(kind.icon, 'small'));
        item.appendChild(icon);
        const text = u.create('div');
        text.style.flex = '1';
        text.style.minWidth = '0';
        text.appendChild(u.create('div', 'device-mini-name', info.name || info.id));
        const sub = kind.label + (info.sample_rates && info.sample_rates.length
          ? ' · ' + info.sample_rates.map(function (rate) { return u.fmtRate(rate); }).join(' / ')
          : '');
        text.appendChild(u.create('div', 'device-card-sub', sub));
        item.appendChild(text);
        const add = u.create('button', 'btn small', '添加');
        add.type = 'button';
        add.dataset.addId = info.id;
        add.setAttribute('aria-label', '添加输出音箱 ' + (info.name || info.id));
        item.appendChild(add);
        list.appendChild(item);
      });
    }

    function renderGlobal() {
      const draft = store.getDraft();
      const attenuation = draft && draft.audio ? u.finite(draft.audio.global_attenuation_percent, 100) : 100;
      u.setRange(el('audio-global'), attenuation);
      u.setText(el('audio-global-value'), Math.round(attenuation) + '%');
      u.setSwitch(el('audio-muted'), !!(draft && draft.audio && draft.audio.muted));
    }

    // -------------------------------------------------------------- curve edit
    function curveKeyOf(curve) {
      return JSON.stringify(curve);
    }

    function schedulePreview() {
      if (previewTimer) clearTimeout(previewTimer);
      const seq = ++previewSeq;
      previewNote = '正在获取后端预览…';
      updatePreviewStatus();
      previewTimer = setTimeout(function () {
        previewTimer = null;
        const id = selectedId;
        const device = id ? store.getDevice(id) : null;
        if (!device) return;
        const curve = store.normalizeCurve(device.curve);
        FP.api.preview(curve).then(function (result) {
          if (seq !== previewSeq || selectedId !== id) return;
          const samples = result && Array.isArray(result.samples) ? result.samples : null;
          if (samples && samples.length) {
            previewSamples = { key: curveKeyOf(curve), samples: samples };
            previewNote = '';
            const current = store.getDevice(id);
            if (current) {
              chart.setData(curveData(current.curve, samples));
            }
          }
          updatePreviewStatus();
        }).catch(function () {
          if (seq !== previewSeq || selectedId !== id) return;
          previewSamples = null;
          previewNote = '后端预览不可用，已用本地公式绘制（保存仍由后端校验）。';
          const current = store.getDevice(id);
          if (current) chart.setData(curveData(current.curve, null));
          updatePreviewStatus();
        });
      }, 220);
    }

    function updatePreviewStatus() {
      u.setText(el('curve-preview-status'), previewNote);
    }

    function curveData(curve, samples) {
      const normalized = store.normalizeCurve(curve);
      return {
        kind: normalized.kind,
        points: normalized.points,
        samples: samples || null,
        editable: normalized.kind === 'custom'
      };
    }

    function commitCurve(curve) {
      if (!selectedId) return;
      const normalized = store.normalizeCurve(curve);
      store.setDeviceField(selectedId, 'curve', normalized);
      previewSamples = null;
      chartKey = '';
      renderCurveControls();
      renderDeviceList();
    }

    function setCurveKind(kind) {
      const device = selectedId ? store.getDevice(selectedId) : null;
      if (!device) return;
      const current = store.normalizeCurve(device.curve);
      let points = current.points;
      if (kind === 'custom' && !points.length) points = FP.curve.neutralPoints();
      commitCurve({ kind: kind, points: points });
    }

    function handleChartChange(points) {
      if (!selectedId) return;
      const device = store.getDevice(selectedId);
      if (!device) return;
      const normalized = store.normalizePoints(points);
      store.setDeviceField(selectedId, 'curve', { kind: 'custom', points: normalized });
      previewSamples = null;
      const current = store.getDevice(selectedId);
      const curve = store.normalizeCurve(current ? current.curve : { kind: 'custom', points: normalized });
      chartKey = curveKeyOf(curve);
      chart.setData(curveData(curve, null));
      renderPointEditor(curve);
      renderDeviceList();
      schedulePreview();
    }

    function renderPointEditor(curve) {
      const container = el('curve-points');
      const points = curve.points || [];
      const expected = points.length + 2;
      if (container.children.length !== expected) {
        u.clearNode(container);
        container.appendChild(fixedPointRow('起点（固定）', '发送端 0% → 输出 0%'));
        points.forEach(function (point, index) {
          container.appendChild(controlPointRow(index, point));
        });
        container.appendChild(fixedPointRow('终点（固定）', '发送端 100% → 输出 100%'));
      } else {
        const rows = container.querySelectorAll('.point-row');
        rows.forEach(function (row, index) {
          const point = points[index];
          if (!point) return;
          const inputs = row.querySelectorAll('input');
          if (document.activeElement !== inputs[0]) inputs[0].value = String(Math.round(point[0] * 100));
          if (document.activeElement !== inputs[1]) inputs[1].value = String(Math.round(point[1] * 100));
        });
      }
    }

    function fixedPointRow(label, value) {
      const row = u.create('div', 'point-fixed');
      row.appendChild(u.create('span', null, label));
      row.appendChild(u.create('b', null, value));
      return row;
    }

    function controlPointRow(index, point) {
      const row = u.create('div', 'point-row');
      row.dataset.index = String(index);
      row.appendChild(u.create('span', 'point-label', '控制点 ' + (index + 1)));
      row.appendChild(pointInput(index, 0, 'X', Math.round(point[0] * 100), '控制点' + (index + 1) + '发送端音量百分比'));
      row.appendChild(pointInput(index, 1, 'Y', Math.round(point[1] * 100), '控制点' + (index + 1) + '输出幅度百分比'));
      const remove = u.create('button', 'icon-btn danger');
      remove.type = 'button';
      remove.setAttribute('aria-label', '删除控制点 ' + (index + 1));
      remove.appendChild(u.svgIcon('trash'));
      remove.dataset.removeIndex = String(index);
      row.appendChild(remove);
      return row;
    }

    function pointInput(index, axis, label, value, ariaLabel) {
      const wrap = u.create('label', 'point-input');
      wrap.appendChild(u.create('span', null, label));
      const input = document.createElement('input');
      input.type = 'number';
      input.min = '0';
      input.max = '100';
      input.step = '1';
      input.inputMode = 'numeric';
      input.value = String(value);
      input.dataset.index = String(index);
      input.dataset.axis = String(axis);
      input.setAttribute('aria-label', ariaLabel);
      wrap.appendChild(input);
      return wrap;
    }

    function updatePointValue(index, axis, raw) {
      if (raw == null || String(raw).trim() === '') return;
      const device = selectedId ? store.getDevice(selectedId) : null;
      if (!device) return;
      const curve = store.normalizeCurve(device.curve);
      if (curve.kind !== 'custom') return;
      const points = curve.points.map(function (point) { return point.slice(); });
      if (!points[index]) return;
      let value = Number(raw);
      if (!Number.isFinite(value)) return;
      value = Math.round(u.clamp(value, 0, 100)) / 100;
      const prev = index > 0 ? points[index - 1] : [0, 0];
      const next = index < points.length - 1 ? points[index + 1] : [1, 1];
      if (axis === 0) value = u.clamp(value, prev[0], next[0]);
      else value = u.clamp(value, prev[1], next[1]);
      points[index][axis] = Math.round(value * 10000) / 10000;
      commitCurve({ kind: 'custom', points: points });
      const current = store.getDevice(selectedId);
      if (!current) return;
      const currentPoint = store.normalizeCurve(current.curve).points[index];
      const row = el('curve-points').querySelector('.point-row[data-index="' + index + '"]');
      if (!row || !currentPoint) return;
      const inputs = row.querySelectorAll('input');
      [0, 1].forEach(function (position) {
        const expected = String(Math.round(currentPoint[position] * 100));
        if (inputs[position].value !== expected) inputs[position].value = expected;
      });
    }

    function removePointAt(index) {
      const device = selectedId ? store.getDevice(selectedId) : null;
      if (!device) return;
      const curve = store.normalizeCurve(device.curve);
      if (curve.kind !== 'custom') return;
      const points = curve.points.slice();
      if (index < 0 || index >= points.length) return;
      points.splice(index, 1);
      commitCurve({ kind: 'custom', points: points });
    }

    function renderCurveControls() {
      const device = selectedId ? store.getDevice(selectedId) : null;
      if (!device) return;
      const curve = store.normalizeCurve(device.curve);
      u.qsa('#curve-presets button').forEach(function (button) {
        button.setAttribute('aria-pressed', button.dataset.curveKind === curve.kind ? 'true' : 'false');
      });
      u.show(el('curve-editor'), curve.kind === 'custom');
      const canvas = el('curve-canvas');
      if (canvas) {
        canvas.setAttribute('aria-label', curve.kind === 'custom'
          ? '独立音量曲线编辑器：自定义模式，选中控制点后可用方向键微调'
          : '独立音量曲线图：' + u.curveKindLabel(curve.kind) + '预设');
      }
      const key = curveKeyOf(curve);
      if (key !== chartKey) {
        chartKey = key;
        const samples = previewSamples && previewSamples.key === key ? previewSamples.samples : null;
        chart.setData(curveData(curve, samples));
        schedulePreview();
      }
      renderPointEditor(curve);
      updatePreviewStatus();
    }

    // ------------------------------------------------------------ detail panel
    function renderRateOptions(device, info) {
      const rates = new Set([44100, 48000]);
      if (info && Array.isArray(info.sample_rates)) {
        info.sample_rates.forEach(function (rate) {
          if (Number(rate) > 0) rates.add(Number(rate));
        });
      }
      if (Number(device.sample_rate) > 0) rates.add(Number(device.sample_rate));
      const sorted = Array.from(rates).sort(function (a, b) { return a - b; });
      const signature = sorted.join(',');
      const select = el('audio-rate');
      if (signature !== rateSig) {
        rateSig = signature;
        u.clearNode(select);
        const follow = document.createElement('option');
        follow.value = '0';
        follow.textContent = '跟随设备默认采样率';
        select.appendChild(follow);
        sorted.forEach(function (rate) {
          const option = document.createElement('option');
          option.value = String(rate);
          option.textContent = u.fmtRate(rate);
          select.appendChild(option);
        });
      }
      u.setSelect(select, String(device.sample_rate || 0));
    }

    function renderAdvanced(device, runtime, info) {
      const kv = el('audio-advanced-kv');
      u.clearNode(kv);
      const rows = [
        ['ALSA 设备 ID', device.id, true],
        ['声道', u.fmtChannels((runtime && runtime.channels) || (info && info.channels) || 0), false],
        ['输出格式', runtime && runtime.format ? runtime.format : '—', true],
        ['缓冲深度', runtime ? u.fmtMs(runtime.buffer_ms) : '—', false],
        ['时钟漂移', runtime ? u.fmtPpm(runtime.drift_ppm) : '—', false],
        ['underrun', runtime ? u.fmtCount(runtime.underruns) : '—', false],
        ['已写帧数', runtime ? u.fmtCount(runtime.written_frames) : '—', false],
        ['支持采样率', info && info.sample_rates && info.sample_rates.length
          ? info.sample_rates.map(function (rate) { return u.fmtRate(rate); }).join(' / ')
          : '未上报', false]
      ];
      rows.forEach(function (row) {
        const item = u.create('div');
        item.appendChild(u.create('dt', null, row[0]));
        const dd = u.create('dd', row[2] ? 'mono' : null, String(row[1]));
        item.appendChild(dd);
        kv.appendChild(item);
      });
      const mode = store.state.status && store.state.status.audio && store.state.status.audio.mode;
      u.setText(el('audio-test-hint'), mode === 'null'
        ? '当前为 null 音频后端（无实际声卡），测试音请求会被接受但不会发声。'
        : '播放约 1 秒的真实测试音，用于确认该音箱已经接通。');
    }

    function renderPanel() {
      const device = selectedId ? store.getDevice(selectedId) : null;
      u.show(el('audio-device-panel'), !!device);
      u.show(el('audio-no-selection'), !device);
      if (!device) {
        const emptySub = el('audio-empty-sub');
        if (!store.getDraft()) {
          u.setText(emptySub, '正在加载配置…');
        } else {
          u.setText(emptySub, draftDevices().length
            ? '从左侧设备列表选择音箱，即可调整响度、同步延迟与独立音量曲线。'
            : '尚未配置输出音箱。请先在左侧列表中添加设备。');
        }
        return;
      }

      const runtime = store.getDeviceStatus(device.id);
      const info = deviceInfo(device);
      const kind = u.deviceKind(device.name, device.id, info && info.card);
      const state = deviceState(device);

      u.setText(el('audio-panel-name'), device.name || device.id);
      u.setText(el('audio-panel-type'),
        kind.label + ' · ' + u.fmtRate(runtime && runtime.sample_rate ? runtime.sample_rate :
          ((info && info.sample_rates && info.sample_rates[0]) || 0)) +
        (deviceOnline(device) ? '' : ' · 当前离线'));
      const iconWrap = el('audio-panel-icon');
      u.clearNode(iconWrap);
      iconWrap.appendChild(u.svgIcon(kind.icon));
      const statePill = el('audio-panel-state');
      u.setText(statePill, state.text);
      statePill.className = 'pill ' + state.tone;

      const gain = u.clamp(u.finite(device.gain_db, 0), -30, 0);
      u.setRange(el('audio-gain'), gain);
      u.setText(el('audio-gain-value'), u.round(gain, 1) + ' dB');
      const delay = u.clamp(u.finite(device.delay_ms, 0), 0, 2000);
      u.setRange(el('audio-delay-range'), delay);
      u.setText(el('audio-delay-value'), Math.round(delay) + ' ms');
      u.setInput(el('audio-delay-number'), String(Math.round(delay)));

      renderRateOptions(device, info);
      renderAdvanced(device, runtime, info);
      renderCurveControls();
    }

    let groupsSignature;
    function renderGroups() {
      const audio = (store.getDraft() || {}).audio;
      if (!audio) return;
      const groups = audio.output_groups || [];
      const enabled = audio.devices.filter(d => d.enabled).map(d => d.id);
      const signature = JSON.stringify([groups, enabled, audio.devices.map(d => d.id), audio.instances]);
      if (groupsSignature === signature) return;
      groupsSignature = signature;
      const list = el('output-groups');
      u.clearNode(list);
      for (const group of groups) {
        const row = u.create('li', 'card-foot');
        const selected = group.devices.length === enabled.length && enabled.every(id => group.devices.includes(id));
        const apply = u.create('button', 'btn', group.name);
        apply.type = 'button';
        apply.setAttribute('aria-pressed', String(selected));
        apply.disabled = selected;
        apply.onclick = function () {
          if (group.devices.some(id => assignedSpeaker(id))) {
            ctx.toast('此方案包含独立音箱，请先调整音箱分配。', 'warn');
            return;
          }
          const devices = store.getDraft().audio.devices;
          if (group.devices.some(id => !devices.some(d => d.id === id))) {
            ctx.toast('方案中的音箱已被移除，请重新添加设备。', 'warn');
            return;
          }
          devices.forEach(d => store.setDeviceField(d.id, 'enabled', group.devices.includes(d.id)));
          render();
        };
        const remove = u.create('button', 'btn small', '移除');
        remove.type = 'button';
        remove.setAttribute('aria-label', '移除输出方案 ' + group.name);
        remove.onclick = function () {
          store.setGlobal('output_groups', store.getDraft().audio.output_groups.filter(g => g.name !== group.name));
          render();
        };
        row.append(apply, remove);
        list.appendChild(row);
      }
    }

    function render() {
      if (!active) return;
      ensureSelection();
      renderDeviceList();
      renderGroups();
      speakerSettings.render();
      renderAddList();
      renderGlobal();
      renderPanel();
    }

    // ------------------------------------------------------------------- data
    function loadDiscovery(silent) {
      return FP.api.devices().then(function (result) {
        store.setDiscovered((result && result.devices) || []);
        render();
      }).catch(function (error) {
        store.setDiscovered(store.state.discovered, error.message);
        if (!silent) toast('设备列表获取失败：' + error.message, 'error');
      });
    }

    function selectDevice(id, openDetail) {
      if (!store.getDevice(id)) return;
      selectedId = id;
      resetPreview();
      if (openDetail && isNarrow()) document.body.classList.add('audio-detail-open');
      render();
    }

    function toggleDevice(id) {
      const device = store.getDevice(id);
      if (!device) return;
      store.setDeviceField(id, 'enabled', !device.enabled);
      render();
    }

    function handleDelete(id) {
      if (pendingDeleteId !== id) {
        pendingDeleteId = id;
        if (deleteTimer) clearTimeout(deleteTimer);
        deleteTimer = setTimeout(function () {
          pendingDeleteId = null;
          deleteTimer = null;
          renderDeviceList();
        }, 3500);
        renderDeviceList();
        return;
      }
      if (deleteTimer) clearTimeout(deleteTimer);
      deleteTimer = null;
      pendingDeleteId = null;
      const wasAdded = !store.getBaselineDevice(id);
      store.removeDevice(id);
      if (selectedId === id) {
        selectedId = null;
        resetPreview();
      }
      render();
      toast(wasAdded ? '已移除该音箱配置。' : '已标记移除该音箱，保存后生效。', 'info');
    }

    function addDiscovered(id) {
      const info = store.getDiscoveredDevice(id);
      if (!info) return;
      store.addDevice({
        id: info.id,
        name: info.name || info.id,
        enabled: true,
        gain_db: 0,
        delay_ms: 0,
        sample_rate: 0,
        curve: { kind: 'balanced', points: [] }
      });
      selectedId = id;
      resetPreview();
      if (isNarrow()) document.body.classList.add('audio-detail-open');
      render();
      toast('已添加音箱，保存并应用后开始输出。', 'ok');
    }

    function refreshDevices() {
      if (refreshing) return;
      refreshing = true;
      const button = el('audio-refresh');
      button.disabled = true;
      button.setAttribute('aria-busy', 'true');
      FP.api.refreshDevices().then(function () {
        return loadDiscovery(true);
      }).then(function () {
        toast('设备列表已刷新。', 'ok');
      }).catch(function (error) {
        toast('刷新设备失败：' + error.message, 'error');
      }).finally(function () {
        refreshing = false;
        button.disabled = false;
        button.removeAttribute('aria-busy');
        render();
      });
    }

    function testTone() {
      const device = selectedId ? store.getDevice(selectedId) : null;
      if (!device) return;
      const button = el('audio-test');
      button.disabled = true;
      FP.api.testTone(device.id).then(function () {
        const mode = store.state.status && store.state.status.audio && store.state.status.audio.mode;
        if (mode === 'null') {
          toast('测试音请求已发送；当前为 null 音频后端，不会实际发声。', 'warn');
        } else {
          toast('已发送测试音请求（约 1 秒）。', 'ok');
        }
      }).catch(function (error) {
        toast('测试音失败：' + error.message, 'error');
      }).finally(function () {
        button.disabled = false;
      });
    }

    // ------------------------------------------------------------------ bind
    function bind() {
      if (bound) return;
      bound = true;

      chart = FP.createCurveChart(el('curve-canvas'), {
        onCurveChange: function (points) { handleChartChange(points); }
      }) || {
        setData: function () {},
        redraw: function () {},
        addPoint: function () { return false; },
        removeSelected: function () { return false; },
        resetPoints: function () {}
      };

      el('group-create').addEventListener('click', function () {
        const audio = (store.getDraft() || {}).audio;
        if (!audio) return;
        const name = el('group-name').value.trim();
        const groups = audio.output_groups || [];
        const devices = audio.devices.filter(d => d.enabled).map(d => d.id);
        if (!name || !devices.length || groups.some(g => g.name === name)) {
          ctx.toast('请启用至少一个音箱，并填写未使用的方案名称。', 'warn');
          return;
        }
        store.setGlobal('output_groups', groups.concat([{name, devices}]));
        el('group-name').value = '';
        render();
      });
      el('audio-refresh').addEventListener('click', refreshDevices);
      el('audio-back').addEventListener('click', function () {
        document.body.classList.remove('audio-detail-open');
        render();
      });

      const list = el('audio-devices');
      list.addEventListener('click', function (event) {
        const item = event.target.closest('.device-card');
        if (!item || !item.dataset.id) return;
        const id = item.dataset.id;
        if (event.target.closest('.device-delete')) {
          handleDelete(id);
          return;
        }
        if (event.target.closest('.switch')) {
          toggleDevice(id);
          return;
        }
        selectDevice(id, true);
      });
      list.addEventListener('keydown', function (event) {
        if (event.key !== 'Enter' && event.key !== ' ') return;
        const main = event.target.closest('.device-card-main');
        if (!main) return;
        const item = main.closest('.device-card');
        if (!item) return;
        event.preventDefault();
        selectDevice(item.dataset.id, true);
      });

      el('audio-add-list').addEventListener('click', function (event) {
        const button = event.target.closest('[data-add-id]');
        if (button) addDiscovered(button.dataset.addId);
      });

      const globalRange = el('audio-global');
      globalRange.addEventListener('input', function () {
        const value = u.clamp(u.finite(globalRange.value, 100), 0, 100);
        store.setGlobal('global_attenuation_percent', value);
        u.setText(el('audio-global-value'), Math.round(value) + '%');
        u.syncRange(globalRange);
      });
      el('audio-muted').addEventListener('click', function () {
        const draft = store.getDraft();
        store.setGlobal('muted', !(draft && draft.audio && draft.audio.muted));
        renderGlobal();
      });

      const gainRange = el('audio-gain');
      gainRange.addEventListener('input', function () {
        if (!selectedId) return;
        const value = u.clamp(u.finite(gainRange.value, 0), -30, 0);
        store.setDeviceField(selectedId, 'gain_db', u.round(value, 1));
        u.setText(el('audio-gain-value'), u.round(value, 1) + ' dB');
        u.syncRange(gainRange);
      });

      const delayRange = el('audio-delay-range');
      const delayNumber = el('audio-delay-number');
      delayRange.addEventListener('input', function () {
        if (!selectedId) return;
        const value = Math.round(u.clamp(u.finite(delayRange.value, 0), 0, 2000));
        store.setDeviceField(selectedId, 'delay_ms', value);
        delayNumber.value = String(value);
        u.setText(el('audio-delay-value'), value + ' ms');
        u.syncRange(delayRange);
      });
      delayNumber.addEventListener('input', function () {
        if (!selectedId) return;
        const raw = Number(delayNumber.value);
        if (!Number.isFinite(raw)) return;
        const value = Math.round(u.clamp(raw, 0, 2000));
        store.setDeviceField(selectedId, 'delay_ms', value);
        u.setText(el('audio-delay-value'), value + ' ms');
        u.setRange(delayRange, value);
      });
      delayNumber.addEventListener('blur', function () {
        if (!selectedId) return;
        const device = store.getDevice(selectedId);
        delayNumber.value = String(Math.round(u.finite(device && device.delay_ms, 0)));
      });

      el('audio-rate').addEventListener('change', function () {
        if (!selectedId) return;
        store.setDeviceField(selectedId, 'sample_rate', Math.max(0, Math.round(u.finite(el('audio-rate').value, 0))));
      });

      el('curve-presets').addEventListener('click', function (event) {
        const button = event.target.closest('button[data-curve-kind]');
        if (button) setCurveKind(button.dataset.curveKind);
      });
      el('curve-add').addEventListener('click', function () {
        if (!chart.addPoint()) toast('没有可插入控制点的区间。', 'info');
      });
      el('curve-remove').addEventListener('click', function () {
        if (!chart.removeSelected()) toast('请先在曲线上选择一个控制点。', 'info');
      });
      el('curve-reset').addEventListener('click', function () {
        chart.resetPoints();
        toast('已重置为中性曲线。', 'info');
      });

      const points = el('curve-points');
      points.addEventListener('input', function (event) {
        const input = event.target;
        if (!input.dataset || input.dataset.axis == null) return;
        updatePointValue(Number(input.dataset.index), Number(input.dataset.axis), input.value);
      });
      points.addEventListener('click', function (event) {
        const button = event.target.closest('[data-remove-index]');
        if (button) removePointAt(Number(button.dataset.removeIndex));
      });
      points.addEventListener('focusout', function (event) {
        const input = event.target;
        if (!input.dataset || input.dataset.axis == null) return;
        const device = selectedId ? store.getDevice(selectedId) : null;
        if (!device) return;
        renderPointEditor(store.normalizeCurve(device.curve));
      });

      el('audio-test').addEventListener('click', testTone);

      document.addEventListener('fp:theme', function () {
        if (chart) chart.redraw();
      });
    }

    return {
      enter: function () {
        active = true;
        bind();
        document.body.classList.remove('audio-detail-open');
        ensureSelection();
        render();
        loadDiscovery(true);
      },
      leave: function () {
        active = false;
        document.body.classList.remove('audio-detail-open');
        if (previewTimer) {
          clearTimeout(previewTimer);
          previewTimer = null;
        }
        previewSeq += 1;
      },
      render: render,
      onTheme: function () { if (chart) chart.redraw(); }
    };
  };
})(window.FP);
