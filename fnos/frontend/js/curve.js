'use strict';
/*
 * Per-device volume-curve chart.
 *
 * The curve maps the sender's logical volume (0–100 %) to this speaker's output
 * amplitude (0–100 %). The interactive preview evaluates the same
 * mapping as the receiver without changing hardware volume.
 */
window.FP = window.FP || {};

(function (FP) {
  const util = FP.util;
  const KIND_LABELS = { linear: '线性', soft: '柔和', balanced: '均衡', custom: '自定义' };

  function smoothstep(x) {
    const t = util.clamp(x, 0, 1);
    return t * t * (3 - 2 * t);
  }

  function localOutput(kind, points, logical) {
    const x = util.clamp(logical, 0, 1);
    switch (kind) {
      case 'linear':
        return x;
      case 'soft':
        return smoothstep(x);
      case 'balanced': {
        if (x <= 0) return 0;
        const db = -60 * Math.pow(1 - x, 2.2);
        return Math.pow(10, db / 20);
      }
      case 'custom': {
        const list = [{ x: 0, y: 0 }]
          .concat((points || []).map(function (point) { return { x: point[0], y: point[1] }; }))
          .concat([{ x: 1, y: 1 }])
          .sort(function (a, b) { return a.x - b.x; });
        for (let i = 0; i < list.length - 1; i += 1) {
          const a = list[i];
          const b = list[i + 1];
          if (x >= a.x && x <= b.x) {
            if (b.x - a.x <= 1e-6) return util.clamp(b.y, 0, 1);
            const t = (x - a.x) / (b.x - a.x);
            return util.clamp(a.y + (b.y - a.y) * t, 0, 1);
          }
        }
        return x;
      }
      default:
        return x;
    }
  }

  function samplesFor(curve) {
    const kind = curve && KIND_LABELS[curve.kind] ? curve.kind : 'balanced';
    const points = curve && Array.isArray(curve.points) ? curve.points : [];
    const samples = [];
    for (let i = 0; i <= 100; i += 1) {
      const logical = i / 100;
      samples.push({ logical: logical, output: localOutput(kind, points, logical) });
    }
    return samples;
  }

  function neutralPoints() {
    return [[0.25, 0.25], [0.5, 0.5], [0.75, 0.75]];
  }

  function normalizePoints(points) {
    return (points || [])
      .filter(function (point) { return Array.isArray(point) && point.length === 2; })
      .map(function (point) {
        return [util.clamp(util.finite(point[0], 0), 0, 1), util.clamp(util.finite(point[1], 0), 0, 1)];
      })
      .sort(function (a, b) { return a[0] - b[0]; });
  }

  FP.curve = {
    samplesFor: samplesFor,
    outputAt: localOutput,
    neutralPoints: neutralPoints,
    normalizePoints: normalizePoints,
    KIND_LABELS: KIND_LABELS
  };

  FP.createCurveChart = function (canvas, options) {
    if (!canvas || !canvas.getContext) return null;
    const opts = options || {};
    const ctx = canvas.getContext('2d');
    if (!ctx) return null;
    const state = {
      kind: 'balanced',
      points: [],
      samples: samplesFor({ kind: 'balanced', points: [] }),
      editable: false,
      selected: -1
    };
    let dragging = false;
    let plot = { left: 0, top: 0, width: 1, height: 1 };
    let resizeObserver = null;

    function cssVar(name) {
      return getComputedStyle(canvas).getPropertyValue(name).trim() || '#888';
    }

    function chartSize() {
      const width = Math.max(60, Math.round(canvas.clientWidth || 320));
      const height = Math.max(60, Math.round(canvas.clientHeight || 190));
      return { width: width, height: height };
    }

    function resize() {
      const size = chartSize();
      const dpr = window.devicePixelRatio || 1;
      const pixelWidth = Math.round(size.width * dpr);
      const pixelHeight = Math.round(size.height * dpr);
      if (canvas.width !== pixelWidth || canvas.height !== pixelHeight) {
        canvas.width = pixelWidth;
        canvas.height = pixelHeight;
      }
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
      draw(size.width, size.height);
    }

    function draw(width, height) {
      if (!width || !height) return;
      // Resolve system colours on the element. A bare CanvasText keyword
      // given to a 2D context can resolve against the wrong colour scheme.
      const system = forcedColors.matches ? getComputedStyle(canvas) : null;
      const accent = system ? system.color : cssVar('--accent');
      const border = system ? system.borderTopColor : cssVar('--border');
      const muted = system ? system.color : cssVar('--muted');
      const card = system ? system.backgroundColor : cssVar('--card');
      const fontFamily = cssVar('--font') || 'sans-serif';
      const compact = width < 320;
      const pad = {
        left: compact ? 30 : 36,
        right: 12,
        top: 10,
        bottom: 21
      };
      const plotWidth = Math.max(10, width - pad.left - pad.right);
      const plotHeight = Math.max(10, height - pad.top - pad.bottom);
      plot = { left: pad.left, top: pad.top, width: plotWidth, height: plotHeight };

      ctx.clearRect(0, 0, width, height);

      // Grid + tick labels.
      ctx.lineWidth = 1;
      ctx.strokeStyle = border;
      ctx.fillStyle = muted;
      ctx.font = '10px ' + fontFamily;
      ctx.textBaseline = 'middle';
      for (let i = 0; i <= 4; i += 1) {
        const ratio = i / 4;
        const gx = pad.left + plotWidth * ratio;
        const gy = pad.top + plotHeight * ratio;
        ctx.beginPath();
        ctx.moveTo(gx, pad.top);
        ctx.lineTo(gx, pad.top + plotHeight);
        ctx.stroke();
        ctx.beginPath();
        ctx.moveTo(pad.left, gy);
        ctx.lineTo(pad.left + plotWidth, gy);
        ctx.stroke();
      }

      ctx.textAlign = 'center';
      for (let i = 0; i <= 4; i += 1) {
        const ratio = i / 4;
        const gx = pad.left + plotWidth * ratio;
        ctx.fillText(Math.round(ratio * 100) + '%', gx, pad.top + plotHeight + 11);
      }
      ctx.textAlign = 'right';
      for (let i = 0; i <= 4; i += 1) {
        const ratio = i / 4;
        const gy = pad.top + plotHeight * (1 - ratio);
        ctx.fillText(Math.round(ratio * 100) + '%', pad.left - 6, gy);
      }

      // Curve.
      const samples = state.samples && state.samples.length
        ? state.samples
        : samplesFor({ kind: state.kind, points: state.points });
      ctx.beginPath();
      samples.forEach(function (sample, index) {
        const x = pad.left + util.clamp(Number(sample.logical), 0, 1) * plotWidth;
        const y = pad.top + (1 - util.clamp(Number(sample.output), 0, 1)) * plotHeight;
        if (index === 0) ctx.moveTo(x, y);
        else ctx.lineTo(x, y);
      });
      ctx.strokeStyle = accent;
      ctx.lineWidth = 2;
      ctx.lineJoin = 'round';
      ctx.lineCap = 'round';
      ctx.stroke();

      ctx.lineTo(pad.left + plotWidth, pad.top + plotHeight);
      ctx.lineTo(pad.left, pad.top + plotHeight);
      ctx.closePath();
      const fill = ctx.createLinearGradient(0, pad.top, 0, pad.top + plotHeight);
      fill.addColorStop(0, 'rgba(242,242,245,.24)');
      fill.addColorStop(1, 'rgba(242,242,245,0)');
      ctx.fillStyle = fill;
      ctx.fill();
      if (opts.probe) {
        const sample = samples[opts.probe()];
        if (sample) {
          const x = plot.left + sample.logical * plot.width;
          const y = plot.top + (1 - sample.output) * plot.height;
          ctx.beginPath(); ctx.moveTo(x, y); ctx.lineTo(x, plot.top + plot.height);
          ctx.strokeStyle = muted; ctx.lineWidth = 1; ctx.setLineDash([3,4]); ctx.stroke(); ctx.setLineDash([]);
          ctx.beginPath(); ctx.arc(x, y, 4.5, 0, Math.PI * 2); ctx.fillStyle = accent; ctx.fill();
        }
      }

      // Fixed endpoints + editable control points for custom curves.
      if (state.kind === 'custom') {
        ctx.fillStyle = muted;
        [[0, 0], [1, 1]].forEach(function (point) {
          ctx.beginPath();
          ctx.arc(pad.left + point[0] * plotWidth, pad.top + (1 - point[1]) * plotHeight, 3, 0, Math.PI * 2);
          ctx.fill();
        });
        state.points.forEach(function (point, index) {
          const cx = pad.left + point[0] * plotWidth;
          const cy = pad.top + (1 - point[1]) * plotHeight;
          const isSelected = index === state.selected;
          ctx.beginPath();
          ctx.arc(cx, cy, isSelected ? 5.5 : 4.4, 0, Math.PI * 2);
          ctx.fillStyle = isSelected ? accent : card;
          ctx.fill();
          ctx.strokeStyle = accent;
          ctx.lineWidth = 1.8;
          ctx.stroke();
        });
      }
    }

    function pointFromEvent(event) {
      const rect = canvas.getBoundingClientRect();
      const x = (event.clientX - rect.left - plot.left) / Math.max(1, plot.width);
      const y = 1 - (event.clientY - rect.top - plot.top) / Math.max(1, plot.height);
      return { x: util.clamp(x, 0, 1), y: util.clamp(y, 0, 1) };
    }

    function findPoint(event) {
      if (state.kind !== 'custom' || !state.editable) return -1;
      const rect = canvas.getBoundingClientRect();
      const px = event.clientX - rect.left;
      const py = event.clientY - rect.top;
      let best = -1;
      let bestDistance = 15;
      state.points.forEach(function (point, index) {
        const cx = plot.left + point[0] * plot.width;
        const cy = plot.top + (1 - point[1]) * plot.height;
        const distance = Math.sqrt(Math.pow(px - cx, 2) + Math.pow(py - cy, 2));
        if (distance <= bestDistance) {
          bestDistance = distance;
          best = index;
        }
      });
      return best;
    }

    function emitChange() {
      state.samples = samplesFor({ kind: 'custom', points: state.points });
      resize();
      if (typeof opts.onCurveChange === 'function') {
        opts.onCurveChange(state.points.map(function (point) { return [point[0], point[1]]; }));
      }
    }

    function movePoint(index, x, y) {
      if (index < 0 || index >= state.points.length) return;
      const prev = index > 0 ? state.points[index - 1] : [0, 0];
      const next = index < state.points.length - 1 ? state.points[index + 1] : [1, 1];
      const epsilon = 0.004;
      let low = prev[0] + epsilon;
      let high = next[0] - epsilon;
      if (low > high) {
        low = high = (prev[0] + next[0]) / 2;
      }
      const nx = util.round(util.clamp(x, low, high), 4);
      let yLow = prev[1];
      let yHigh = next[1];
      if (yLow > yHigh) {
        const swap = yLow;
        yLow = yHigh;
        yHigh = swap;
      }
      const ny = util.round(util.clamp(y, yLow, yHigh), 4);
      if (state.points[index][0] === nx && state.points[index][1] === ny) return;
      state.points[index] = [nx, ny];
      emitChange();
    }

    function onPointerDown(event) {
      if (!state.editable || state.kind !== 'custom') return;
      const index = findPoint(event);
      if (index < 0) {
        state.selected = -1;
        resize();
        return;
      }
      state.selected = index;
      dragging = true;
      if (canvas.setPointerCapture) {
        try { canvas.setPointerCapture(event.pointerId); } catch (error) { /* ignore */ }
      }
      event.preventDefault();
      resize();
      canvas.focus();
    }

    function onPointerMove(event) {
      if (!dragging || state.selected < 0) return;
      event.preventDefault();
      const point = pointFromEvent(event);
      movePoint(state.selected, point.x, point.y);
    }

    function endDrag(event) {
      if (!dragging) return;
      dragging = false;
      if (event && canvas.releasePointerCapture && event.pointerId != null) {
        try { canvas.releasePointerCapture(event.pointerId); } catch (error) { /* ignore */ }
      }
    }

    function onKeyDown(event) {
      if (!state.editable || state.kind !== 'custom') return;
      const step = event.shiftKey ? 0.05 : 0.01;
      if (event.key === 'Delete' || event.key === 'Backspace') {
        if (removeSelected()) {
          event.preventDefault();
        }
        return;
      }
      let dx = 0;
      let dy = 0;
      if (event.key === 'ArrowLeft') dx = -step;
      else if (event.key === 'ArrowRight') dx = step;
      else if (event.key === 'ArrowUp') dy = step;
      else if (event.key === 'ArrowDown') dy = -step;
      else return;
      if (state.selected < 0) {
        state.selected = 0;
        resize();
        return;
      }
      const point = state.points[state.selected];
      if (!point) return;
      event.preventDefault();
      movePoint(state.selected, point[0] + dx, point[1] + dy);
    }

    function addPoint() {
      if (state.kind !== 'custom') return false;
      const list = [{ x: 0, y: 0 }];
      state.points.forEach(function (point) { list.push({ x: point[0], y: point[1] }); });
      list.push({ x: 1, y: 1 });
      let bestIndex = -1;
      let bestGap = 0.02;
      for (let i = 0; i < list.length - 1; i += 1) {
        const gap = list[i + 1].x - list[i].x;
        if (gap > bestGap) {
          bestGap = gap;
          bestIndex = i;
        }
      }
      if (bestIndex < 0) return false;
      const a = list[bestIndex];
      const b = list[bestIndex + 1];
      const point = [
        util.round((a.x + b.x) / 2, 4),
        util.round((a.y + b.y) / 2, 4)
      ];
      state.points.push(point);
      state.points.sort(function (p1, p2) { return p1[0] - p2[0]; });
      state.selected = state.points.indexOf(point);
      emitChange();
      return true;
    }

    function removeSelected() {
      if (state.kind !== 'custom' || state.selected < 0 || state.selected >= state.points.length) {
        return false;
      }
      state.points.splice(state.selected, 1);
      if (state.selected >= state.points.length) state.selected = state.points.length - 1;
      emitChange();
      return true;
    }

    function resetPoints() {
      if (state.kind !== 'custom') return;
      state.points = neutralPoints();
      state.selected = -1;
      emitChange();
    }

    function setData(data) {
      const next = data || {};
      state.kind = KIND_LABELS[next.kind] ? next.kind : 'balanced';
      state.points = normalizePoints(next.points);
      state.editable = !!next.editable && state.kind === 'custom';
      if (Array.isArray(next.samples) && next.samples.length) {
        state.samples = next.samples;
      } else {
        state.samples = samplesFor({ kind: state.kind, points: state.points });
      }
      if (state.selected >= state.points.length) state.selected = -1;
      resize();
    }

    canvas.addEventListener('pointerdown', onPointerDown);
    canvas.addEventListener('pointermove', onPointerMove);
    canvas.addEventListener('pointerup', endDrag);
    canvas.addEventListener('pointercancel', endDrag);
    canvas.addEventListener('keydown', onKeyDown);
    canvas.addEventListener('blur', function () { dragging = false; });

    if (typeof ResizeObserver !== 'undefined') {
      resizeObserver = new ResizeObserver(function () { resize(); });
      resizeObserver.observe(canvas);
    }
    const forcedColors = window.matchMedia('(forced-colors:active)');
    const palettes = [forcedColors, window.matchMedia('(prefers-color-scheme:dark)')];
    for (const palette of palettes) palette.addEventListener('change', resize);
    window.addEventListener('resize', resize);
    resize();

    return {
      setData: setData,
      redraw: resize,
      movePoint: movePoint,
      addPoint: addPoint,
      removeSelected: removeSelected,
      resetPoints: resetPoints,
      getPoints: function () {
        return state.points.map(function (point) { return [point[0], point[1]]; });
      },
      getKind: function () { return state.kind; },
      getSelected: function () { return state.selected; },
      destroy: function () {
        if (resizeObserver) resizeObserver.disconnect();
        for (const palette of palettes) palette.removeEventListener('change', resize);
        window.removeEventListener('resize', resize);
        canvas.removeEventListener('pointerdown', onPointerDown);
        canvas.removeEventListener('pointermove', onPointerMove);
        canvas.removeEventListener('pointerup', endDrag);
        canvas.removeEventListener('pointercancel', endDrag);
        canvas.removeEventListener('keydown', onKeyDown);
      }
    };
  };
})(window.FP);
