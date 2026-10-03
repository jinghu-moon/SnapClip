import { call } from "../client";

import type { HistoryPage, PayloadKind } from "../../../shared/contracts";

/** One page of history, newest first. */
export async function getHistoryPage(options: {
  query?: string | null;
  kind?: PayloadKind | null;
  cursor?: string | null;
  limit?: number;
} = {}): Promise<HistoryPage> {
  const startedAt = performance.now();
  const initialPage = options.cursor == null && options.query == null && options.kind == null;
  if (initialPage) console.info("[snapclip][frontend] initial history IPC started");
  try {
    const page = await call<HistoryPage>("history_page", options);
    if (initialPage) {
      console.info(
        `[snapclip][frontend] initial history IPC completed elapsed_ms=${Math.round(performance.now() - startedAt)} items=${page.items.length}`,
      );
    }
    return page;
  } catch (error) {
    if (initialPage) {
      console.error(
        `[snapclip][frontend] initial history IPC failed elapsed_ms=${Math.round(performance.now() - startedAt)}`,
        error,
      );
    }
    throw error;
  }
}

/** Thumbnail/full image for a history row, as a data URL. */
export function getImagePayloadDataUrl(contentHash: string): Promise<string> {
  return call<string>("image_payload_data_url", { contentHash });
}
