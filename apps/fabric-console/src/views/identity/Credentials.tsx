// SPDX-License-Identifier: AGPL-3.0-only
import type { WhoAmI } from "@apelora/api-client";

import { Heading } from "../../components/ui";
import { holdsPlatform } from "../../lib/identity";
import { PeopleSignInsSection } from "../credentials/SignIns";
import { PeopleTokensSection } from "../credentials/Tokens";

/**
 * Everyone's sign-ins (`user.manage`) and personal tokens (`token.manage`), with sign out and
 * revoke (`#/identity/credentials`). A person's own are under the user menu.
 */
export function CredentialsView({ me, onAuth }: { me: WhoAmI; onAuth: (m: string) => void }) {
  return (
    <div className="flex flex-col gap-8">
      <div className="flex flex-col gap-2">
        <Heading level={1}>Sign-ins &amp; tokens</Heading>
        <p className="max-w-2xl text-sm text-base-content/70">Who is signed in, and the tokens people made. Yours are in the menu under your name.</p>
      </div>
      {holdsPlatform(me, "user.manage") && <PeopleSignInsSection onAuth={onAuth} />}
      {holdsPlatform(me, "token.manage") && <PeopleTokensSection onAuth={onAuth} />}
    </div>
  );
}
