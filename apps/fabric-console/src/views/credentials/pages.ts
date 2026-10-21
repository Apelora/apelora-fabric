// SPDX-License-Identifier: AGPL-3.0-only
import type { WhoAmI } from "@apelora/api-client";

/** Pages about the signed-in person's own credentials, opened from the user menu (`#/account/…`). */
export const ACCOUNT_PAGES = [
  { id: "account/tokens", label: "Your tokens" },
  { id: "account/sign-ins", label: "Your sign-ins" },
] as const;

export type AccountPageId = (typeof ACCOUNT_PAGES)[number]["id"];
export const isAccountPage = (id: string): id is AccountPageId => ACCOUNT_PAGES.some((p) => p.id === id);

/**
 * How the caller signed in (`whoami.credential`): `identity-provider`, `session`,
 * `personal-token`, `service-token`, `api-token` or `development`. Empty from an older control
 * plane that does not say.
 */
export const credentialOf = (me: WhoAmI): string => (me as { credential?: string }).credential ?? "";
