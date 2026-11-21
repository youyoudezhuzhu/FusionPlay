'use strict';

// Keep the draft local until Save. The server compares the original audio
// configuration under its write lock before changing hardware or instances.
FP.editOutputs = async function (original, fail, changed) {
  const $ = id => document.getElementById(id);
  const clone = value => JSON.parse(JSON.stringify(value));
  const draft = clone(original);
  const select = $('output-select');
  const save = $('outputs-save');
  let discovered = (await FP.api.devices()).devices;
  let busy = false;

  function edited() {
    const dirty = JSON.stringify(draft) !== JSON.stringify(original);
    changed(dirty);
    save.disabled = busy || !dirty;
  }
  function selected() { return draft.devices.find(device => device.id === select.value); }
  function available() {
    return new Map([...discovered, ...draft.devices].map(device => [device.id, device]));
  }
  function owner(id) { return draft.instances?.find(room => room.device_id === id); }
  function selectOutput() {
    const device = selected();
    const info = discovered.find(item => item.id === select.value);
    const room = owner(select.value);
    $('output-fields').disabled = !select.options.length;
    $('output-owner').textContent = room ? '已绑定「' + room.name + '」' :
      !info ? '当前离线' : device?.enabled ? '主接收器使用中' : '可分配给主接收器';
    $('output-enable').checked = Boolean(device?.enabled);
    $('output-enable').disabled = Boolean(room);
    $('output-forget').disabled = !device || Boolean(room);
    $('output-test').disabled = !original.devices.some(item => item.id === select.value);
    $('output-gain').value = device?.gain_db ?? 0;
    $('output-delay').value = device?.delay_ms ?? 0;
    const rate = device?.sample_rate || 0;
    const rates = [...new Set([0, rate, ...(info?.sample_rates || [])])].sort((a,b) => a-b);
    $('output-rate').replaceChildren(...rates.map(hz => new Option(hz ? hz + ' Hz' : '设备默认', hz)));
    $('output-rate').value = rate;
  }
  function listOutputs() {
    const id = select.value;
    const all = available();
    select.replaceChildren(...Array.from(all.values(), d => new Option(d.name || d.id, d.id)));
    if (all.has(id)) select.value = id;
    selectOutput();
  }
  function changeDevice(field, value) {
    let device = selected();
    if (!device) {
      const info = available().get(select.value);
      if (!info) return;
      device = {id:info.id, name:info.name || info.id, enabled:false, gain_db:0, delay_ms:0, sample_rate:0};
      draft.devices.push(device);
    }
    device[field] = value;
    edited();
  }
  function renderGroups() {
    const list = $('output-groups');
    list.replaceChildren();
    for (const group of draft.output_groups || []) {
      const row = document.createElement('div');
      row.className = 'toolbar';
      const apply = document.createElement('button');
      apply.type = 'button';
      apply.textContent = group.name;
      apply.onclick = () => {
        if (group.devices.some(id => owner(id) || !draft.devices.some(d => d.id === id))) {
          fail(new Error('此方案包含已移除或已独立绑定的音箱。'));
          return;
        }
        for (const device of draft.devices) device.enabled = group.devices.includes(device.id);
        selectOutput();
        edited();
      };
      const remove = document.createElement('button');
      remove.type = 'button';
      remove.textContent = '移除方案';
      remove.onclick = () => {
        draft.output_groups = draft.output_groups.filter(item => item !== group);
        renderGroups();
        edited();
      };
      row.append(apply, remove);
      list.append(row);
    }
  }
  select.onchange = selectOutput;
  $('output-enable').onchange = event => { changeDevice('enabled', event.target.checked); selectOutput(); };
  for (const [id, field] of [['output-gain','gain_db'], ['output-delay','delay_ms'], ['output-rate','sample_rate']]) {
    $(id).onchange = event => {
      if (!event.target.value || !event.target.reportValidity()) { selectOutput(); return; }
      changeDevice(field, Number(event.target.value));
    };
  }
  $('output-level').value = draft.global_attenuation_percent;
  $('output-level').oninput = event => {
    if (event.target.validity.valid) { draft.global_attenuation_percent = event.target.valueAsNumber; edited(); }
  };
  $('output-mute').checked = draft.muted;
  $('output-mute').onchange = event => { draft.muted = event.target.checked; edited(); };
  $('output-refresh').onclick = async () => {
    try {
      await FP.api.refreshDevices();
      discovered = (await FP.api.devices()).devices;
      listOutputs();
    } catch (error) { fail(error); }
  };
  $('output-test').onclick = async () => {
    try { await FP.api.testTone(select.value); $('notice').textContent = '已发送测试音'; }
    catch (error) { fail(error); }
  };
  $('output-forget').onclick = () => {
    const id = select.value;
    if (owner(id) || !confirm('保存后移除此音箱的校准及输出配置？')) return;
    draft.devices = draft.devices.filter(d => d.id !== id);
    if (draft.output_groups) draft.output_groups = draft.output_groups
      .map(g => ({...g, devices:g.devices.filter(device => device !== id)})).filter(g => g.devices.length);
    listOutputs(); renderGroups(); edited();
  };
  $('group-add').onclick = () => {
    const name = $('group-name').value.trim();
    const devices = draft.devices.filter(d => d.enabled).map(d => d.id);
    if (!name || !devices.length || draft.output_groups?.some(g => g.name === name)) {
      fail(new Error('请启用至少一个音箱，并填写未使用的方案名称。')); return;
    }
    (draft.output_groups ||= []).push({name, devices});
    $('group-name').value = '';
    renderGroups(); edited();
  };
  $('outputs-form').onsubmit = async event => {
    event.preventDefault();
    if (busy || save.disabled) return;
    busy = true;
    const form = $('outputs-form');
    form.inert = true; save.disabled = true;
    try {
      await FP.api.saveOutputs(original, draft);
      original = clone(draft);
      changed(false);
      location.reload();
    } catch (error) { fail(error); }
    finally { form.inert = false; busy = false; edited(); }
  };
  listOutputs();
  renderGroups();
};
