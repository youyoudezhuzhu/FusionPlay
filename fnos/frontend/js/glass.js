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
    const droplet = panel.classList.contains('probe-lens');
    const rim = Math.min(10, w / 3, h / 3);
    for (let y = 0; y < canvas.height; y++) {
      const py = (y + .5) * h / canvas.height - h / 2;
      for (let x = 0; x < canvas.width; x++) {
        const px = (x + .5) * w / canvas.width - w / 2;
        const qx = Math.abs(px) - w / 2 + radius;
        const qy = Math.abs(py) - h / 2 + radius;
        if (!droplet && qx < radius - rim && qy < radius - rim && qx < 0 && qy < 0) {
          // Skip the straight interior of this scanline.
          if (px < 0) x = Math.max(x, canvas.width - x - 2);
          continue;
        }
        const dx = Math.max(qx, 0), dy = Math.max(qy, 0);
        const length = Math.hypot(dx, dy);
        const distance = length + Math.min(Math.max(qx, qy), 0) - radius;
        const bend = Math.max(0, 1 - Math.abs(distance) / rim);
        if (!bend && !droplet) continue;
        const nx = length ? dx / length : Number(qx >= qy);
        const ny = length ? dy / length : Number(qy > qx);
        const offset = (y * canvas.width + x) * 4;
        const radial = droplet ? Math.min(1, Math.hypot(px / (w / 2), py / (h / 2))) : 0;
        const dome = droplet ? .57 * Math.sqrt(1 - radial * radial) : 0;
        pixels.data[offset] = 128 + 126 * bend * nx * Math.sign(px) - 255 * px * dome / 14;
        pixels.data[offset + 1] = 128 + 126 * bend * ny * Math.sign(py) - 255 * py * dome / 14;
      }
    }
    ctx.putImageData(pixels, 0, 0);
    if (!entry) {
      const id = 'surface-lens-' + ++serial;
      const filter = element('filter', { id, x: '0%', y: '0%', width: '100%', height: '100%', 'color-interpolation-filters': 'sRGB' });
      const map = element('feImage', {width: '100%', height: '100%', preserveAspectRatio: 'none', result: 'normals'});
      const displacement = element('feDisplacementMap', {in: 'SourceGraphic', in2: 'normals', result: 'edge', scale: 14, xChannelSelector: 'R', yChannelSelector: 'G'});
      filter.append(map, displacement);
      defs.append(filter);
      panel.style.setProperty('--surface-lens', 'url(#' + id + ')');
      entry = { map, filter };
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
      if (window.fpGetEffects()) document.querySelectorAll('.card,.probe-lens').forEach(p => visibility.observe(p));
    }
    // A small, shared normal map follows the pointer. Keep the second
    // displacement pass attached only while a card is pressed or settling.
    let contactMap;
    function contactNormals() {
      if (contactMap) return contactMap;
      const canvas = document.createElement('canvas');
      const side = canvas.width = canvas.height = 64;
      const ctx = canvas.getContext('2d');
      const pixels = ctx.createImageData(side, side);
      for (let y = 0; y < side; y++) {
        for (let x = 0; x < side; x++) {
          const dx = (x + .5) / side * 2 - 1, dy = (y + .5) / side * 2 - 1;
          const r = Math.min(1, Math.hypot(dx, dy));
          const power = 1 - r * r * (3 - 2 * r);
          const i = (y * side + x) * 4;
          pixels.data[i] = 128 - 127 * dx * power;
          pixels.data[i + 1] = 128 - 127 * dy * power;
          pixels.data[i + 3] = 255;
        }
      }
      ctx.putImageData(pixels, 0, 0);
      return contactMap = canvas.toDataURL();
    }

    const contacts = new Map();
    function move(contact, event) {
      const rect = contact.panel.getBoundingClientRect();
      contact.image.setAttribute('x', event.clientX - rect.left - contact.radius);
      contact.image.setAttribute('y', event.clientY - rect.top - contact.radius);
    }
    document.addEventListener('pointerdown', event => {
      const panel = event.target.closest('.card');
      const entry = maps.get(panel);
      if (!entry || !event.isPrimary || !window.fpGetEffects()) return;
      contacts.get(event.pointerId)?.finish();
      entry.finish?.();
      const radius = Math.min(56, panel.clientHeight / 2, panel.clientWidth / 2);
      const neutral = element('feFlood', {'flood-color': '#808080', result:'neutral'});
      const image = element('feImage', {href:contactNormals(), width:2*radius, height:2*radius, result:'contact'});
      const field = element('feMerge', {result:'field'});
      field.append(element('feMergeNode', {in:'neutral'}), element('feMergeNode', {in:'contact'}));
      const bend = element('feDisplacementMap', {in:'edge', in2:'field', scale:radius*.65, xChannelSelector:'R', yChannelSelector:'G'});
      const nodes = [neutral, image, field, bend];
      const contact = {panel, radius, image, bend, frame:0};
      contact.finish = entry.finish = () => {
        cancelAnimationFrame(contact.frame);
        nodes.forEach(node => node.remove());
        contacts.delete(event.pointerId);
        entry.finish = null;
      };
      contacts.set(event.pointerId, contact);
      move(contact, event);
      entry.filter.append(...nodes);
    });
    document.addEventListener('pointermove', event => {
      const contact = contacts.get(event.pointerId);
      if (contact && !contact.frame) move(contact, event);
    });
    function release(event) {
      const contact = contacts.get(event.pointerId);
      if (!contact || contact.frame) return;
      if (matchMedia('(prefers-reduced-motion:reduce)').matches) return contact.finish();
      const start = performance.now();
      function settle(now) {
        const remaining = Math.max(0, 1 - (now - start) / 150);
        contact.bend.setAttribute('scale', contact.radius * .65 * remaining ** 2);
        if (remaining) contact.frame = requestAnimationFrame(settle);
        else contact.finish();
      }
      contact.frame = requestAnimationFrame(settle);
    }
    for (const type of ['pointerup', 'pointercancel']) window.addEventListener(type, release);
    window.addEventListener('blur', () => contacts.forEach(contact => contact.finish()));
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
