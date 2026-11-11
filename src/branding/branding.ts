import { getLogoSvg } from '@/branding/logo';

// Replace the in-page "YouTube Music" branding (nav-bar logo + document
// title) with Windharp branding. Pure client-side UI change inside our own
// webview; the music.youtube.com content itself is untouched.

const APP_NAME = 'Windharp';

const TITLE_RE = /YouTube\s*Music/gi;

const rebrandTitle = (raw: string | null | undefined): string =>
  (raw == null ? '' : String(raw).replace(TITLE_RE, APP_NAME)).trim() ||
  APP_NAME;

export const installTitleRebrand = (): void => {
  // The page (and its router) set document.title repeatedly. Intercept the
  // native setter so every write lands rebranded, then rewrite whatever the
  // page already set before we installed.
  const descriptor =
    Object.getOwnPropertyDescriptor(Document.prototype, 'title') ??
    Object.getOwnPropertyDescriptor(HTMLDocument.prototype, 'title');

  if (!descriptor?.set || !descriptor.get) return;

  const { set: nativeSet, get: nativeGet } = descriptor;

  Object.defineProperty(document, 'title', {
    configurable: true,
    get() {
      return nativeGet.call(document);
    },
    set(value) {
      nativeSet.call(document, rebrandTitle(value));
    },
  });

  // Rewrite the title the page set before this script ran.
  nativeSet.call(document, rebrandTitle(nativeGet.call(document)));
};

const isLogoAlreadySwapped = (logo: Element): boolean =>
  logo.querySelector('a > svg.wh-logo') !== null;

const swapLogo = (logo: Element): void => {
  // At least two instances exist: the nav-bar logo and a hidden one kept
  // for the collapsible guide drawer. Rebrand every instance.
  const anchor = logo.querySelector('a');
  if (!anchor) return;

  anchor.innerHTML = getLogoSvg();
  anchor.setAttribute('data-wh-logo', '1');
};

let observer: MutationObserver | null = null;

export const installLogoRebrand = (): void => {
  const apply = (): void => {
    document
      .querySelectorAll('ytmusic-logo')
      .forEach((logo) => void applyOne(logo));
  };

  const applyOne = (logo: Element): void => {
    if (!isLogoAlreadySwapped(logo)) swapLogo(logo);
  };

  apply();

  observer = new MutationObserver(() => apply());
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
};
