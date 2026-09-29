import { invoke } from "@tauri-apps/api/core";
import type { HistoryPage, PayloadKind } from "../../shared/contracts";

export function getImagePayloadDataUrl(contentHash: string): Promise<string> {
  return invoke<string>("image_payload_data_url", { contentHash });
}

export function copyPayload(contentHash: string, kind: PayloadKind): Promise<void> {
  return invoke("copy_payload", { contentHash, kind });
}

export function getHistoryPage(options: {
  query?: string | null;
  kind?: PayloadKind | null;
  cursor?: string | null;
  limit?: number;
} = {}): Promise<HistoryPage> {
  return invoke<HistoryPage>("history_page", options);
}
