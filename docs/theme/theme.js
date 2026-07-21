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

  window.toggleTheme = function () {
    var next = root.getAttribute('data-theme') === 'dark' ? 'light' : 'dark';
    root.setAttribute('data-theme', next);
    try { localStorage.setItem('agentstack-theme', next); } catch (e) {}
    relabel();
  };

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', relabel);
  } else {
    relabel();
  }
})();
