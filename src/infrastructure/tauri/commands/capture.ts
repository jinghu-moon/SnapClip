import { call } from "../client";

import type { CaptureState } from "../../../shared/contracts";

/**
 * Start a capture session.
 *
 * Returns `false` when a session is already running. The native overlay handles
 * `F5`, `Esc` and `Enter` itself, so this exists for the future toolbar and for the
 * status panel; it is not on any high-frequency path.
 */
export function startCapture(): Promise<boolean> {
  return call<boolean>("capture_start");
}

/** Cancel the running capture session, if any. */
export function cancelCapture(): Promise<void> {
  return call<void>("capture_cancel");
}

/** Confirm the current selection and produce an artifact. */
export function confirmCapture(): Promise<void> {
  return call<void>("capture_confirm");
}

/** Current capture session state. */
export function getCaptureState(): Promise<CaptureState> {
  return call<{ state: CaptureState }>("capture_state").then((view) => view.state);
}