import { TrimApp } from './trim-web-app.js';

const scene = window.FP.initGlass();
const sdk = new TrimApp();
const byId = id => document.getElementById(id);
const pending = 'fusionplay.update.folder';
const gateway = '/app/fusionplay-fnos/';
let room = new URL(location.href).searchParams.get('instance');
let selected = '';
let release;
let busy = false;

function render() {
  byId('back').href = 'player/index.html' + (room ? '?instance=' + encodeURIComponent(room) : '');
  byId('pick').hidden = !release;
  byId('save').hidden = !release || !selected;
  byId('directory').hidden = !selected;
  byId('directory').textContent = selected;
  for (const id of ['check','pick','save']) byId(id).disabled = busy;
}

function showError(error) {
  window.fpRecordBrowserError?.('更新失败：' + (error.message || String(error)));
  byId('message').textContent = error.message || String(error);
  byId('check').hidden = false;
  byId('progress').hidden = true;
}

function callback() {
  const result = sdk.parseAppAuthCallback(location.href);
  if (!result.status) return;
  const request = JSON.parse(sessionStorage.getItem(pending) || 'null');
  sessionStorage.removeItem(pending);
  history.replaceState(null, '', location.pathname);
  if (!request || !result.state || result.state !== request.state ||
      (result.method && result.method !== 'pickUserFile') || (result.appName && result.appName !== 'fusionplay-fnos')) {
    throw new Error('无法确认本次目录授权，请重新选择');
  }
  room = request.room;
  if (room) history.replaceState(null, '', '?instance=' + encodeURIComponent(room));
  if (result.status === 'success') {
    selected = result.path?.[0] || '';
  } else if (result.status !== 'cancel') {
    throw new Error('飞牛目录授权失败，请检查账号权限并重试');
  }
}

async function check() {
  busy = true;
  release = null;
  byId('progress').hidden = true;
  byId('check').hidden = true;
  byId('message').textContent = '正在检查飞牛版本…';
  render();
  try {
    const response = await fetch('api/update/check', {cache:'no-store'});
    const info = await response.json();
    if (!response.ok) throw new Error(info.error || '无法读取发布信息');
    if (info.status === 'available') {
      release = info;
      byId('release').textContent = '发现新版本 ' + info.versionName;
      byId('message').textContent = info.filename + ' · ' + (info.apkSize / 1048576).toFixed(1) + ' MB';
    } else if (info.status === 'up_to_date') {
      byId('release').textContent = '已是最新版本';
      byId('message').textContent = '当前架构暂无更高的正式版本。';
    } else throw new Error('无法识别发布信息');
  } catch (err) { showError(err); }
  finally { busy = false; render(); }
}

byId('pick').onclick = async () => {
  busy = true;
  render();
  try {
    if (!location.pathname.startsWith(gateway)) throw new Error('请从飞牛桌面打开 FusionPlay，以使用 NAS 文件夹授权。');
    byId('message').textContent = '正在打开飞牛目录选择器…';
    await sdk.ready();
    if (sdk.isStandaloneWeb) {
      const state = Array.from(crypto.getRandomValues(new Uint32Array(4)), v => v.toString(16).padStart(8, '0')).join('');
      sessionStorage.setItem(pending, JSON.stringify({state, room}));
      await sdk.openAppAuth('pickUserFile', {
        appName:'fusionplay-fnos', directory:true, state, redirectUri:gateway + 'update.html',
      }, {target:'_self'});
    } else {
      const result = await sdk.pickUserFile({directory:true, title:'选择 FPK 保存目录'});
      if (result && result.code !== 0) throw new Error(result.msg || '未获得目录授权');
      if (result?.data?.length) {
        selected = result.data[0];
        byId('message').textContent = '请确认安装包的保存位置。';
      }
      else byId('message').textContent = '尚未选择目录。若未弹出选择器，请检查飞牛系统（1.2.0401 起）和客户端（1.34.0 起）。';
    }
  } catch (err) { showError(err); }
  finally { busy = false; render(); }
};

async function receive(response) {
  if (!response.ok) throw new Error((await response.text()) || '下载失败');
  const reader = response.body.getReader();
  const decoder = new TextDecoder();
  let buffer = '';
  let path;
  for (;;) {
    const {done, value} = await reader.read();
    buffer += decoder.decode(value, {stream:!done});
    const lines = buffer.split('\n');
    buffer = lines.pop();
    for (const line of lines) {
      const event = JSON.parse(line);
      if (event.error) throw new Error(event.error);
      if (event.path) path = event.path;
      if (event.total > 0) {
        const progress = byId('progress');
        progress.max = event.total;
        progress.value = event.bytes;
        byId('message').textContent = event.bytes === event.total ? '正在校验安装包…' :
          '正在下载 ' + Math.floor(event.bytes * 100 / event.total) + '%';
      }
    }
    if (done) break;
  }
  if (!path) throw new Error('下载连接已中断，请重新下载');
  return path;
}

byId('save').onclick = async () => {
  busy = true;
  render();
  byId('progress').removeAttribute('value');
  byId('progress').hidden = false;
  byId('message').textContent = '正在核对目录权限并连接下载源…';
  try {
    const path = await receive(await fetch('api/update/save', {
      method:'POST', headers:{'Content-Type':'application/json','X-FusionPlay-Update':'1'},
      body:JSON.stringify({tag:release.releaseKey,directory:selected}),
    }));
    selected = '';
    release = null;
    byId('release').textContent = '安装包已保存';
    byId('message').textContent = path + '\n\n请在飞牛「应用中心 → 手动安装」中选择这个 FPK。';
  } catch (err) { showError(err); }
  finally { busy = false; render(); }
};

byId('check').onclick = check;
let refreshingArtwork = false;
async function refreshArtwork() {
  if (refreshingArtwork || document.hidden) return;
  refreshingArtwork = true;
  try { scene(await FP.api.status(room), room); }
  catch (_) { scene(null, room); }
  finally { refreshingArtwork = false; }
}

try {
  callback();
  refreshArtwork();
  document.addEventListener('visibilitychange', refreshArtwork);
  setInterval(refreshArtwork, 4000);
  await check();
}
catch (err) { showError(err); render(); }
