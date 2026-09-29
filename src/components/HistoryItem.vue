<script setup lang="ts">
import { onMounted, ref, watch } from "vue";
import {
  IconDeviceDesktop,
} from "@tabler/icons-vue";
import type { ClipSummary, PayloadKind } from "../shared/contracts";
import { getAppIcon } from "../infrastructure/tauri/icons";

const props = defineProps<{
  item: ClipSummary;
  selected: boolean;
}>();

const emit = defineEmits<{ select: [] }>();

const iconUrl = ref<string | null>(null);

const kindLabels: Record<PayloadKind, string> = {
  text: "文本",
  html: "HTML",
  rtf: "RTF",
  image: "图片",
  files: "文件",
  other: "其他",
};

function formatTime(timestamp: number) {
  const date = new Date(timestamp);
  const sameDay = new Date().toDateString() === date.toDateString();
  const time = new Intl.DateTimeFormat("zh-CN", { hour: "2-digit", minute: "2-digit" }).format(date);
  if (sameDay) return `今天 ${time}`;
  const day = new Intl.DateTimeFormat("zh-CN", { month: "numeric", day: "numeric" }).format(date);
  return `${day} ${time}`;
}

function formatSize(item: ClipSummary) {
  const size = item.payloads.reduce((total, payload) => total + payload.sizeBytes, 0);
  if (size < 1024) return `${size} B`;
  if (size < 1024 * 1024) return `${(size / 1024).toFixed(size < 10 * 1024 ? 1 : 0)} KB`;
  return `${(size / 1024 / 1024).toFixed(1)} MB`;
}

async function loadIcon() {
  iconUrl.value = null;
  const exePath = props.item.sourceExePath;
  if (!exePath) return;
  iconUrl.value = await getAppIcon(exePath);
}

onMounted(() => {
  void loadIcon();
});

watch(
  () => props.item.sourceExePath,
  () => {
    void loadIcon();
  },
);
</script>

<template>
  <button
    class="history-item"
    :class="{ 'history-item--selected': selected }"
    type="button"
    @click="emit('select')"
  >
    <span class="history-item__preview">{{ item.previewText || kindLabels[item.primaryKind] }}</span>
    <span class="history-item__meta">
      <span>{{ formatTime(item.createdAtUnixMs) }}</span>
      <span class="meta-dot">·</span>
      <span>{{ formatSize(item) }}</span>
      <span class="meta-dot">·</span>
      <span class="history-item__source">
        <img
          v-if="iconUrl"
          class="history-item__source-icon"
          :src="iconUrl"
          alt=""
          width="16"
          height="16"
        />
        <IconDeviceDesktop v-else :size="15" />
        <span>{{ item.sourceApp || "未知来源" }}</span>
      </span>
      <span class="history-item__formats">{{ item.payloads.length }}</span>
    </span>
  </button>
</template>
