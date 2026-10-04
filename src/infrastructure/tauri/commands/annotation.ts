import { call } from "../client";

import type { AnnotationCommand } from "../../../shared/contracts";

/**
 * Forward one coarse toolbar command to the native annotation document.
 *
 * Low frequency by design: a single call per toolbar click. The overlay drains the
 * command on its own render cadence, so this never sits on a mouse or pixel path.
 */
export function sendAnnotationCommand(command: AnnotationCommand): Promise<void> {
  return call<void>("capture_annotation", { command });
}
