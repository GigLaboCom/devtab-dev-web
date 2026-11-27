/**
 * The script in every page's <head> (BaseLayout.astro): it sets the
 * visitor's theme on <html> before the first paint and wires the header's
 * switch (ThemeSwitch.astro). The site's content security policy allows it
 * by its hash, which astro.config.mjs computes from this same text, so the
 * two cannot drift apart.
 */
export const themeScript = `(() => {
  const order = ['system', 'light', 'dark'];
  const root = document.documentElement;
  let theme = 'system';
  try {
    theme = localStorage.getItem('devtab-theme') || 'system';
  } catch {
    // No storage (a private window, blocked site data): the system's theme.
  }
  if (!order.includes(theme)) theme = 'system';
  const apply = () => {
    if (theme === 'system') root.removeAttribute('data-theme');
    else root.setAttribute('data-theme', theme);
  };
  apply();
  addEventListener('DOMContentLoaded', () => {
    const button = document.getElementById('theme-switch');
    if (!button) return;
    const show = () => {
      const label = button.dataset['label' + theme[0].toUpperCase() + theme.slice(1)] || '';
      button.setAttribute('aria-label', label);
      button.title = label;
      for (const icon of button.querySelectorAll('[data-icon]')) {
        icon.classList.toggle('hidden', icon.dataset.icon !== theme);
      }
    };
    show();
    button.hidden = false;
    button.addEventListener('click', () => {
      theme = order[(order.indexOf(theme) + 1) % order.length];
      try {
        if (theme === 'system') localStorage.removeItem('devtab-theme');
        else localStorage.setItem('devtab-theme', theme);
      } catch {
        // Not kept: it holds for this page only.
      }
      apply();
      show();
    });
  });
})();`;
