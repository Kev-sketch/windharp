// Replace the remaining YouTube-Music identity the page installs for itself:
// favicons, the web-app manifest, and the og:*/itemprop metadata that name the
// site "YouTube Music".
//
// YouTube re-creates these during boot and again on some SPA navigations, so
// everything here is applied through a reconciler (observer + backstop timer),
// mirroring the title logic in branding.ts.

import { APP_NAME } from '@/branding/branding';

/**
 * Square Windharp app glyph as a standalone SVG data URI — used for the
 * favicon. Deliberately drawn from the same master artwork as the nav logo so
 * there is a single visual identity.
 */
const buildGlyphSvg = (): string => {
  const glyphPaths: [string, number][] = [
    ['M 312,352 L 312,808', 52],
    ['M 312,352 Q 600,290 745,505', 34],
    [
      'M 745,505 C 785,480 815,430 790,390 C 770,358 725,365 722,398 C 720,420 740,432 758,424',
      34,
    ],
    ['M 312,808 L 745,505', 46],
    ['M 410,344 L 410,739', 46],
    ['M 590,374 L 590,613', 46],
  ];

  const paths = glyphPaths
    .map(([d, width]) => `<path d="${d}" stroke-width="${width}"/>`)
    .join('');

  // 512 square: sky-gradient rounded rect + the white wire-harp.
  return (
    `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 512 512" width="512" height="512">` +
    `<defs><linearGradient id="sky" x1="0" y1="0" x2="0" y2="1">` +
    `<stop offset="0%" stop-color="#8FE3F7"/><stop offset="50%" stop-color="#4FB2E4"/><stop offset="100%" stop-color="#1D63B8"/>` +
    `</linearGradient></defs>` +
    `<rect width="512" height="512" rx="116" fill="url(#sky)"/>` +
    `<g transform="translate(40,40) scale(0.4219)" fill="none" stroke="#ffffff" stroke-linecap="round">${paths}</g>` +
    `</svg>`
  );
};

const ICON_SIZES = [16, 32, 48, 96, 144, 180, 192, 512];

/**
 * Idempotent: a favicon is "done" once our marker attribute is present.
 */
const reconcileIcons = (dataUri: string): void => {
  const head = document.head;
  if (head == null) return;

  const marked = head.querySelectorAll('link[data-wh-icon]');
  const existing = head.querySelectorAll('link[rel*="icon" i]');

  // Drop the page's own icon links (they point at YouTube-hosted artwork).
  existing.forEach((link) => {
    if (!link.hasAttribute('data-wh-icon')) link.remove();
  });

  if (marked.length > 0) return;

  for (const size of ICON_SIZES) {
    const link = document.createElement('link');
    link.rel = 'icon';
    link.type = 'image/svg+xml';
    link.sizes = `${size}x${size}`;
    link.href = dataUri;
    link.setAttribute('data-wh-icon', '1');
    head.appendChild(link);
  }
};

/**
 * The manifest names the app "YouTube Music"; point it at an inline manifest so
 * the installed/web-app identity is ours.
 */
const reconcileManifest = (): void => {
  const head = document.head;
  if (head == null) return;

  const manifestLink = head.querySelector('link[rel="manifest"]');
  if (manifestLink == null || manifestLink.hasAttribute('data-wh-manifest')) {
    return;
  }

  const manifest = {
    name: APP_NAME,
    short_name: APP_NAME,
    start_url: '/',
    display: 'standalone',
    background_color: '#101010',
    theme_color: '#101010',
  };

  manifestLink.setAttribute(
    'href',
    `data:application/manifest+json,${encodeURIComponent(JSON.stringify(manifest))}`,
  );
  manifestLink.setAttribute('data-wh-manifest', '1');
};

/** og:site_name / og:title / itemprop=name still read "YouTube Music". */
const reconcileMetadata = (): void => {
  const selectors = [
    'meta[property="og:site_name"]',
    'meta[property="og:title"]',
    'meta[itemprop="name"]',
    'meta[name="apple-mobile-web-app-title"]',
    'meta[name="application-name"]',
  ];

  for (const selector of selectors) {
    document.querySelectorAll(selector).forEach((meta) => {
      const content = meta.getAttribute('content') ?? '';
      if (/youtube\s*music/i.test(content)) {
        meta.setAttribute('content', APP_NAME);
      }
    });
  }
};

export const installIconRebrand = (): void => {
  const dataUri = `data:image/svg+xml,${encodeURIComponent(buildGlyphSvg())}`;

  const reconcile = (): void => {
    reconcileIcons(dataUri);
    reconcileManifest();
    reconcileMetadata();
  };

  reconcile();

  const observer = new MutationObserver(() => reconcile());
  observer.observe(document.documentElement, {
    childList: true,
    subtree: true,
  });

  setInterval(reconcile, 1000);
};
