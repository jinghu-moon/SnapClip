import { call } from "../client";

import type { PayloadKind } from "../../../shared/contracts";

/** Copy a history payload back to the Windows clipboard. */
export function copyPayload(contentHash: string, kind: PayloadKind): Promise<void> {
  return call<void>("copy_payload", { contentHash, kind });
}