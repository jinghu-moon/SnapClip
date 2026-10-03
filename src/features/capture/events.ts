import { onCaptureEvents, type CaptureEventHandlers } from "../../infrastructure/tauri/events/capture";

/**
 * Subscribe to capture lifecycle events.
 *
 * Events carry ids, sizes and states only — never image data — so a consumer that
 * wants pixels goes back through `features/history/api` or a future artifact reader.
 */
export function subscribeCaptureEvents(
  handlers: CaptureEventHandlers,
): Promise<() => void> {
  return onCaptureEvents(handlers);
}

export type { CaptureEventHandlers };
