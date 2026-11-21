'use strict';

window.FP.createSpeakerSettings = function (ctx, renderAudio) {
  const {store, toast} = ctx;
  const {byId, create} = FP.util;
  const form = byId('instance-form');
  const list = byId('speaker-instances');
  const name = byId('instance-name');
  const device = byId('instance-device');
  let editing = null;
  let listKey, optionsKey;

  function instances() { return store.getDraft().audio.instances || []; }

  // Include configured offline outputs, including those restored from a backup.
  function devices() {
    const outputs = new Map();
    instances().forEach(s => outputs.set(s.device_id, s.device_id));
    store.getDraft().audio.devices.forEach(d => outputs.set(d.id, d.name || d.id));
    store.state.discovered.forEach(d => outputs.set(d.id, d.name || d.id));
    return outputs;
  }

  function updateHint() {
    const output = store.getDevice(device.value);
    byId('instance-assignment-hint').textContent = !device.value
      ? '没有可分配的音箱。连接设备后，请刷新输出设备。'
      : editing === 'default' ? '保存后，主接收器将使用这只音箱，并保留已启用的同步输出。'
        : output && output.enabled ? '保存并应用后，此音箱将离开主接收器，开始独立接收。'
        : '保存并应用后生效。';
    form.querySelector('[type="submit"]').disabled = !device.value;
  }

  function renderOptions(outputs) {
    const options = Array.from(outputs, ([id, label]) => {
      const owner = instances().find(s => s.id !== editing && s.device_id === id);
      return {id, label, owner: owner?.name};
    });
    const key = JSON.stringify([editing, options]);
    if (key !== optionsKey) {
      const previous = device.value;
      const current = instances().find(s => s.id === editing);
      device.replaceChildren(...options.map(({id, label, owner}) => {
        const option = new Option(owner ? label + ' · 已绑定「' + owner + '」' : label, id);
        option.disabled = Boolean(owner);
        return option;
      }));
      const available = options.filter(option => !option.owner);
      const chosen = available.find(o => o.id === previous) ||
        available.find(o => o.id === current?.device_id) ||
        (editing === 'default' && available.find(o => store.getDevice(o.id)?.enabled)) ||
        available[0];
      device.value = chosen ? chosen.id : '';
      optionsKey = key;
    }
    updateHint();
  }

  function open(instance) {
    editing = instance.id;
    optionsKey = undefined;
    name.value = instance.name;
    device.replaceChildren();
    form.hidden = false;
    renderOptions(devices());
    name.focus();
  }

  function close() {
    editing = null;
    form.hidden = true;
    byId('new-instance').focus();
  }

  function render() {
    const draft = store.getDraft();
    if (!draft) return;
    byId('new-instance').disabled = false;
    const outputs = devices();
    if (!form.hidden) renderOptions(outputs);
    const saved = (store.state.baseline.audio.instances || []).map(s => s.id);
    const primaryOutputs = draft.audio.devices.filter(d => d.enabled).map(d => outputs.get(d.id));
    const activeMain = FP.primaryActive(store.state.baseline.audio);
    const rows = [{ id: 'default', name: draft.receiver.name }, ...instances()];
    const key = JSON.stringify([rows, saved, activeMain, primaryOutputs, Array.from(outputs)]);
    if (key === listKey) return;
    listKey = key;
    list.replaceChildren();
    rows.forEach(s => {
      const main = s.id === 'default';
      const row = create('li', 'speaker-instance');
      const info = create('div', 'instance-info');
      info.append(create('strong', '', s.name), create('span', 'hint',
        main ? '主接收器 · ' + (primaryOutputs.join('、') || '可分配第一只音箱') : outputs.get(s.device_id) || s.device_id));
      const actions = create('div', 'instance-actions');
      if ((main && activeMain) || saved.includes(s.id)) {
        const player = create('a', 'text-link', '播放页 ↗');
        player.href = FP.playerUrl(s.id);
        player.target = '_blank';
        player.rel = 'noopener';
        player.setAttribute('aria-label', '在新标签页打开' + s.name + '播放器');
        actions.append(player);
      } else {
        actions.append(create('span', 'hint', main ? '未启动' : '待保存'));
      }
      if (main) {
        const assign = create('button', 'btn small', '分配主音箱');
        assign.type = 'button';
        assign.onclick = () => open(s);
        actions.append(assign);
      } else {
        const edit = create('button', 'btn small', '编辑');
        edit.type = 'button';
        edit.onclick = () => open(s);
        const remove = create('button', 'btn small', '移除');
        remove.type = 'button';
        remove.onclick = () => {
          store.setGlobal('instances', instances().filter(other => other.id !== s.id));
          if (editing === s.id) close();
          renderAudio();
          toast('已标记移除，保存后该播放器将关闭。', 'info');
        };
        actions.append(edit, remove);
      }
      row.append(info, actions);
      list.append(row);
    });
  }

  byId('new-instance').onclick = () => {
    if (store.hasDirty()) { toast('请先保存或撤销当前修改。','warn'); return; }
    const target = new URL('speakers.html',document.baseURI);
    target.search = location.search;
    target.hash = 'new';
    location.assign(target);
  };
  byId('cancel-instance').onclick = close;
  device.onchange = updateHint;
  form.onsubmit = event => {
    event.preventDefault();
    const label = name.value.trim();
    const draft = store.getDraft();
    const canonical = label.toLowerCase();
    if (!label || (editing !== 'default' && draft.receiver.name.trim().toLowerCase() === canonical) ||
        instances().some(s => s.id !== editing && s.name.trim().toLowerCase() === canonical)) {
      toast('请填写尚未使用的投送名称。', 'warn');
      name.focus();
      return;
    }
    if (!device.value || instances().some(s => s.id !== editing && s.device_id === device.value)) {
      toast('请选择未分配的音箱。', 'warn');
      return;
    }
    const output = store.getDevice(device.value);
    if (editing === 'default') {
      if (output) store.setDeviceField(output.id, 'enabled', true);
      else store.addDevice({id: device.value, name: devices().get(device.value), enabled: true});
      store.setReceiverField('name', label);
      close();
      renderAudio();
      return;
    }
    if (!instances().some(s => s.id === editing)) {
      toast('此音箱已被移除，请重新选择。', 'warn');
      close();
      renderAudio();
      return;
    }
    if (output && output.enabled) store.setDeviceField(output.id, 'enabled', false);
    if (!output) store.addDevice({id: device.value, name: devices().get(device.value), enabled: false});
    const next = {id: editing, name: label, device_id: device.value};
    store.setGlobal('instances', instances().map(s => s.id === editing ? next : s));
    close();
    renderAudio();
  };
  return { render };
};
