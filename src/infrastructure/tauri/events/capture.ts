import { subscribe } from "../client";
import {
  CAPTURE_EVENTS,
  type CaptureCancelledEvent,
  type CaptureCompletedEvent,
  type CaptureFailedEvent,
  type CaptureStartedEvent,
  type CaptureStateChangedEvent,
} from "../../../shared/contracts";

export interface CaptureEventHandlers {
  onStarted?: (event: CaptureStartedEvent) => void;
  onState?: (event: CaptureStateChangedEvent) => void;
  onCompleted?: (event: CaptureCompletedEvent) => void;
  onCancelled?: (event: CaptureCancelledEvent) => void;
  onFailed?: (event: CaptureFailedEvent) => void;
}

/**
 * Subscribe to every capture lifecycle event.
 *
 * The native overlay drives the session, so these are low-frequency status
 * notifications — never per-frame or per-mouse-move messages.
 */
export async function onCaptureEvents(
  handlers: CaptureEventHandlers,
): Promise<() => void> {
  const unlisten = await Promise.all([
    handlers.onStarted
      ? subscribe<CaptureStartedEvent>(CAPTURE_EVENTS.started, handlers.onStarted)
      : null,
    handlers.onState
      ? subscribe<CaptureStateChangedEvent>(CAPTURE_EVENTS.state, handlers.onState)
      : null,
    handlers.onCompleted
      ? subscribe<CaptureCompletedEvent>(CAPTURE_EVENTS.completed, handlers.onCompleted)
      : null,
    handlers.onCancelled
      ? subscribe<CaptureCancelledEvent>(CAPTURE_EVENTS.cancelled, handlers.onCancelled)
      : null,
    handlers.onFailed
      ? subscribe<CaptureFailedEvent>(CAPTURE_EVENTS.failed, handlers.onFailed)
      : null,
  ]);

  return () => {
    for (const stop of unlisten) stop?.();
  };
}
