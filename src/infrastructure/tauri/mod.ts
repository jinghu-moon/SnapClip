/**
 * Tauri access layer.
 *
 * `client` is the only module that imports `@tauri-apps/api`; `commands/*` and
 * `events/*` expose typed wrappers for the features to consume.
 */

export { call, subscribe } from "./client";
