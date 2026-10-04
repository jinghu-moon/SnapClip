import { sendAnnotationCommand } from "../../infrastructure/tauri/commands/annotation";

import type { AnnotationKind, Rgba } from "../../shared/contracts";

/**
 * Annotation toolbar use cases.
 *
 * Each function maps one user intent to a single coarse [`AnnotationCommand`] and
 * forwards it to the overlay. The document applies it on the overlay thread, so the
 * front-end owns no annotation state — it is a pure command producer (docs/12 Phase 5).
 */

/** Switch to the select / move / resize tool. */
export function selectAnnotationTool(): Promise<void> {
  return sendAnnotationCommand("selectTool");
}

/** Switch to a drawing tool. */
export function pickAnnotationTool(kind: AnnotationKind): Promise<void> {
  return sendAnnotationCommand({ tool: kind });
}

/** Stroke colour of the selection, else the pending default. */
export function setAnnotationStrokeColor(rgba: Rgba): Promise<void> {
  return sendAnnotationCommand({ setStrokeColor: rgba });
}

/** Fill colour of the selection, else the pending default; `null` disables fill. */
export function setAnnotationFillColor(rgba: Rgba | null): Promise<void> {
  return sendAnnotationCommand({ setFillColor: rgba });
}

/** Stroke width in physical pixels of the selection, else the pending default. */
export function setAnnotationStrokeWidth(width: number): Promise<void> {
  return sendAnnotationCommand({ setStrokeWidth: width });
}

/** Step back one committed edit. */
export function undoAnnotation(): Promise<void> {
  return sendAnnotationCommand("undo");
}

/** Step forward one undone edit. */
export function redoAnnotation(): Promise<void> {
  return sendAnnotationCommand("redo");
}

/** Delete the selected object. */
export function deleteSelectedAnnotation(): Promise<void> {
  return sendAnnotationCommand("delete");
}

/** Duplicate the selected object. */
export function duplicateSelectedAnnotation(): Promise<void> {
  return sendAnnotationCommand("duplicate");
}

/** Move the selected object one layer up the draw stack. */
export function bringAnnotationForward(): Promise<void> {
  return sendAnnotationCommand("bringForward");
}

/** Move the selected object one layer down the draw stack. */
export function sendAnnotationBackward(): Promise<void> {
  return sendAnnotationCommand("sendBackward");
}
