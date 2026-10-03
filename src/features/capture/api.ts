import {
  cancelCapture,
  confirmCapture,
  getCaptureState,
  startCapture,
} from "../../infrastructure/tauri/commands/capture";

import type { CaptureState } from "../../shared/contracts";

/**
 * Camera capture use cases.
 *
 * These are the low-frequency commands the toolbar and status panel use. The native
 * overlay owns `F5`, `Esc`, `Enter` and all mouse input, so nothing here is on a
 * high-frequency path.
 */

/** Start a session. Resolves to `false` when one is already running. */
export function beginCapture(): Promise<boolean> {
  return startCapture();
}

/** Cancel the running session, if any. */
export function abortCapture(): Promise<void> {
  return cancelCapture();
}

/** Confirm the current selection and write the artifact. */
export function acceptCapture(): Promise<void> {
  return confirmCapture();
}

/** Current session state, for a status indicator. */
export function readCaptureState(): Promise<CaptureState> {
  return getCaptureState();
}
