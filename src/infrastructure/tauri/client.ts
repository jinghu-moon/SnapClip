import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { invoke, isTauri } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";

import { isEventEnvelope } from "../../shared/ipc";
import type { EventEnvelope } from "../../shared/ipc";

/**
 * The only module in the front-end that touches Tauri's IPC primitives.
 *
 * Every feature API goes through here, so replacing the native capture overlay
 * never reaches beyond this file.
 */
export async function call<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await invoke<T>(command, args);
  } catch (error) {
    throw normaliseError(error);
  }
}

/**
 * Subscribe to a versioned event.
 *
 * Envelopes that do not match the expected shape are dropped with a warning rather
 * than handed to a consumer that cannot interpret them.
 */
export async function subscribe<T>(
  event: string,
  handler: (payload: T) => void,
): Promise<UnlistenFn> {
  return listen<EventEnvelope<T>>(event, (message) => {
    if (!isEventEnvelope(message.payload)) {
      console.warn(`[snapclip] dropping ${event} without an event envelope`);
      return;
    }
    handler(message.payload.payload);
  });
}

/** Show the main window only after the first Vue render is ready. */
export async function showMainWindow(): Promise<void> {
  await getCurrentWindow().show();
}

export function isTauriRuntime(): boolean {
  return isTauri();
}

function normaliseError(error: unknown): Error {
  if (error instanceof Error) return error;
  if (typeof error === "object" && error !== null && "message" in error) {
    const message = (error as { message?: unknown }).message;
    if (typeof message === "string") return new Error(message);
  }
  return new Error(String(error));
}
