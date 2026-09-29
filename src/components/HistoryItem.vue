<script setup lang="ts">
import { onMounted, ref, watch } from "vue";
import {
  IconDeviceDesktop,
  IconCopy,
} from "@tabler/icons-vue";
import type { ClipSummary, PayloadKind } from "../shared/contracts";
import { getAppIcon } from "../infrastructure/tauri/icons";
import { copyPayload, getImagePayloadDataUrl } from "../infrastructure/tauri/history";

const props = defineProps<{
  item: ClipSummary;
  selected: boolean;
}>();

const emit = defineEmits<{ select: []; copied: [message: string] }>();

const iconUrl = ref<string | null>(null);
const imageUrl = ref<string | null>(null);
const isCopying = ref(false);

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

function copyTarget() {
  return props.item.payloads.find((payload) => payload.kind === "image")
    ?? props.item.payloads.find((payload) => ["text", "html", "rtf", "other"].includes(payload.kind));
}

async function copyItem(event?: Event) {
  event?.stopPropagation();
  const payload = copyTarget();
  if (!payload || isCopying.value) return;
  isCopying.value = true;
  try {
    await copyPayload(payload.contentHash, payload.kind);
    emit("copied", "已复制到剪贴板");
  } catch {
    emit("copied", "复制失败");
  } finally {
    isCopying.value = false;
  }
}

function selectCard() {
  emit("select");
}

function onCardKeydown(event: KeyboardEvent) {
  if (event.key === "Enter" || event.key === " ") {
    event.preventDefault();
    selectCard();
  }
}

const ocrLabels: Record<string, string> = {
  queued: "OCR 排队中",
  running: "OCR 识别中",
  done: "已识别",
  failed: "OCR 失败",
  skipped: "OCR 已跳过",
};

const ocrErrorLabels: Record<string, string> = {
  language_unavailable: "缺少 Windows OCR 语言包",
  decode_failed: "图片解码失败",
  timeout: "OCR 超时",
  cancelled: "OCR 已取消",
  engine_failed: "Windows OCR 引擎失败",
};

async function loadIcon() {
  iconUrl.value = null;
  const exePath = props.item.sourceExePath;
  if (!exePath) return;
  iconUrl.value = await getAppIcon(exePath);
}

async function loadImage() {
  imageUrl.value = null;
  const image = props.item.payloads.find((payload) => payload.kind === "image");
  if (!image) return;
  try {
    imageUrl.value = await getImagePayloadDataUrl(image.contentHash);
  } catch {
    imageUrl.value = null;
  }
}

onMounted(() => {
  void loadIcon();
  void loadImage();
});

watch(
  () => props.item.sourceExePath,
  () => {
    void loadIcon();
    void loadImage();
  },
);
watch(() => props.item.payloads, () => void loadImage(), { deep: true });
</script>

<template>
  <article
    class="history-item"
    :class="{ 'history-item--selected': selected }"
    role="button"
    tabindex="0"
    @click="selectCard"
    @keydown="onCardKeydown"
  >
    <span v-if="imageUrl" class="history-item__image-wrap">
      <img class="history-item__image" :src="imageUrl" alt="剪贴板图片" />
    </span>
    <span v-else class="history-item__preview">{{ item.ocrText?.trim() || item.previewText?.trim() || kindLabels[item.primaryKind] }}</span>
    <span v-if="item.ocrStatus !== 'none'" class="history-item__ocr">
      <span>{{ item.ocrText?.trim() || (item.ocrErrorCode ? ocrErrorLabels[item.ocrErrorCode] : null) || ocrLabels[item.ocrStatus] }}</span>
    </span>
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
      <button
        class="history-item__copy"
        type="button"
        :disabled="isCopying || !copyTarget()"
        title="复制到剪贴板"
        aria-label="复制到剪贴板"
        @click="copyItem"
      ><IconCopy :size="15" /></button>
    </span>
  </article>
</template>
