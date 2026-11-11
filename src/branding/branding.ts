import { installIconRebrand } from '@/branding/icons';
import { getLogoSvg } from '@/branding/logo';

// Replace the in-page "YouTube Music" branding (logo + title) with Windharp
// branding. Pure client-side UI change inside our own webview; the
// music.youtube.com content itself is untouched.

export const APP_NAME = 'Windharp';

/**
 * Case-insensitive, loop-safe replacement of every "YouTube Music" occurrence.
 *
 * Deliberately does NOT use a /g regex with .test(): that keeps `lastIndex`
 * state between calls, so alternating calls silently skip the replacement.
 */
const rebrandText = (value: string | null | undefined): string => {
  const source = value == null ? '' : String(value);
  const lower = source.toLowerCase();
  const needle = 'youtube music';

  let out = '';
  let from = 0;

  for (;;) {
    const at = lower.indexOf(needle, from);
    if (at < 0) {
      out += source.slice(from);
      break;
    }
    out += source.slice(from, at) + APP_NAME;
    from = at + needle.length;
  }

  return out;
};

/**
 * YouTube Music DOES NOT set `document.title` — it creates a fresh <title>
 * element and writes `textContent` on it. That bypasses the Document.title
 * prototype setter entirely, so intercepting the setter is not enough (it was
 * the original implementation and it never took effect in the app).
 *
 * Correct approach: reconcile both `document.title` and every <title>
 * element's textContent, and re-run whenever the DOM mutates (title element
 * added/changed) plus on a slow interval as a backstop.
 */
export const installTitleRebrand = (): void => {
  const reconcile = (): void => {
    try {
      const current = document.title;
      const wanted = rebrandText(current);
      if (wanted !== current) {
        document.title = wanted;
      }
    } catch {
      // Title may be unavailable during early document construction.
    }

    const titles = document.getElementsByTagName('title');
    for (let i = 0; i < titles.length; i += 1) {
      const element = titles[i];
      const text = element.textContent;
      const wanted = rebrandText(text);
      if (wanted !== text) {
        element.textContent = wanted;
      }
    }
  };

  const isTitleNode = (node: Node): boolean =>
    node.nodeName === 'TITLE' ||
    (node.parentNode != null && node.parentNode.nodeName === 'TITLE');

  const observer = new MutationObserver((mutations) => {
    for (const mutation of mutations) {
      const target = mutation.target;
      if (target != null && target.nodeName === 'TITLE') return reconcile();

      if (mutation.type === 'childList') {
        for (const added of Array.from(mutation.addedNodes)) {
          if (isTitleNode(added)) return reconcile();
        }
      }
    }
  });

  observer.observe(document.documentElement, {
    childList: true,
    subtree: true,
    characterData: true,
  });

  reconcile();

  // Backstop for anything the observer cannot see (e.g. same-value writes).
  setInterval(reconcile, 500);
};

const isLogoAlreadySwapped = (logo: Element): boolean =>
  logo.querySelector('a > svg.wh-logo') !== null;

const swapLogo = (logo: Element): void => {
  // At least two instances exist: the nav-bar logo and a hidden one kept for
  // the collapsible guide drawer. Rebrand every instance.
  const anchor = logo.querySelector('a');
  if (!anchor) return;

  anchor.innerHTML = getLogoSvg();
  anchor.setAttribute('data-wh-logo', '1');
};

export const installLogoRebrand = (): void => {
  const apply = (): void => {
    document.querySelectorAll('ytmusic-logo').forEach((logo) => {
      if (!isLogoAlreadySwapped(logo)) swapLogo(logo);
    });
  };

  apply();

  const observer = new MutationObserver(() => apply());
  observer.observe(document.documentElement, {
    childList: true,
    subtree: true,
  });
};

export const setupBranding = (): void => {
  try {
    installTitleRebrand();
  } catch (error) {
    console.error('[pear] title rebrand failed', error);
  }

  try {
    installLogoRebrand();
  } catch (error) {
    console.error('[pear] logo rebrand failed', error);
  }

  try {
    installIconRebrand();
  } catch (error) {
    console.error('[pear] icon rebrand failed', error);
  }
};
