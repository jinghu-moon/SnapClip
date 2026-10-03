import { onClipboardUpdated, onOcrStatusChanged } from "../../infrastructure/tauri/events/clipboard";

/** Notified when the back-end persisted a new clipboard publication. */
export function onPublicationSaved(handler: () => void): Promise<() => void> {
  return onClipboardUpdated(() => handler());
}

/** Notified when an OCR job changes state. */
export function onOcrStatus(handler: () => void): Promise<() => void> {
  return onOcrStatusChanged(() => handler());
}
