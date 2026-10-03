/**
 * Application bootstrap: Pinia and mount.
 *
 * Back-end event subscriptions live with the feature that consumes them (each
 * feature exposes an `events.ts`), so nothing here needs to know the transport.
 */

import { createApp } from "vue";
import { createPinia } from "pinia";

import App from "./App.vue";
import "../styles.css";
import { isTauriRuntime, showMainWindow } from "../infrastructure/tauri/client";

export function bootstrap() {
  const startedAt = performance.now();
  const root = document.querySelector("#app");
  if (!root) {
    throw new Error("SnapClip mount point #app was not found");
  }

  const app = createApp(App);
  console.info(`[snapclip][frontend] Vue app created elapsed_ms=${Math.round(performance.now() - startedAt)}`);
  app.use(createPinia());
  app.config.errorHandler = (error, _instance, info) => {
    console.error(`[snapclip] Vue startup error (${info})`, error);
    root.innerHTML = `<div class="status status--error">SnapClip 加载失败，请查看开发者工具控制台。</div>`;
  };
  app.mount(root);
  performance.mark("snapclip-app-mounted");
  console.info(`[snapclip][frontend] Vue app mounted elapsed_ms=${Math.round(performance.now() - startedAt)}`);
  if (isTauriRuntime()) {
    void showMainWindow().catch((error) => {
      console.error("[snapclip][frontend] failed to show main window", error);
    });
  }
}
