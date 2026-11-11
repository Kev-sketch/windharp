// Replace the remaining YouTube-Music identity the page installs for itself:
// favicons, the web-app manifest, and the og:*/itemprop metadata that name the
// site "YouTube Music".
//
// YouTube re-creates these during boot and again on some SPA navigations, so
// everything here is applied through a reconciler (observer + backstop timer),
// mirroring the title logic in branding.ts.

import { APP_NAME, rebrandText } from '@/branding/rebrand';

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

/**
 * og:site_name / og:title / itemprop=name still name the site "YouTube Music".
 * Deeper routes use longer strings ("Explore new music… | YouTube Music"), so
 * this rewrites every occurrence rather than only an exact match.
 */
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
      const rewritten = rebrandText(content);
      if (rewritten !== content) {
        meta.setAttribute('content', rewritten);
      }
    });
  }
};

/**
 * Some YTM surfaces (e.g. search-result section labels) render the words
 * "YouTube Music" as ordinary visible text. Rewrite those text nodes.
 *
 * Deliberately conservative: text nodes only (never innerHTML), never inside
 * form controls / editable regions, and it walks with a TreeWalker so it does
 * not thrash the observer on every keystroke.
 */
const reconcileVisibleText = (): void => {
  const root = document.body;
  if (root == null) return;

  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT, {
    acceptNode(node) {
      const parent = node.parentElement;
      if (parent == null) return NodeFilter.FILTER_REJECT;

      const tag = parent.tagName;
      if (
        tag === 'SCRIPT' ||
        tag === 'STYLE' ||
        tag === 'NOSCRIPT' ||
        tag === 'TEXTAREA' ||
        tag === 'INPUT' ||
        tag === 'TITLE'
      ) {
        return NodeFilter.FILTER_REJECT;
      }

      if (parent.isContentEditable) return NodeFilter.FILTER_REJECT;

      const value = node.nodeValue;
      if (value == null || !/youtube\s*music/i.test(value)) {
        return NodeFilter.FILTER_REJECT;
      }

      return NodeFilter.FILTER_ACCEPT;
    },
  });

  const pending: Text[] = [];
  for (let node = walker.nextNode(); node != null; node = walker.nextNode()) {
    pending.push(node as Text);
  }

  for (const node of pending) {
    const current = node.nodeValue ?? '';
    const rewritten = rebrandText(current);
    if (rewritten !== current) {
      node.nodeValue = rewritten;
    }
  }
};

export const installIconRebrand = (): void => {
  const dataUri = `data:image/svg+xml,${encodeURIComponent(buildGlyphSvg())}`;

  // Every step is isolated: a throw in one (e.g. a moment where head is null)
  // must never skip the others. Previously an exception in any step silently
  // aborted the remainder, which is why some SPA routes came out rebranded and
  // others did not.
  const step = (fn: () => void, name: string): void => {
    try {
      fn();
    } catch (error) {
      console.error(`[pear] branding step failed: ${name}`, error);
    }
  };

  const fullReconcile = (): void => {
    step(() => reconcileIcons(dataUri), 'icons');
    step(reconcileManifest, 'manifest');
    step(reconcileMetadata, 'metadata');
    step(reconcileVisibleText, 'visible-text');
  };

  const cheapReconcile = (): void => {
    step(() => reconcileIcons(dataUri), 'icons');
    step(reconcileManifest, 'manifest');
    step(reconcileMetadata, 'metadata');
  };

  fullReconcile();

  let scheduled = false;
  const schedule = (): void => {
    if (scheduled) return;
    scheduled = true;
    // Coalesce mutation bursts into one pass on the next frame.
    requestAnimationFrame(() => {
      scheduled = false;
      fullReconcile();
    });
  };

  // Route changes rebuild whole subtrees, so the observer drives the FULL pass
  // (visible text included) rather than only the cheap subset.
  const observer = new MutationObserver(schedule);
  observer.observe(document.documentElement, {
    childList: true,
    subtree: true,
  });

  // The observer cannot see in-place writes to already-mounted text nodes, and
  // requestAnimationFrame stalls while the window is occluded — so keep timers
  // too. Both paths are idempotent and cheap.
  setInterval(cheapReconcile, 800);
  setInterval(() => step(reconcileVisibleText, 'visible-text'), 1200);
};
