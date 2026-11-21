'use strict';

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
