// TxPlayer site — sticky nav, scrollspy, reveal, ARIA tabs, screenshot viewer, copy buttons, acoustic waveform dot field, lightbox
(function () {
  'use strict';

  function slice(list) { return Array.prototype.slice.call(list); }

  // ---- sticky nav ----
  var nav = document.getElementById('nav');
  function onScroll() {
    if (nav) nav.classList.toggle('scrolled', window.scrollY > 24);
  }
  window.addEventListener('scroll', onScroll, { passive: true });
  onScroll();

  // ---- scrollspy ----
  var links = slice(document.querySelectorAll('.nav-links a'))
    .filter(function (a) { return (a.getAttribute('href') || '').charAt(0) === '#'; });
  var sections = links
    .map(function (a) { return document.querySelector(a.getAttribute('href')); })
    .filter(Boolean);

  if ('IntersectionObserver' in window) {
    var spy = new IntersectionObserver(function (entries) {
      entries.forEach(function (e) {
        if (!e.isIntersecting) return;
        links.forEach(function (a) {
          a.classList.toggle('active', a.getAttribute('href') === '#' + e.target.id);
        });
      });
    }, { rootMargin: '-40% 0px -55% 0px' });
    sections.forEach(function (s) { spy.observe(s); });
  }

  // ---- reveal on scroll ----
  var revealEls = document.querySelectorAll('.reveal');
  if ('IntersectionObserver' in window) {
    var ro = new IntersectionObserver(function (entries) {
      entries.forEach(function (e) {
        if (e.isIntersecting) {
          e.target.classList.add('visible');
          ro.unobserve(e.target);
        }
      });
    }, { threshold: 0.1 });
    revealEls.forEach(function (el) { ro.observe(el); });
  } else {
    revealEls.forEach(function (el) { el.classList.add('visible'); });
  }

  // ---- ARIA tabs ----
  slice(document.querySelectorAll('.tabs')).forEach(function (root) {
    var tabs = slice(root.querySelectorAll('.tab-bar [role="tab"]'));
    if (!tabs.length) return;
    var panes = tabs.map(function (t) {
      return document.getElementById(t.getAttribute('aria-controls'));
    });
    function select(i, focus) {
      tabs.forEach(function (t, j) {
        var on = j === i;
        t.setAttribute('aria-selected', on ? 'true' : 'false');
        t.tabIndex = on ? 0 : -1;
        if (panes[j]) {
          panes[j].hidden = !on;
          var vid = panes[j].querySelector('video');
          if (vid) {
            if (on) { vid.play().catch(function () {}); } else { vid.pause(); }
          }
        }
      });
      if (focus) tabs[i].focus();
    }
    tabs.forEach(function (t, i) {
      t.addEventListener('click', function () { select(i); });
      t.addEventListener('keydown', function (e) {
        var j = null;
        if (e.key === 'ArrowRight') j = (i + 1) % tabs.length;
        else if (e.key === 'ArrowLeft') j = (i + tabs.length - 1) % tabs.length;
        else if (e.key === 'Home') j = 0;
        else if (e.key === 'End') j = tabs.length - 1;
        if (j !== null) { e.preventDefault(); select(j, true); }
      });
    });
  });

  var jumpVideo = document.getElementById('jumpVideoTour');
  if (jumpVideo) {
    jumpVideo.addEventListener('click', function (e) {
      e.stopPropagation();
      var vtab = document.getElementById('sh-video');
      if (vtab) vtab.click();
    });
  }

  // ---- screenshot viewer ----
  slice(document.querySelectorAll('.shot-pane')).forEach(function (pane) {
    var figs = slice(pane.querySelectorAll('.shot-item'));
    if (!figs.length) return;
    var prev = pane.querySelector('.sv-prev');
    var next = pane.querySelector('.sv-next');
    var count = pane.querySelector('.sv-count');
    var idx = 0;
    function show(i) {
      idx = (i + figs.length) % figs.length;
      figs.forEach(function (f, j) { f.hidden = j !== idx; });
      if (count) count.textContent = (idx + 1) + ' / ' + figs.length;
    }
    if (figs.length < 2) {
      if (prev) prev.hidden = true;
      if (next) next.hidden = true;
      if (count) count.hidden = true;
    } else {
      prev.addEventListener('click', function () { show(idx - 1); });
      next.addEventListener('click', function () { show(idx + 1); });
      pane.addEventListener('keydown', function (e) {
        if (e.key === 'ArrowLeft') { e.preventDefault(); show(idx - 1); }
        else if (e.key === 'ArrowRight') { e.preventDefault(); show(idx + 1); }
      });
    }
    figs.forEach(function (f) {
      f.addEventListener('keydown', function (e) {
        if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); f.click(); }
      });
    });
    show(0);
  });

  // ---- terminal copy buttons ----
  slice(document.querySelectorAll('.term')).forEach(function (term) {
    var btn = term.querySelector('.term-copy');
    var code = term.querySelector('code');
    if (!btn || !code) return;
    var timer = 0;
    function feedback() {
      btn.classList.add('done');
      clearTimeout(timer);
      timer = setTimeout(function () { btn.classList.remove('done'); }, 1600);
    }
    function fallback() {
      var sel = window.getSelection();
      var range = document.createRange();
      range.selectNodeContents(code);
      sel.removeAllRanges();
      sel.addRange(range);
      feedback();
    }
    btn.addEventListener('click', function () {
      var text = code.textContent;
      if (navigator.clipboard && navigator.clipboard.writeText) {
        navigator.clipboard.writeText(text).then(feedback, fallback);
      } else {
        fallback();
      }
    });
  });

  // ---- ambient dot-particle field: drifting musical Treble/Bass clefs, beamed notes & piano keys ----
  (function () {
    var canvas = document.getElementById('bgDots');
    if (!canvas || !canvas.getContext) return;
    var ctx = canvas.getContext('2d');
    var reduced = window.matchMedia && window.matchMedia('(prefers-reduced-motion: reduce)').matches;
    var dpr = Math.min(window.devicePixelRatio || 1, 2);
    var W = 0, H = 0, raf = 0;
    var glyphs = [];
    var mouse = { x: -1e4, y: -1e4 };

    var GLYPH_DEFS = [
      { kind: 'treble', fx: 0.84, fy: 0.26, size: 420, tint: '45, 212, 191', alpha: 0.34, amp: 24, period: 62000, phase: 0.0 },
      { kind: 'notes',  fx: 0.10, fy: 0.55, size: 285, tint: '94, 234, 212', alpha: 0.22, amp: 28, period: 78000, phase: 2.1 },
      { kind: 'piano',  fx: 0.72, fy: 0.84, size: 340, tint: '45, 212, 191', alpha: 0.18, amp: 26, period: 74000, phase: 4.0 },
      { kind: 'bass',   fx: 0.30, fy: 0.35, size: 220, tint: '94, 234, 212', alpha: 0.16, amp: 20, period: 88000, phase: 1.4 }
    ];
    var GLYPH_DEFS_SM = [
      { kind: 'treble', fx: 0.84, fy: 0.20, size: 250, tint: '45, 212, 191', alpha: 0.28, amp: 16, period: 62000, phase: 0.0 },
      { kind: 'notes',  fx: 0.14, fy: 0.74, size: 195, tint: '94, 234, 212', alpha: 0.18, amp: 18, period: 78000, phase: 2.1 }
    ];

    function sampleMusicalGlyph(kind, size) {
      var s = 2;
      var off = document.createElement('canvas');
      off.width = off.height = Math.round(size * s);
      var c = off.getContext('2d');
      c.scale(s, s);
      c.strokeStyle = '#fff';
      c.fillStyle = '#fff';
      c.lineCap = 'round';
      c.lineJoin = 'round';
      var cx = size / 2, cy = size / 2;

      if (kind === 'treble') {
        // Classic engraved Treble (G) Clef — bold vector Bezier silhouette
        var u = size * 0.0085;
        c.lineWidth = Math.max(9, size * 0.044);
        // Main spine & top tear-drop loop
        c.beginPath();
        c.moveTo(cx + 3 * u, cy - 42 * u);
        c.bezierCurveTo(cx + 22 * u, cy - 30 * u, cx + 12 * u, cy - 8 * u, cx - 12 * u, cy + 4 * u);
        c.bezierCurveTo(cx - 32 * u, cy + 14 * u, cx - 28 * u, cy + 34 * u, cx - 2 * u, cy + 34 * u);
        c.bezierCurveTo(cx + 24 * u, cy + 34 * u, cx + 28 * u, cy + 10 * u, cx + 8 * u, cy + 4 * u);
        c.bezierCurveTo(cx - 8 * u, cy - 1 * u, cx - 14 * u, cy + 14 * u, cx - 2 * u, cy + 20 * u);
        c.stroke();

        // Upright spine sweeping down to the bottom scroll dot
        c.lineWidth = Math.max(8, size * 0.036);
        c.beginPath();
        c.moveTo(cx + 3 * u, cy - 42 * u);
        c.bezierCurveTo(cx - 10 * u, cy - 32 * u, cx - 6 * u, cy - 10 * u, cx + 2 * u, cy + 42 * u);
        c.bezierCurveTo(cx + 4 * u, cy + 50 * u, cx - 10 * u, cy + 52 * u, cx - 14 * u, cy + 44 * u);
        c.stroke();
        c.beginPath();
        c.arc(cx - 14 * u, cy + 43 * u, 6.5 * u, 0, Math.PI * 2);
        c.fill();
      } else if (kind === 'notes') {
        // Beamed eighth-note pair (♫) with tilted oval noteheads, stems & thick top beam
        var u = size * 0.009;
        // Left & right tilted noteheads
        [
          { x: cx - 22 * u, y: cy + 22 * u },
          { x: cx + 20 * u, y: cy + 12 * u }
        ].forEach(function (n) {
          c.save();
          c.translate(n.x, n.y);
          c.rotate(-0.34);
          c.beginPath();
          c.ellipse(0, 0, 14 * u, 10 * u, 0, 0, Math.PI * 2);
          c.fill();
          c.restore();
        });
        // Upright stems
        c.lineWidth = Math.max(7, size * 0.036);
        c.beginPath();
        c.moveTo(cx - 11 * u, cy + 19 * u);
        c.lineTo(cx - 11 * u, cy - 26 * u);
        c.moveTo(cx + 31 * u, cy + 9 * u);
        c.lineTo(cx + 31 * u, cy - 36 * u);
        c.stroke();
        // Double angled musical beams
        c.lineWidth = Math.max(10, size * 0.056);
        c.beginPath();
        c.moveTo(cx - 11 * u, cy - 26 * u);
        c.lineTo(cx + 31 * u, cy - 36 * u);
        c.stroke();
        c.lineWidth = Math.max(7, size * 0.036);
        c.beginPath();
        c.moveTo(cx - 11 * u, cy - 13 * u);
        c.lineTo(cx + 31 * u, cy - 23 * u);
        c.stroke();
      } else if (kind === 'bass') {
        // Classic Bass (F) Clef with twin F-line dots
        var u = size * 0.009;
        c.beginPath();
        c.arc(cx - 22 * u, cy - 10 * u, 8.5 * u, 0, Math.PI * 2);
        c.fill();
        c.lineWidth = Math.max(9, size * 0.048);
        c.beginPath();
        c.moveTo(cx - 22 * u, cy - 10 * u);
        c.bezierCurveTo(cx - 22 * u, cy - 36 * u, cx + 22 * u, cy - 36 * u, cx + 18 * u, cy - 4 * u);
        c.bezierCurveTo(cx + 14 * u, cy + 18 * u, cx - 6 * u, cy + 32 * u, cx - 26 * u, cy + 38 * u);
        c.stroke();
        c.beginPath();
        c.arc(cx + 30 * u, cy - 16 * u, 5.5 * u, 0, Math.PI * 2);
        c.arc(cx + 30 * u, cy + 2 * u, 5.5 * u, 0, Math.PI * 2);
        c.fill();
      } else {
        // 1-octave Piano Keybed silhouette + 3 subtle staff lines above
        var u = size * 0.009;
        var kw = 11 * u, kh = 34 * u;
        var startX = cx - 3.5 * kw;
        var topY = cy - 4 * u;
        c.lineWidth = Math.max(4, size * 0.018);
        // Staff lines floating above the keyboard
        [-28, -18, -8].forEach(function (dy) {
          c.beginPath();
          c.moveTo(startX - 4 * u, topY + dy * u);
          c.lineTo(startX + 7 * kw + 4 * u, topY + dy * u);
          c.stroke();
        });
        // White key outline + dividers
        c.strokeRect(startX, topY, 7 * kw, kh);
        for (var k = 1; k < 7; k++) {
          c.beginPath();
          c.moveTo(startX + k * kw, topY);
          c.lineTo(startX + k * kw, topY + kh);
          c.stroke();
        }
        // Black keys (C#, D#, F#, G#, A#)
        [1, 2, 4, 5, 6].forEach(function (pos) {
          c.fillRect(startX + pos * kw - kw * 0.34, topY, kw * 0.68, kh * 0.60);
        });
      }

      var data = c.getImageData(0, 0, off.width, off.height).data;
      var step = 6, pts = [];
      for (var attempt = 0; attempt < 6; attempt++) {
        pts = [];
        for (var y = 0; y < off.height; y += step * s) {
          for (var x = 0; x < off.width; x += step * s) {
            if (data[(y * off.width + x) * 4 + 3] > 90) {
              pts.push({ x: x / s - cx, y: y / s - cy });
            }
          }
        }
        if (pts.length <= 720) break;
        step++;
      }
      return pts;
    }

    function layout() {
      W = window.innerWidth; H = window.innerHeight;
      dpr = Math.min(window.devicePixelRatio || 1, 2);
      canvas.width = Math.round(W * dpr);
      canvas.height = Math.round(H * dpr);
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
      var defs = W < 700 ? GLYPH_DEFS_SM : GLYPH_DEFS;
      var scale = Math.max(0.7, Math.min(1.15, W / 1600));
      var old = glyphs;
      glyphs = defs.map(function (d, gi) {
        var size = d.size * scale;
        var pts = sampleMusicalGlyph(d.kind, size);
        var prevGlyph = old[gi];
        return {
          def: d, size: size,
          dots: pts.map(function (p, i) {
            var prev = prevGlyph && prevGlyph.dots[i];
            return { lx: p.x, ly: p.y, x: prev ? prev.x : 0, y: prev ? prev.y : 0, vx: 0, vy: 0, init: !!prev };
          })
        };
      });
      draw(performance.now());
    }

    function glyphOrigin(g, t) {
      var d = g.def;
      return {
        x: d.fx * W + Math.sin(t / d.period * Math.PI * 2 + d.phase) * d.amp,
        y: d.fy * H + Math.cos(t / d.period * Math.PI * 2 + d.phase * 1.7) * d.amp * 0.7
      };
    }

    function draw(t) {
      ctx.clearRect(0, 0, W, H);
      for (var gi = 0; gi < glyphs.length; gi++) {
        var g = glyphs[gi];
        var o = glyphOrigin(g, t);
        ctx.fillStyle = 'rgba(' + g.def.tint + ', ' + g.def.alpha + ')';
        for (var i = 0; i < g.dots.length; i++) {
          var p = g.dots[i];
          if (!p.init) { p.x = o.x + p.lx; p.y = o.y + p.ly; p.init = true; }
          ctx.fillRect(p.x, p.y, 2.4, 2.4);
        }
      }
    }

    function tick(now) {
      raf = 0;
      var R = 110, R2 = R * R;
      for (var gi = 0; gi < glyphs.length; gi++) {
        var g = glyphs[gi];
        var o = glyphOrigin(g, now);
        for (var i = 0; i < g.dots.length; i++) {
          var p = g.dots[i];
          var hx = o.x + p.lx, hy = o.y + p.ly;
          var dx = p.x - mouse.x, dy = p.y - mouse.y;
          var d2 = dx * dx + dy * dy;
          if (d2 < R2 && d2 > 0.01) {
            var d = Math.sqrt(d2);
            var f = (1 - d / R) * 1.5;
            p.vx += (dx / d) * f;
            p.vy += (dy / d) * f;
          }
          p.vx = (p.vx + (hx - p.x) * 0.016) * 0.90;
          p.vy = (p.vy + (hy - p.y) * 0.016) * 0.90;
          p.x += p.vx; p.y += p.vy;
        }
      }
      draw(now);
      raf = requestAnimationFrame(tick);
    }

    function start() { if (!raf && !reduced && !document.hidden) raf = requestAnimationFrame(tick); }
    function stop() { if (raf) { cancelAnimationFrame(raf); raf = 0; } }

    window.addEventListener('pointermove', function (e) { mouse.x = e.clientX; mouse.y = e.clientY; }, { passive: true });
    document.addEventListener('pointerleave', function () { mouse.x = -1e4; mouse.y = -1e4; });
    document.addEventListener('visibilitychange', function () { if (document.hidden) stop(); else start(); });
    var rt = 0;
    window.addEventListener('resize', function () {
      clearTimeout(rt);
      rt = setTimeout(function () { layout(); start(); }, 150);
    });
    layout();
    start();
  })();

  // ---- lightbox ----
  var lb = document.getElementById('lightbox');
  var lbImg = document.getElementById('lbImg');
  var lbPrev = document.getElementById('lbPrev');
  var lbNext = document.getElementById('lbNext');
  var lbList = [];
  var current = 0;

  function showCurrent() { lbImg.src = lbList[current].getAttribute('data-lightbox'); }
  function open(list, i) {
    lbList = list;
    current = (i + list.length) % list.length;
    showCurrent();
    lb.hidden = false;
    document.body.style.overflow = 'hidden';
    var multi = list.length > 1;
    lbPrev.style.display = multi ? '' : 'none';
    lbNext.style.display = multi ? '' : 'none';
  }
  function step(d) {
    if (lbList.length < 2) return;
    current = (current + d + lbList.length) % lbList.length;
    showCurrent();
  }
  function close() {
    lb.hidden = true;
    lbImg.src = '';
    document.body.style.overflow = '';
  }

  slice(document.querySelectorAll('[data-lightbox]')).forEach(function (el) {
    el.addEventListener('click', function () {
      var pane = el.closest('.shot-pane');
      var list = pane ? slice(pane.querySelectorAll('[data-lightbox]')) : [el];
      open(list, list.indexOf(el));
    });
  });
  var closeBtn = document.getElementById('lbClose');
  if (closeBtn) closeBtn.addEventListener('click', close);
  if (lbPrev) lbPrev.addEventListener('click', function (e) { e.stopPropagation(); step(-1); });
  if (lbNext) lbNext.addEventListener('click', function (e) { e.stopPropagation(); step(1); });
  if (lb) lb.addEventListener('click', function (e) { if (e.target === lb) close(); });
  document.addEventListener('keydown', function (e) {
    if (!lb || lb.hidden) return;
    if (e.key === 'Escape') close();
    if (e.key === 'ArrowLeft') step(-1);
    if (e.key === 'ArrowRight') step(1);
  });
})();
