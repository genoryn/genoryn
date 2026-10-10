(function () {
  try {
    var saved = localStorage.getItem('genoryn-theme');
    var theme = saved === 'light' || saved === 'dark' ? saved : window.matchMedia('(prefers-color-scheme: light)').matches ? 'light' : 'dark';
    document.documentElement.setAttribute('data-theme', theme);
  } catch (e) {
    document.documentElement.setAttribute('data-theme', 'dark');
  }
})();
