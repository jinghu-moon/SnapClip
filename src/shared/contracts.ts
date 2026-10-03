/**
 * Contract types mirrored from the Rust DTOs.
 *
 * Anything that crosses the IPC boundary is declared here, so a mismatch shows up
 * as a TypeScript error in the feature that consumes it.
 */

/**
 * Frozen Phase 0 contract (docs/11 §2.2): the full lifecycle the native
 * overlay publishes. `preparing`/`adjusting`/`annotating`/`exporting` arrive with
 * their phases; `finishing` is the current produce state, replaced by `exporting`.
 */
export type CaptureState =
  | "idle"
  | "preparing"
  | "armed"
  | "selecting"
  | "selected"
  | "adjusting"
  | "annotating"
  | "exporting"
  | "finishing";

export type PublicationOrigin = "clipboard" | "capture";

export type PayloadKind =
  | "text"
  | "html"
  | "rtf"
  | "image"
  | "files"
  | "other";

export type OcrStatus =
  | "none"
  | "queued"
  | "running"
  | "done"
  | "failed"
  | "skipped";

export type OcrErrorCode =
  | "language_unavailable"
  | "decode_failed"
  | "timeout"
  | "cancelled"
  | "engine_failed";

export type ErrorCode =
  | "invalid_argument"
  | "not_found"
  | "conflict"
  | "unsupported"
  | "cancelled"
  | "storage"
  | "internal";

export interface ImageDimensions {
  width: number;
  height: number;
}

export interface PayloadRef {
  payloadId: string;
  contentHash: string;
  kind: PayloadKind;
  sizeBytes: number;
  mimeType: string | null;
  imageDimensions: ImageDimensions | null;
}

/** One observation of user content: a clipboard snapshot or a finished capture. */
export interface Publication {
  publicationId: string;
  origin: PublicationOrigin;
  capturedAtUnixMs: number;
  sourceApp: string | null;
  sourceExePath: string | null;
  payloads: PayloadRef[];
}

export interface ClipSummary {
  id: string;
  createdAtUnixMs: number;
  origin: PublicationOrigin;
  primaryKind: PayloadKind;
  previewText: string | null;
  sourceApp: string | null;
  sourceExePath: string | null;
  thumbnail: PayloadRef | null;
  payloads: PayloadRef[];
  ocrStatus: OcrStatus;
  ocrText: string | null;
  ocrLayout: string | null;
  ocrEngine: string | null;
  ocrUpdatedAt: number | null;
  ocrErrorCode: OcrErrorCode | null;
}

export interface HistoryPage {
  items: ClipSummary[];
  nextCursor: string | null;
}

export interface OcrStatusView {
  status: OcrStatus;
  engine: string | null;
  updatedAt: number | null;
  errorCode: OcrErrorCode | null;
}

export interface IpcError {
  code: ErrorCode;
  message: string | null;
  traceId: string | null;
}

// ---- event payloads -------------------------------------------------------

export interface CaptureStartedEvent {
  sessionId: string;
  monitorLeft: number;
  monitorTop: number;
  monitorWidth: number;
  monitorHeight: number;
  dpi: number;
  provider: string;
}

export interface CaptureStateChangedEvent {
  sessionId: string;
  state: CaptureState;
  dpi: number | null;
}

export interface CaptureCompletedEvent {
  sessionId: string;
  /** Restricted reference: a file inside the application artifact directory. */
  artifactRef: string;
  width: number;
  height: number;
}

export interface CaptureCancelledEvent {
  sessionId: string;
  reason: string;
}

export interface CaptureFailedEvent {
  sessionId: string | null;
  errorCode: string;
  provider: string;
  message: string;
}

export interface ClipboardUpdatedEvent {
  publicationId: string;
  origin: PublicationOrigin;
  payloadKinds: PayloadKind[];
}

/**
 * Event names, versioned so the contract can evolve without ambiguity.
 *
 * Only `[a-z0-9-]` may appear here. Tauri validates event names against a narrow
 * whitelist and a rejected name silently drops the event, so no `.` or `scheme://`
 * prefix. These strings must match `src-tauri/src/events/mod.rs` exactly.
 */
export const CAPTURE_EVENTS = {
  started: "capture-started-v1",
  state: "capture-state-v1",
  completed: "capture-completed-v1",
  cancelled: "capture-cancelled-v1",
  failed: "capture-failed-v1",
} as const;

export const CLIPBOARD_UPDATED_EVENT = "clipboard-updated-v1";
export const OCR_STATUS_EVENT = "ocr-status-v1";
