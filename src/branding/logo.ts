// In-page Windharp logo: the app-icon wind-harp glyph + "Windharp" wordmark,
// drawn as an inline SVG that inherits the page's nav-bar text colour so it
// themes with the rest of the UI (dark and light modes alike).

const GLYPH_PATHS: [string, number][] = [
  // pillar
  ['M 312,352 L 312,808', 52],
  // neck: feather-light sweep, head curling into a gust of wind
  [
    'M 312,352 Q 600,290 745,505',
    34,
  ],
  [
    'M 745,505 C 785,480 815,430 790,390 C 770,358 725,365 722,398 C 720,420 740,432 758,424',
    34,
  ],
  // soundboard
  ['M 312,808 L 745,505', 46],
  // two strings, feet on the soundboard
  ['M 410,344 L 410,739', 46],
  ['M 590,374 L 590,613', 46],
];

const renderGlyphPaths = (): string =>
  GLYPH_PATHS.map(
    ([d, strokeWidth]) => `<path d="${d}" stroke-width="${strokeWidth}"/>`,
  ).join('');

export const getLogoSvg = (): string =>
  `<svg class="wh-logo" viewBox="0 0 152 26" height="24" preserveAspectRatio="xMinYMid meet" style="display:block" aria-hidden="true">` +
  `<g transform="translate(1,1) scale(0.0641) translate(-310,-350)" fill="none" stroke="var(--wh-logo-color, var(--yt-spec-text-primary, #fff))" stroke-linecap="round">` +
  renderGlyphPaths() +
  `</g>` +
  `<text x="42" y="20.5" font-size="19" font-weight="700" letter-spacing="0.2" fill="var(--wh-logo-color, var(--yt-spec-text-primary, #fff))" font-family="'Segoe UI',system-ui,-apple-system,Roboto,'Helvetica Neue',Arial,sans-serif">Windharp</text>` +
  `</svg>`;
