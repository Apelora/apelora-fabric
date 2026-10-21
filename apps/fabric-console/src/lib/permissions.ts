import type { Permission, WhoAmI } from "@apelora/api-client";

import { holds } from "./identity";

/**
 * Holds `permission` at some scope, or (a legacy token, or an older control plane that does not
 * report capabilities) the legacy permission that stands for it. The UI only avoids dead ends;
 * the server decides every request.
 */
export const can = (me: WhoAmI | null | undefined, permission: string, legacy: Permission): boolean =>
  !!me && (holds(me, permission) || ((me.capabilities ?? []).length === 0 && me.permissions.includes(legacy)));
