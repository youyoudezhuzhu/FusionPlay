'use strict';

(async function () {
  const $ = id => document.getElementById(id);
  const url = new URL(location.href);
  const section = ['curve','new'].includes(location.hash.slice(1)) ? location.hash.slice(1) : 'rooms';
  const api = FP.api;
  let roomId = url.searchParams.get('instance');
  let dirty = false;
  function page(id, section, device) {
    const target = new URL('speakers.html', document.baseURI);
    if (id) target.searchParams.set('instance', id);
    if (device) target.searchParams.set('device', device);
    target.hash = section;
    return target.href;
  }
  function player(id) {
    const target = new URL('player/index.html', document.baseURI);
    if (id) target.searchParams.set('instance', id);
    return target.href;
  }
  function node(tag, cls, text) {
    const el = document.createElement(tag);
    if (cls) el.className = cls;
    if (text) el.textContent = text;
    return el;
  }
  function link(label, href, cls) {
    const el = node('a', cls, label);
    el.href = href;
    return el;
  }
  function fail(error) {
    $('notice').textContent = error.message || String(error);
    $('retry').hidden = false;
  }
  $('retry').onclick = () => location.reload();
  window.addEventListener('hashchange', () => location.reload());
  window.addEventListener('pageshow', event => {
    if (event.persisted && !dirty) location.reload();
  });
  window.addEventListener('beforeunload', event => {
    if (dirty) { event.preventDefault(); event.returnValue = ''; }
  });
  $(section).hidden = false;
  $('back').href = player(roomId);

  try {
    const [catalog, config] = await Promise.all([api.instances(), api.config()]);
    roomId = roomId || catalog.default;
    $('back').href = player(roomId);
    $('all-rooms').href = page(roomId, 'rooms');
    $('add-room').href = page(roomId, 'new');
    const devices = config.audio.devices;
    const instances = config.audio.instances || [];
    const outputs = id => devices.filter(device => id === 'default' ? device.enabled :
      instances.some(s => s.id === id && s.device_id === device.id));
    const room = catalog.instances.find(s => s.id === roomId);
    const cards = new Map();
    if (section === 'rooms') {
      for (const s of catalog.instances) {
        const item = node('article','card room');
        const top = node('div','room-top');
        const symbol = node('span','speaker-symbol');
        const cover = node('img','cover'); cover.alt = ''; cover.hidden = true;
        cover.onload = () => { cover.hidden = false; symbol.hidden = true; };
        cover.onerror = () => { cover.hidden = true; symbol.hidden = false; };
        top.append(symbol, cover);
        if (s.id === roomId) top.append(node('span','current','当前界面'));
        const title = node('div');
        const media = node('p','media','正在读取播放状态…');
        title.append(node('h2','',s.name),media);
        const actions = node('div','room-actions');
        actions.append(link('打开播放器 ↗',player(s.id)));
        if (outputs(s.id).length) actions.append(link('音量曲线',page(s.id,'curve')));
        item.append(top,title,actions); $('room-list').append(item);
        cards.set(s.id,{cover,symbol,media});
      }
    }
    const scene = FP.initGlass();
    let refreshing = false;
    async function refreshMedia() {
      if (refreshing || document.hidden) return;
      refreshing = true;
      try {
        const ids = section === 'rooms' ? [...cards.keys()] : room ? [room.id] : [];
        for (let i = 0; i < ids.length; i += 4) {
          await Promise.all(ids.slice(i,i+4).map(async id => {
            const status = await api.status(id).catch(() => null);
            if (id === roomId) scene(status,id);
            const card = cards.get(id);
            if (!card) return;
            const protocol = status?.current_source;
            const media = protocol && status.media;
            const source = {airplay:'AirPlay',dlna:'DLNA',miplay:'妙播'}[protocol] || protocol;
            card.media.textContent = !status ? '接收器暂不可用' : !source ? '等待投送' :
              [source, media?.title].filter(Boolean).join(' · ');
            const coverUrl = media?.cover_url;
            if (coverUrl) {
              const path = coverUrl.startsWith('artwork/') ? FP.apiUrl('/api/instances/'+encodeURIComponent(id)+'/'+coverUrl) : coverUrl;
              if (card.cover.getAttribute('src') !== path) card.cover.src = path;
            } else {
              card.cover.removeAttribute('src'); card.cover.hidden = true; card.symbol.hidden = false;
            }
          }));
        }
      } finally { refreshing = false; }
    }
    refreshMedia();
    window.setInterval(refreshMedia, 4000);
    document.addEventListener('visibilitychange',refreshMedia);
    if (section === 'new') {
      const discovered = (await api.devices()).devices;
      const available = new Map([...devices,...discovered].map(d => [d.id,d.name || d.id]));
      for (const s of instances) available.delete(s.device_id);
      const select = $('bind-device');
      select.replaceChildren(new Option('选择输出设备',''), ...Array.from(available, ([id,name]) => new Option(name,id)));
      function hint() {
        $('bind-hint').textContent = !available.size ? '没有可分配的输出设备。连接音箱后请重新加载。' :
          devices.some(d => d.enabled && d.id === select.value) ? '绑定后，这只音箱将从主接收器转为独立接收。' :
          '绑定成功后即可设置这只音箱的音量曲线。';
        $('bind').disabled = !select.value;
      }
      hint(); select.onchange = hint;
      if (!available.size) $('retry').hidden = false;
      $('bind-form').onsubmit = async event => {
        event.preventDefault();
        const name = $('room-name');
        const deviceId = select.value;
        name.disabled = select.disabled = $('bind').disabled = true;
        try {
          const result = await api.createSpeaker(name.value.trim(), deviceId);
          const next = new URL(page(result.id, 'curve', deviceId));
          next.searchParams.set('created', '1');
          location.assign(next);
        } catch (error) {
          fail(error);
          name.disabled = select.disabled = false;
          hint();
        }
      };
    }
    if (section === 'curve') {
      if (!room) throw new Error('此实例已移除。');
      $('room-label').textContent = room.name;
      const bound = outputs(roomId);
      if (!bound.length) {
        $('curve-device-row').hidden = true;
        throw new Error('此实例尚未绑定音箱。请先分配输出设备。');
      }
      const select = $('curve-device');
      select.replaceChildren(new Option('选择要调整的音箱',''), ...bound.map(d => new Option(d.name || d.id,d.id)));
      const preferred = url.searchParams.get('device');
      if (preferred && !bound.some(d => d.id === preferred)) {
        $('notice').textContent = '原音箱绑定已变更，请重新选择输出设备。';
      } else {
        select.value = preferred || (bound.length === 1 ? bound[0].id : '');
      }
      if (select.value && url.searchParams.has('created')) {
        $('binding-result').textContent = '已绑定' + select.selectedOptions[0].text + '，可以调节曲线或直接返回播放器。';
        url.searchParams.delete('created');
        history.replaceState(null, '', url);
      }
      editCurves(select,bound);
    }
  } catch (error) { fail(error); }

  function editCurves(select, devices) {
    let device, original, curve, saving = false;
    const chart = FP.createCurveChart($('graph'), { probe: () => Number($('probe').value), onCurveChange(points) {
      curve.points = points; render();
    }});
    function load() {
      if (dirty && !window.confirm('放弃这只音箱尚未保存的曲线？')) {
        select.value = device.id; return;
      }
      dirty = false;
      device = devices.find(d => d.id === select.value);
      $('editor').hidden = !device;
      if (!device) return;
      original = device.curve || {kind:'balanced',points:[]};
      curve = JSON.parse(JSON.stringify(original));
      render();
    }
    function probe() {
      const x = Number($('probe').value);
      const y = FP.curve.outputAt(curve.kind, curve.points, x / 100);
      $('readout').textContent = x + '% → ' + Math.round(y * 100) + '%';
    }
    function render() {
      dirty = JSON.stringify(curve) !== JSON.stringify(original);
      $('save-state').textContent = dirty ? '尚未保存' : '';
      $('save').disabled = saving || !dirty;
      for (const btn of $('modes').children) btn.setAttribute('aria-pressed',btn.value === curve.kind);
      $('custom').hidden = curve.kind !== 'custom';
      chart.setData({...curve, editable:curve.kind === 'custom'});
      if (curve.kind === 'custom') pointRows();
      probe();
    }
    function pointRows() {
      const rows = $('points');
      while (rows.childElementCount > curve.points.length) rows.lastChild.remove();
      curve.points.forEach((point,i) => {
        let row = rows.children[i];
        if (!row) {
          row = node('div','point');
          for (const axis of [0,1]) {
            const field = node('input'); field.type = 'number'; field.step = '1';
            field.setAttribute('aria-label',(i+1)+' 号点'+(axis ? '输出' : '输入')+'百分比');
            field.onchange = () => {
              const v = field.valueAsNumber / 100;
              if (Number.isFinite(v)) {
                const [x,y] = curve.points[i];
                chart.movePoint(i, axis === 0 ? v : x, axis === 1 ? v : y);
              }
              render();
            };
            if (axis) row.append(document.createTextNode('→'));
            row.append(field);
          }
          const del = node('button','','×'); del.setAttribute('aria-label','删除第 '+(i+1)+' 个点');
          del.onclick = () => { curve.points.splice(i,1); render(); };
          row.append(del); rows.append(row);
        }
        row.querySelectorAll('input').forEach((el,axis) => { el.value = Math.round(point[axis]*100); });
      });
    }
    for (const [kind,label] of Object.entries(FP.curve.KIND_LABELS)) {
      const button = node('button','',label); button.value = kind;
      button.onclick = () => {
        if (kind === 'custom' && !curve.points.length) curve.points = FP.curve.neutralPoints();
        curve.kind = kind; render();
      };
      $('modes').append(button);
    }
    $('add').onclick = () => chart.addPoint();
    $('reset').onclick = () => { curve.points = FP.curve.neutralPoints(); render(); };
    $('probe').oninput = () => { probe(); chart.redraw(); };
    $('save').onclick = async () => {
      if (saving) return;
      saving = true;
      const sent = JSON.parse(JSON.stringify(curve));
      select.disabled = true; $('save').disabled = true;
      try {
        await api.saveSpeakerCurve(roomId,{device_id:device.id,curve:sent,original});
        device.curve = original = sent;
        $('notice').textContent = ''; $('retry').hidden = true;
        render();
        if (!dirty) $('save-state').textContent = '已保存至'+(device.name || device.id);
      } catch (error) { fail(error); }
      finally { saving = false; select.disabled = false; $('save').disabled = !dirty; }
    };
    select.onchange = load;
    load();
  }
})();
