<script setup lang="ts">
/**
 * Application shell.
 *
 * Assembles feature panels and owns nothing else: all history state, clipboard
 * actions and Tauri access live inside their features and the infrastructure layer.
 */
import HistoryPanel from "../features/history/HistoryPanel.vue";
</script>

<template>
  <div class="shell">
    <main class="main">
      <HistoryPanel />
    </main>
  </div>
</template>
