import { invoke } from "@tauri-apps/api/core";
import type { HistoryPage, PayloadKind } from "../../shared/contracts";

export function getHistoryPage(options: {
  query?: string | null;
  kind?: PayloadKind | null;
  cursor?: string | null;
  limit?: number;
} = {}): Promise<HistoryPage> {
  return invoke<HistoryPage>("history_page", options);
}
