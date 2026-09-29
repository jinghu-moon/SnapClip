export const IPC_SCHEMA_VERSION = 1 as const;

export type PayloadKind =
  | "text"
  | "html"
  | "rtf"
  | "image"
  | "files"
  | "other";

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

export interface ClipboardPublication {
  publicationId: string;
  capturedAtUnixMs: number;
  sourceApp: string | null;
  sourceExePath: string | null;
  payloads: PayloadRef[];
}

export interface ClipSummary {
  id: string;
  createdAtUnixMs: number;
  primaryKind: PayloadKind;
  previewText: string | null;
  sourceApp: string | null;
  sourceExePath: string | null;
  thumbnail: PayloadRef | null;
  payloads: PayloadRef[];
}

export interface HistoryPage {
  items: ClipSummary[];
  nextCursor: string | null;
}

export type ErrorCode =
  | "invalid_argument"
  | "not_found"
  | "conflict"
  | "unsupported"
  | "cancelled"
  | "storage"
  | "internal";

export interface IpcError {
  code: ErrorCode;
  message: string | null;
  traceId: string | null;
}
