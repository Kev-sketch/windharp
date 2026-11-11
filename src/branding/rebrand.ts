// Shared string-level rebranding used by every branding reconciler.

export const APP_NAME = 'Windharp';

/**
 * Case-insensitive, loop-safe replacement of every "YouTube Music" occurrence.
 *
 * Deliberately does NOT use a /g regex with .test(): that keeps `lastIndex`
 * state between calls, so alternating calls silently skip the replacement
 * (this exact bug made the original title fix a no-op).
 */
export const rebrandText = (value: string | null | undefined): string => {
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
