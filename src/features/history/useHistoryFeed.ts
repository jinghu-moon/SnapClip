import { onMounted, onUnmounted } from "vue";

import { onPublicationSaved, onOcrStatus } from "../ocr/events";
import { useHistoryStore } from "./stores/history";

/**
 * Keep the history list in sync with the back-end.
 *
 * Subscriptions live in a composable so the history panel never imports the Tauri
 * event API itself.
 */
export function useHistoryFeed() {
  const history = useHistoryStore();
  let stops: Array<() => void> = [];

  onMounted(async () => {
    try {
      stops = await Promise.all([
        onPublicationSaved(() => void history.refresh()),
        onOcrStatus(() => void history.refresh()),
      ]);
    } catch (error) {
      // A plain Vite tab has no Tauri event bridge. History remains usable
      // through commands when embedded in Tauri, and the UI must still mount
      // while the bridge is unavailable during development or diagnostics.
      console.warn("[snapclip] live history events unavailable", error);
      stops = [];
    }
  });

  onUnmounted(() => {
    for (const stop of stops) stop();
    stops = [];
  });

  return history;
}
