'use strict';

/* The displacement map describes the lens, never the content below it.
 * The browser samples its live backdrop on scroll and artwork changes. */
window.FP = window.FP || {};
const effectsKey = 'fusionplay.advancedEffects';
window.fpGetEffects = () => {
  try { return localStorage.getItem(effectsKey) !== 'off'; }
  catch (_) { return true; }
};
window.fpSetEffects = enabled => {
  try { localStorage.setItem(effectsKey, enabled ? 'on' : 'off'); }
  catch (_) { /* The player still applies this choice for the session. */ }
};
function applySurfacePreference() {
  document.documentElement.classList.toggle('solid-surfaces', !window.fpGetEffects());
}
applySurfacePreference();
window.addEventListener('storage', event => {
  if (event.key === effectsKey || event.key === null) applySurfacePreference();
});
FP.initGlass = function () {
  const art = document.getElementById('scene-art');
  let lastArt = '';
  art.onload = () => { art.hidden = false; };
  art.onerror = () => { art.hidden = true; };

  const ns = 'http://www.w3.org/2000/svg';
  const defs = document.getElementById('glass-filters');
  const maps = new WeakMap();
  let serial = 0;

  function element(tag, attributes) {
    const node = document.createElementNS(ns, tag);
    for (const [key, value] of Object.entries(attributes)) node.setAttribute(key, value);
    return node;
  }

  function updateLens(panel) {
    const w = panel.offsetWidth, h = panel.offsetHeight;
    if (!w || !h) return;
    const radius = Math.min(parseFloat(getComputedStyle(panel).borderTopLeftRadius) || 0, w / 2, h / 2);
    const signature = [w, h, radius].join('/');
    let entry = maps.get(panel);
    if (entry && entry.signature === signature) return;

    // A neutral interior needs no processing. Only generate pixels in the
    // perimeter strips. Cap resolution for large panels and very tall logs.
    const scale = Math.min(1, 800 / w, 800 / h);
    const canvas = document.createElement('canvas');
    canvas.width = Math.max(1, Math.round(w * scale));
    canvas.height = Math.max(1, Math.round(h * scale));
    const ctx = canvas.getContext('2d');
    if (!ctx) return;
    ctx.fillStyle = 'rgb(128,128,0)';
    ctx.fillRect(0, 0, canvas.width, canvas.height);
    const pixels = ctx.getImageData(0, 0, canvas.width, canvas.height);
    const rim = Math.min(10, w / 3, h / 3);
    for (let y = 0; y < canvas.height; y++) {
      const py = (y + .5) * h / canvas.height - h / 2;
      for (let x = 0; x < canvas.width; x++) {
        const px = (x + .5) * w / canvas.width - w / 2;
        const qx = Math.abs(px) - w / 2 + radius;
        const qy = Math.abs(py) - h / 2 + radius;
        if (qx < radius - rim && qy < radius - rim && qx < 0 && qy < 0) {
          // Skip the straight interior of this scanline.
          if (px < 0) x = Math.max(x, canvas.width - x - 2);
          continue;
        }
        const dx = Math.max(qx, 0), dy = Math.max(qy, 0);
        const length = Math.hypot(dx, dy);
        const distance = length + Math.min(Math.max(qx, qy), 0) - radius;
        const bend = Math.max(0, 1 - Math.abs(distance) / rim);
        if (!bend) continue;
        const nx = length ? dx / length : Number(qx >= qy);
        const ny = length ? dy / length : Number(qy > qx);
        const offset = (y * canvas.width + x) * 4;
        pixels.data[offset] = 128 + 126 * bend * nx * Math.sign(px);
        pixels.data[offset + 1] = 128 + 126 * bend * ny * Math.sign(py);
      }
    }
    ctx.putImageData(pixels, 0, 0);
    if (!entry) {
      const id = 'surface-lens-' + ++serial;
      const filter = element('filter', { id, x: '0%', y: '0%', width: '100%', height: '100%', 'color-interpolation-filters': 'sRGB' });
      const map = element('feImage', {width: '100%', height: '100%', preserveAspectRatio: 'none', result: 'normals'});
      filter.append(map, element('feDisplacementMap', {in: 'SourceGraphic', in2: 'normals', scale: 14, xChannelSelector: 'R', yChannelSelector: 'G'}));
      defs.append(filter);
      panel.style.setProperty('--surface-lens', 'url(#' + id + ')');
      entry = { map };
      maps.set(panel, entry);
    }
    entry.map.setAttribute('width', w);
    entry.map.setAttribute('height', h);
    entry.map.setAttribute('href', canvas.toDataURL());
    entry.signature = signature;
  }

  if (window.ResizeObserver && window.IntersectionObserver && CSS.supports('backdrop-filter', 'url(#lens)')) {
    const resize = new ResizeObserver(entries => entries.forEach(e => updateLens(e.target)));
    const visibility = new IntersectionObserver(entries => entries.forEach(e => {
      if (e.isIntersecting) resize.observe(e.target);
      else resize.unobserve(e.target);
    }));
    function observeGlass() {
      visibility.disconnect();
      resize.disconnect();
      if (window.fpGetEffects()) document.querySelectorAll('.card').forEach(p => visibility.observe(p));
    }
    observeGlass();
    window.addEventListener('storage', event => {
      if (event.key === effectsKey || event.key === null) observeGlass();
    });
  }
  return function updateArtwork(status, instance) {
    let url = status && status.current_source && status.media && status.media.cover_url || '';
    if (instance && url.startsWith('artwork/')) {
      url = FP.apiUrl('/api/instances/' + encodeURIComponent(instance) + '/' + url);
    }
    if (url === lastArt) return;
    lastArt = url;
    art.hidden = true;
    if (url) art.src = url;
    else art.removeAttribute('src');
  };
};
