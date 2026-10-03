import { backfillOcr, getOcrStatus, retryFailedOcr } from "../../infrastructure/tauri/commands/ocr";

import type { OcrStatusView } from "../../shared/contracts";

export interface OcrBatchResult {
  enqueued: number;
  skipped: number;
  failed: number;
}

/** Queue OCR for image clips that have not been processed yet. */
export function backfill(limit?: number): Promise<OcrBatchResult> {
  return backfillOcr(limit);
}

/** Re-queue OCR for clips whose last attempt failed. */
export function retryFailed(limit?: number): Promise<OcrBatchResult> {
  return retryFailedOcr(limit);
}

/** OCR state of one clip. */
export function status(clipId: string): Promise<OcrStatusView> {
  return getOcrStatus(clipId);
}
