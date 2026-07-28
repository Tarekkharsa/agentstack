/* Theme switching for the design-system pages. Load synchronously in <head>
   (before first paint) so the stored theme applies without a flash.
   The palette is pinned to "slate" by a data-palette attribute in the HTML;
   this script only owns light/dark. */
(function () {
  var theme;
  try { theme = localStorage.getItem('agentstack-theme'); } catch (e) {}
  if (theme !== 'light' && theme !== 'dark') theme = 'dark';
  var root = document.documentElement;
  root.setAttribute('data-theme', theme);

  function relabel() {
    var label = root.getAttribute('data-theme') === 'dark' ? 'Light mode' : 'Dark mode';
    var btns = document.querySelectorAll('[data-theme-toggle]');
    for (var i = 0; i < btns.length; i++) btns[i].textContent = label;
  }

  // A horizontally or vertically scrollable region must be reachable without
  // a pointer. Pages include generated code/table wrappers and authored demo
  // terminals, so derive this from actual layout instead of maintaining a
  // second selector list that drifts when a page is added.
  function makeScrollableRegionsFocusable() {
    var nodes = document.querySelectorAll('body *');
    for (var i = 0; i < nodes.length; i++) {
      var el = nodes[i];
      if (el.hasAttribute('tabindex')) continue;
      var style = getComputedStyle(el);
      var x = el.scrollWidth > el.clientWidth + 1 && /auto|scroll/.test(style.overflowX);
      var y = el.scrollHeight > el.clientHeight + 1 && /auto|scroll/.test(style.overflowY);
      if (x || y) el.setAttribute('tabindex', '0');
    }
  }

  function ready() {
    relabel();
    makeScrollableRegionsFocusable();
  }

  window.toggleTheme = function () {
    var next = root.getAttribute('data-theme') === 'dark' ? 'light' : 'dark';
    root.setAttribute('data-theme', next);
    try { localStorage.setItem('agentstack-theme', next); } catch (e) {}
    relabel();
  };

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', ready);
  } else {
    ready();
  }
  // Web fonts and page-local renderers can create overflow after DOM ready.
  window.addEventListener('load', makeScrollableRegionsFocusable);
  window.addEventListener('resize', makeScrollableRegionsFocusable);
})();
