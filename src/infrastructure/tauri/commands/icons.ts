import { call } from "../client";

const cache = new Map<string, Promise<string | null>>();

/**
 * Executable icon as a `data:image/png;base64,...` URL.
 *
 * Results are cached per executable, including failures, because history rows
 * request the same icon repeatedly while scrolling.
 */
export function getAppIcon(exePath: string): Promise<string | null> {
  const key = exePath.trim();
  if (!key) return Promise.resolve(null);
  let pending = cache.get(key);
  if (!pending) {
    pending = call<string>("get_app_icon", { exePath: key }).then(
      (dataUrl) => dataUrl,
      () => null,
    );
    cache.set(key, pending);
  }
  return pending;
}