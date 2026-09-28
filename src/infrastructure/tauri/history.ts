import { invoke } from "@tauri-apps/api/core";
import type { HistoryPage } from "../../shared/contracts";

export function getHistoryPage(options: {
  cursor?: string | null;
  limit?: number;
} = {}): Promise<HistoryPage> {
  return invoke<HistoryPage>("history_page", options);
}
