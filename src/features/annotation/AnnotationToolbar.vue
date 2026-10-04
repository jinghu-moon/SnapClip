<script setup lang="ts">
/**
 * Annotation toolbar.
 *
 * A pure command producer: every control forwards one coarse
 * [`AnnotationCommand`] through the feature API and owns no document state. Tool
 * highlight is the only local state — the active drawing tool is presentation, held
 * by the overlay controller, mirrored here just so the button can light up.
 *
 * Low frequency by design: nothing here runs per mouse move or pixel frame, and the
 * window that hosts this must stay out of the capture pixel pipeline (docs/11 §7.1).
 */
import { ref } from "vue";
import IconPointer from "@tabler/icons-vue/dist/esm/icons/IconPointer.mjs";
import IconRectangle from "@tabler/icons-vue/dist/esm/icons/IconRectangle.mjs";
import IconCircle from "@tabler/icons-vue/dist/esm/icons/IconCircle.mjs";
import IconArrowNarrowRight from "@tabler/icons-vue/dist/esm/icons/IconArrowNarrowRight.mjs";
import IconMinus from "@tabler/icons-vue/dist/esm/icons/IconMinus.mjs";
import IconTypography from "@tabler/icons-vue/dist/esm/icons/IconTypography.mjs";
import IconPencil from "@tabler/icons-vue/dist/esm/icons/IconPencil.mjs";
import IconHighlight from "@tabler/icons-vue/dist/esm/icons/IconHighlight.mjs";
import IconArrowBackUp from "@tabler/icons-vue/dist/esm/icons/IconArrowBackUp.mjs";
import IconArrowForwardUp from "@tabler/icons-vue/dist/esm/icons/IconArrowForwardUp.mjs";
import IconTrash from "@tabler/icons-vue/dist/esm/icons/IconTrash.mjs";
import IconCopy from "@tabler/icons-vue/dist/esm/icons/IconCopy.mjs";
import IconArrowNarrowUp from "@tabler/icons-vue/dist/esm/icons/IconArrowNarrowUp.mjs";
import IconArrowNarrowDown from "@tabler/icons-vue/dist/esm/icons/IconArrowNarrowDown.mjs";

import {
  bringAnnotationForward,
  deleteSelectedAnnotation,
  duplicateSelectedAnnotation,
  pickAnnotationTool,
  redoAnnotation,
  selectAnnotationTool,
  sendAnnotationBackward,
  setAnnotationFillColor,
  setAnnotationStrokeColor,
  setAnnotationStrokeWidth,
  undoAnnotation,
} from "./api";

import type { AnnotationKind, Rgba } from "../../shared/contracts";

/** Drawing tools with their button icon; `null` entry is the select tool. */
const tools: ReadonlyArray<{ kind: AnnotationKind | null; label: string }> = [
  { kind: null, label: "选择" },
  { kind: "rectangle", label: "矩形" },
  { kind: "ellipse", label: "椭圆" },
  { kind: "arrow", label: "箭头" },
  { kind: "line", label: "直线" },
  { kind: "text", label: "文字" },
  { kind: "freehand", label: "手绘" },
  { kind: "highlight", label: "高亮" },
];

// Swatches are sRGB on 0–1; the document stores premultiplied-ready RGBA.
const strokeColors: ReadonlyArray<Rgba> = [
  [0.0, 0.47, 0.83, 1.0], // accent blue (default)
  [0.86, 0.2, 0.16, 1.0], // red
  [0.13, 0.6, 0.22, 1.0], // green
  [0.95, 0.65, 0.07, 1.0], // amber
  [0.12, 0.13, 0.15, 1.0], // near-black
];

const fillColors: ReadonlyArray<Rgba | null> = [
  null, // no fill
  [0.0, 0.47, 0.83, 0.25],
  [0.86, 0.2, 0.16, 0.25],
  [0.95, 0.65, 0.07, 0.25],
];

const strokeWidths: ReadonlyArray<number> = [2, 3, 5, 8];

// Local mirror of the controller's active tool, purely for button highlighting.
const activeTool = ref<AnnotationKind | null>(null);

function cssColor(rgba: Rgba): string {
  const [r, g, b, a] = rgba;
  return `rgba(${Math.round(r * 255)}, ${Math.round(g * 255)}, ${Math.round(b * 255)}, ${a})`;
}

async function chooseTool(kind: AnnotationKind | null): Promise<void> {
  activeTool.value = kind;
  await (kind === null ? selectAnnotationTool() : pickAnnotationTool(kind));
}

function run(action: () => Promise<void>): void {
  void action().catch((error: unknown) => {
    console.error("[snapclip][annotation] command failed", error);
  });
}
</script>

<template>
  <div class="annotation-toolbar" role="toolbar" aria-label="标注工具栏">
    <div class="group group--tools">
      <button
        v-for="tool in tools"
        :key="tool.label"
        class="btn"
        :class="{ 'btn--active': activeTool === tool.kind }"
        type="button"
        :title="tool.label"
        :aria-label="tool.label"
        :aria-pressed="activeTool === tool.kind"
        @click="run(() => chooseTool(tool.kind))"
      >
        <IconPointer v-if="tool.kind === null" :size="18" />
        <IconRectangle v-else-if="tool.kind === 'rectangle'" :size="18" />
        <IconCircle v-else-if="tool.kind === 'ellipse'" :size="18" />
        <IconArrowNarrowRight v-else-if="tool.kind === 'arrow'" :size="18" />
        <IconMinus v-else-if="tool.kind === 'line'" :size="18" />
        <IconTypography v-else-if="tool.kind === 'text'" :size="18" />
        <IconPencil v-else-if="tool.kind === 'freehand'" :size="18" />
        <IconHighlight v-else-if="tool.kind === 'highlight'" :size="18" />
      </button>
    </div>

    <div class="group group--stroke">
      <button
        v-for="(color, index) in strokeColors"
        :key="`stroke-${index}`"
        class="swatch"
        type="button"
        title="描边颜色"
        aria-label="描边颜色"
        :style="{ background: cssColor(color) }"
        @click="run(() => setAnnotationStrokeColor(color))"
      ></button>
    </div>

    <div class="group group--fill">
      <button
        v-for="(color, index) in fillColors"
        :key="`fill-${index}`"
        class="swatch swatch--fill"
        type="button"
        title="填充颜色"
        aria-label="填充颜色"
        :style="{ background: color ? cssColor(color) : 'transparent' }"
        @click="run(() => setAnnotationFillColor(color))"
      ></button>
    </div>

    <div class="group group--width">
      <button
        v-for="width in strokeWidths"
        :key="`w-${width}`"
        class="btn btn--text"
        type="button"
        :title="`线宽 ${width}px`"
        :aria-label="`线宽 ${width} 像素`"
        @click="run(() => setAnnotationStrokeWidth(width))"
      >{{ width }}</button>
    </div>

    <div class="group group--edit">
      <button class="btn" type="button" title="撤销" aria-label="撤销" @click="run(undoAnnotation)">
        <IconArrowBackUp :size="18" />
      </button>
      <button class="btn" type="button" title="重做" aria-label="重做" @click="run(redoAnnotation)">
        <IconArrowForwardUp :size="18" />
      </button>
      <button class="btn" type="button" title="复制" aria-label="复制所选" @click="run(duplicateSelectedAnnotation)">
        <IconCopy :size="18" />
      </button>
      <button class="btn" type="button" title="删除" aria-label="删除所选" @click="run(deleteSelectedAnnotation)">
        <IconTrash :size="18" />
      </button>
      <button class="btn" type="button" title="上移一层" aria-label="上移一层" @click="run(bringAnnotationForward)">
        <IconArrowNarrowUp :size="18" />
      </button>
      <button class="btn" type="button" title="下移一层" aria-label="下移一层" @click="run(sendAnnotationBackward)">
        <IconArrowNarrowDown :size="18" />
      </button>
    </div>
  </div>
</template>

<style scoped>
.annotation-toolbar {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 6px 8px;
  border: 1px solid #d9e0e5;
  border-radius: 10px;
  background: rgba(250, 251, 252, 0.97);
  box-shadow: 0 4px 14px rgba(26, 53, 67, 0.16);
  user-select: none;
}
.group {
  display: flex;
  align-items: center;
  gap: 4px;
  padding-right: 10px;
  border-right: 1px solid #e2e6e9;
}
.group:last-child {
  padding-right: 0;
  border-right: 0;
}
.btn {
  display: grid;
  place-items: center;
  width: 30px;
  height: 30px;
  padding: 0;
  border: 1px solid transparent;
  border-radius: 6px;
  background: transparent;
  color: #55636e;
}
.btn:hover {
  background: #edf1f3;
  color: #2e789d;
}
.btn--active {
  border-color: #9bbdd2;
  background: #e7f2fa;
  color: #2e789d;
}
.btn--text {
  font-size: 13px;
  font-weight: 600;
}
.swatch {
  width: 20px;
  height: 20px;
  padding: 0;
  border: 1px solid #c8d2d8;
  border-radius: 5px;
}
.swatch:hover {
  border-color: #8fb5c7;
}
.swatch--fill {
  background-image: linear-gradient(45deg, #f0f2f4 25%, transparent 25%, transparent 75%, #f0f2f4 75%);
  background-size: 8px 8px;
}
</style>
