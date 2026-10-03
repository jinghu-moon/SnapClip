import { getHistoryPage, getImagePayloadDataUrl } from "../../infrastructure/tauri/commands/history";

import type { HistoryPage, PayloadKind } from "../../shared/contracts";

export interface HistoryQuery {
  query?: string | null;
  kind?: PayloadKind | null;
  cursor?: string | null;
  limit?: number;
}

/** Read one page of history. */
export function fetchHistoryPage(query: HistoryQuery = {}): Promise<HistoryPage> {
  return getHistoryPage(query);
}

/** Thumbnail or full image for a history row. */
export function fetchImageDataUrl(contentHash: string): Promise<string> {
  return getImagePayloadDataUrl(contentHash);
}
