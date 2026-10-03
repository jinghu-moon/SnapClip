import { call } from "../client";

import type { OcrStatusView } from "../../../shared/contracts";

export interface OcrBatchResult {
  enqueued: number;
  skipped: number;
  failed: number;
}

/** Queue OCR for image clips that have not been processed yet. */
export function backfillOcr(limit?: number): Promise<OcrBatchResult> {
  return call<OcrBatchResult>("ocr_backfill", { limit: limit ?? null });
}

/** Re-queue OCR for clips whose last attempt failed. */
export function retryFailedOcr(limit?: number): Promise<OcrBatchResult> {
  return call<OcrBatchResult>("ocr_retry_failed", { limit: limit ?? null });
}

/** OCR state of one clip. */
export function getOcrStatus(clipId: string): Promise<OcrStatusView> {
  return call<OcrStatusView>("ocr_get_status", { clipId });
}