/* Line-by-line replay for `.term[data-replay]` blocks.
 *
 * Progressive enhancement, deliberately: the full transcript lives in the
 * HTML (selectable, indexable, readable without JS). This script only
 * animates its appearance — command rows (`.row.cmd`) type out their `.t`
 * span, output rows appear line by line. With prefers-reduced-motion, or
 * without IntersectionObserver, nothing is hidden and nothing moves.
 */
(function () {
  'use strict';
  var reduce =
    window.matchMedia &&
    window.matchMedia('(prefers-reduced-motion: reduce)').matches;
  if (reduce || !('IntersectionObserver' in window)) return;

  Array.prototype.slice
    .call(document.querySelectorAll('.term[data-replay]'))
    .forEach(function (term) {
      var rows = Array.prototype.slice.call(term.querySelectorAll('.row'));
      if (!rows.length) return;
      var btn = term.querySelector('.tr-replay');
      var timer = null;
      var playing = false;

      function reset() {
        clearTimeout(timer);
        rows.forEach(function (r) {
          r.classList.remove('on', 'cur', 'typing');
          var t = r.querySelector('.t');
          if (t && t.dataset.full) t.textContent = t.dataset.full;
        });
        term.classList.add('tr-armed');
        term.classList.remove('tr-done');
      }

      function play() {
        if (playing) return;
        playing = true;
        reset();
        var i = 0;
        function next() {
          if (i > 0) rows[i - 1].classList.remove('cur');
          if (i >= rows.length) {
            term.classList.add('tr-done');
            playing = false;
            return;
          }
          var r = rows[i++];
          r.classList.add('on', 'cur');
          var t = r.classList.contains('cmd') ? r.querySelector('.t') : null;
          if (t) {
            var full = t.dataset.full || (t.dataset.full = t.textContent);
            t.textContent = '';
            r.classList.add('typing');
            // Time-based, not tick-based: when a background tab clamps
            // timers to 1s, each tick still reveals the RIGHT amount of
            // text, so the worst case degrades to one line per second
            // instead of one character per second.
            var started = performance.now();
            (function type() {
              var n = Math.min(
                full.length,
                Math.floor((performance.now() - started) / 14)
              );
              t.textContent = full.slice(0, n);
              if (n < full.length) {
                timer = setTimeout(type, 14);
              } else {
                r.classList.remove('typing');
                timer = setTimeout(next, 300);
              }
            })();
          } else {
            timer = setTimeout(next, parseInt(r.dataset.d || '130', 10));
          }
        }
        next();
      }

      // Hide rows only once we know we can animate them (no flash, and the
      // block keeps its full height — visibility, not display).
      reset();

      if (btn) {
        btn.addEventListener('click', function () {
          playing = false;
          play();
        });
      }
      // Don't burn the one automatic playback while the page is hidden
      // (e.g. opened in a background tab) — wait for it to become visible.
      function playWhenVisible() {
        if (!document.hidden) return play();
        var onVis = function () {
          if (!document.hidden) {
            document.removeEventListener('visibilitychange', onVis);
            play();
          }
        };
        document.addEventListener('visibilitychange', onVis);
      }
      var io = new IntersectionObserver(
        function (entries) {
          entries.forEach(function (e) {
            if (e.isIntersecting) {
              io.disconnect();
              playWhenVisible();
            }
          });
        },
        { threshold: 0.35 }
      );
      io.observe(term);
    });
})();
