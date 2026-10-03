import { defineStore } from "pinia";

import { fetchHistoryPage } from "../api";
import type { ClipSummary, PayloadKind } from "../../../shared/contracts";

const PAGE_SIZE = 50;

/**
 * History list state.
 *
 * The store depends on the history feature API only; it never sees Tauri types or
 * command names.
 */
export const useHistoryStore = defineStore("history", {
  state: () => ({
    items: [] as ClipSummary[],
    nextCursor: null as string | null,
    query: "",
    kind: "all" as PayloadKind | "all",
    selectedId: null as string | null,
    isLoading: false,
    error: null as string | null,
    hasLoaded: false,
    requestVersion: 0,
  }),
  getters: {
    selectedItem(state): ClipSummary | null {
      return state.items.find((item) => item.id === state.selectedId) ?? null;
    },
  },
  actions: {
    async refresh() {
      this.requestVersion += 1;
      this.items = [];
      this.nextCursor = null;
      this.selectedId = null;
      this.hasLoaded = false;
      this.isLoading = false;
      await this.loadMore();
    },
    async loadMore() {
      if (this.isLoading || (this.hasLoaded && !this.nextCursor)) return;
      const requestVersion = this.requestVersion;
      const query = this.query.trim();
      this.isLoading = true;
      this.error = null;
      try {
        const page = await fetchHistoryPage({
          query: query || null,
          kind: this.kind === "all" ? null : this.kind,
          cursor: this.nextCursor,
          limit: PAGE_SIZE,
        });
        if (requestVersion !== this.requestVersion) return;
        const existing = new Set(this.items.map((item) => item.id));
        this.items.push(...page.items.filter((item) => !existing.has(item.id)));
        this.nextCursor = page.nextCursor;
        this.hasLoaded = true;
        if (!this.selectedId && this.items[0]) this.selectedId = this.items[0].id;
      } catch (error) {
        if (requestVersion !== this.requestVersion) return;
        this.error = error instanceof Error ? error.message : "历史查询失败";
      } finally {
        if (requestVersion === this.requestVersion) this.isLoading = false;
      }
    },
    select(id: string) {
      this.selectedId = id;
    },
    setQuery(query: string) {
      this.query = query;
    },
    setKind(kind: PayloadKind | "all") {
      this.kind = kind;
    },
  },
});
