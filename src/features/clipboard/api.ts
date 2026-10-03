import { copyPayload } from "../../infrastructure/tauri/commands/clipboard";

import type { PayloadKind } from "../../shared/contracts";

/**
 * Copy a history payload back to the Windows clipboard.
 *
 * The clipboard side effects live in the clipboard feature on purpose: the capture
 * feature never routes through here.
 */
export function copyToClipboard(contentHash: string, kind: PayloadKind): Promise<void> {
  return copyPayload(contentHash, kind);
}
