import { subscribe } from "../client";
import {
  CLIPBOARD_UPDATED_EVENT,
  OCR_STATUS_EVENT,
  type ClipboardUpdatedEvent,
} from "../../../shared/contracts";

/** Notified after the back-end persisted a clipboard publication. */
export function onClipboardUpdated(
  handler: (event: ClipboardUpdatedEvent) => void,
): Promise<() => void> {
  return subscribe<ClipboardUpdatedEvent>(CLIPBOARD_UPDATED_EVENT, handler);
}

/** Notified after an OCR job changed state. */
export function onOcrStatusChanged(
  handler: (event: { clipId: string; status: string }) => void,
): Promise<() => void> {
  return subscribe<{ clipId: string; status: string }>(OCR_STATUS_EVENT, handler);
}
