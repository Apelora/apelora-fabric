import type { WhoAmI } from "@apelora/api-client";

import { AuditExplorer } from "../Audit";

/** The audit trail inside Identity & Access: the same view, filters and export as the Audit log. */
export function IdentityAuditView({ me, onAuth }: { me: WhoAmI | null; onAuth: (m: string) => void }) {
  return (
    <AuditExplorer
      me={me}
      onAuth={onAuth}
      variant={{
        path: "identity/audit",
        title: "Audit history",
        intro: "Changes to access, and every refusal, newest first, in the scopes you may audit.",
        op: "identity_audit",
        testId: "identity-audit",
      }}
    />
  );
}
