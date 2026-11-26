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
        if (panes[j]) panes[j].hidden = !on;
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

  // ---- ambient dot-particle field: drifting acoustic waveform & concentric sound-wave glyphs ----
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
      { kind: 'wave', fx: 0.82, fy: 0.22, size: 390, tint: '45, 212, 191',  alpha: 0.30, amp: 24, period: 62000, phase: 0.0 },
      { kind: 'ring', fx: 0.11, fy: 0.58, size: 260, tint: '230, 184, 92',  alpha: 0.16, amp: 30, period: 82000, phase: 2.1 },
      { kind: 'wave', fx: 0.68, fy: 0.84, size: 310, tint: '77, 168, 255',  alpha: 0.13, amp: 28, period: 74000, phase: 4.0 },
      { kind: 'ring', fx: 0.34, fy: 0.38, size: 175, tint: '45, 212, 191',  alpha: 0.11, amp: 20, period: 92000, phase: 1.4 }
    ];
    var GLYPH_DEFS_SM = [
      { kind: 'wave', fx: 0.85, fy: 0.18, size: 230, tint: '45, 212, 191', alpha: 0.22, amp: 16, period: 62000, phase: 0.0 },
      { kind: 'ring', fx: 0.12, fy: 0.76, size: 170, tint: '230, 184, 92', alpha: 0.12, amp: 18, period: 82000, phase: 2.1 }
    ];

    function sampleAcousticGlyph(kind, size) {
      var s = 2;
      var off = document.createElement('canvas');
      off.width = off.height = Math.round(size * s);
      var c = off.getContext('2d');
      c.scale(s, s);
      c.strokeStyle = '#fff';
      c.lineCap = 'round';
      var cx = size / 2, cy = size / 2;

      if (kind === 'wave') {
        // Vertical studio acoustic waveform bars (symmetric hi-res silhouette)
        var bars = 13;
        var span = size * 0.72;
        var x0 = cx - span * 0.5;
        var stepX = span / (bars - 1);
        c.lineWidth = Math.max(7, size * 0.036);
        var heights = [0.16, 0.34, 0.56, 0.38, 0.76, 0.92, 0.64, 0.86, 0.52, 0.68, 0.40, 0.28, 0.14];
        for (var b = 0; b < bars; b++) {
          var bx = x0 + b * stepX;
          var bh = size * 0.36 * heights[b];
          c.beginPath();
          c.moveTo(bx, cy - bh);
          c.lineTo(bx, cy + bh);
          c.stroke();
        }
      } else {
        // Concentric audiophile vinyl / acoustic transducer rings
        c.lineWidth = Math.max(7, size * 0.042);
        [0.14, 0.27, 0.40].forEach(function (rf) {
          c.beginPath();
          c.arc(cx, cy, size * rf, 0, Math.PI * 2);
          c.stroke();
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
        if (pts.length <= 680) break;
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
        var pts = sampleAcousticGlyph(d.kind, size);
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
