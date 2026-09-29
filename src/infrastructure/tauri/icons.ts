import { invoke } from "@tauri-apps/api/core";

const cache = new Map<string, Promise<string | null>>();

export function getAppIcon(exePath: string): Promise<string | null> {
  const key = exePath.trim();
  if (!key) return Promise.resolve(null);
  let pending = cache.get(key);
  if (!pending) {
    pending = invoke<string>("get_app_icon", { exePath: key }).then(
      (dataUrl) => dataUrl,
      () => null,
    );
    cache.set(key, pending);
  }
  return pending;
}
