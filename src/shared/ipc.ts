/**
 * Versioned IPC envelope shared by every Tauri event.
 *
 * The back-end never sends image bytes through an event: it sends ids, sizes and
 * states, and the front-end fetches payloads through a command when it needs pixels.
 */
export const IPC_SCHEMA_VERSION = 3 as const;

export interface EventEnvelope<T> {
  schemaVersion: number;
  generation: number;
  payload: T;
}

/** Type guard for envelopes arriving from the back-end. */
export function isEventEnvelope(value: unknown): value is EventEnvelope<unknown> {
  if (typeof value !== "object" || value === null) return false;
  const candidate = value as Partial<EventEnvelope<unknown>>;
  return (
    typeof candidate.schemaVersion === "number" &&
    typeof candidate.generation === "number" &&
    "payload" in candidate
  );
}
