<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from "vue";
import { storeToRefs } from "pinia";
import { useVirtualizer } from "@tanstack/vue-virtual";
import {
  IconClipboardText,
  IconFile,
  IconFileText,
  IconPhoto,
  IconRefresh,
  IconSearch,
  IconX,
} from "@tabler/icons-vue";
import HistoryItem from "./components/HistoryItem.vue";
import { useHistoryStore } from "./stores/history";
import type { PayloadKind } from "./shared/contracts";

const history = useHistoryStore();
const { items, isLoading, error, hasLoaded } = storeToRefs(history);
const queryInput = ref("");
const listRef = ref<HTMLElement | null>(null);
const searchInput = ref<HTMLInputElement | null>(null);
const rowHeight = 112;

const visibleItems = computed(() => items.value);
const virtualizer = useVirtualizer(computed(() => ({
  count: visibleItems.value.length,
  getScrollElement: () => listRef.value,
  estimateSize: () => rowHeight,
  overscan: 8,
})));
const virtualRows = computed(() => virtualizer.value.getVirtualItems());

let searchTimer: ReturnType<typeof setTimeout> | undefined;
watch(queryInput, (value) => {
  if (searchTimer) clearTimeout(searchTimer);
  searchTimer = setTimeout(async () => {
    history.setQuery(value);
    await history.refresh();
    await nextTick();
    virtualizer.value.measure();
  }, 220);
});

watch(() => history.kind, () => void history.refresh());

function selectVisible(index: number) {
  const item = visibleItems.value[index];
  if (item) history.select(item.id);
}

function onListScroll() {
  const element = listRef.value;
  if (!element || isLoading.value || !history.nextCursor) return;
  if (element.scrollTop + element.clientHeight >= element.scrollHeight - rowHeight * 3) {
    void history.loadMore();
  }
}

function onKeydown(event: KeyboardEvent) {
  if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "f") {
    event.preventDefault();
    searchInput.value?.focus();
  } else if (event.key === "Escape" && document.activeElement === searchInput.value) {
    queryInput.value = "";
    searchInput.value?.blur();
  }
}

function clearSearch() {
  queryInput.value = "";
  searchInput.value?.focus();
}

function setKind(kind: PayloadKind | "all") {
  history.setKind(kind);
}

onMounted(() => {
  window.addEventListener("keydown", onKeydown);
  void history.loadMore();
});
onUnmounted(() => {
  window.removeEventListener("keydown", onKeydown);
  if (searchTimer) clearTimeout(searchTimer);
});
</script>

<template>
  <div class="shell">
    <main class="main">
      <header class="toolbar">
        <label class="search">
          <IconSearch :size="17" />
          <input ref="searchInput" v-model="queryInput" type="text" placeholder="搜索历史内容" />
          <button v-if="queryInput" type="button" title="清除搜索" @click="clearSearch"><IconX :size="15" /></button>
        </label>
        <button class="toolbar__action" type="button" title="刷新历史" @click="history.refresh"><IconRefresh :size="17" /></button>
      </header>

      <section class="history-area">
        <div ref="listRef" class="list-scroll" @scroll="onListScroll">
          <div v-if="error" class="status status--error">
            <div><div>{{ error }}</div><button type="button" @click="history.refresh">重试</button></div>
          </div>
          <div v-else-if="hasLoaded && visibleItems.length === 0" class="status">没有匹配的历史记录</div>
          <div v-else-if="!hasLoaded && isLoading" class="status">正在加载历史记录…</div>
          <div v-else class="list-virtual" :style="{ height: `${virtualizer.getTotalSize()}px` }">
            <div
              v-for="row in virtualRows"
              :key="visibleItems[row.index]?.id"
              class="list-row"
              :style="{ transform: `translateY(${row.start}px)` }"
            >
              <HistoryItem
                v-if="visibleItems[row.index]"
                :item="visibleItems[row.index]"
                :selected="visibleItems[row.index].id === history.selectedId"
                @select="selectVisible(row.index)"
              />
            </div>
          </div>
          <button v-if="history.nextCursor && !isLoading" class="load-more" type="button" @click="history.loadMore">加载更多</button>
          <div v-if="isLoading && hasLoaded" class="status">正在加载更多…</div>
        </div>
      </section>

      <footer class="filter-bar" aria-label="按内容类型筛选">
        <button class="filter" :class="{ 'filter--active': history.kind === 'all' }" :aria-pressed="history.kind === 'all'" type="button" @click="setKind('all')">
          <IconClipboardText :size="18" /><span>全部</span>
        </button>
        <button class="filter" :class="{ 'filter--active': history.kind === 'text' }" :aria-pressed="history.kind === 'text'" type="button" @click="setKind('text')">
          <IconFileText :size="18" /><span>文本</span>
        </button>
        <button class="filter" :class="{ 'filter--active': history.kind === 'image' }" :aria-pressed="history.kind === 'image'" type="button" @click="setKind('image')">
          <IconPhoto :size="18" /><span>图片</span>
        </button>
        <button class="filter" :class="{ 'filter--active': history.kind === 'files' }" :aria-pressed="history.kind === 'files'" type="button" @click="setKind('files')">
          <IconFile :size="18" /><span>文件</span>
        </button>
        <span class="result-count">{{ items.length }}</span>
      </footer>
    </main>
  </div>
</template>
