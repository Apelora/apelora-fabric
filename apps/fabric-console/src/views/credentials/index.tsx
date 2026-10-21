// SPDX-License-Identifier: AGPL-3.0-only
import type { WhoAmI } from "@apelora/api-client";

import type { AccountPageId } from "./pages";
import { YourSignInsView } from "./SignIns";
import { YourTokensView } from "./Tokens";

/** The page for one `#/account/…` route (opened from the user menu). */
export function AccountPage({ id, me, onAuth }: { id: AccountPageId; me: WhoAmI; onAuth: (m: string) => void }) {
  switch (id) {
    case "account/tokens": return <YourTokensView me={me} onAuth={onAuth} />;
    case "account/sign-ins": return <YourSignInsView me={me} onAuth={onAuth} />;
  }
}
