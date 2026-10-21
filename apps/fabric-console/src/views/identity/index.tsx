import type { WhoAmI } from "@apelora/api-client";

import { SessionsView } from "../Sessions";
import { CredentialsView } from "./Credentials";
import { GroupsView } from "./Groups";
import { IdentityAuditView } from "./IdentityAudit";
import { MembershipsView } from "./Memberships";
import { OrganizationsView } from "./Organizations";
import type { IdentityPageId } from "./pages";
import { PoliciesView } from "./Policies";
import { RolesView } from "./Roles";
import { ServiceIdentitiesView } from "./ServiceIdentities";
import { SignInSettingsView } from "./SignInSettings";
import { TenancyView } from "./Tenancy";
import { UsersView } from "./Users";

/** The page for one Identity & Access route. */
export function IdentityPage({ id, me, onAuth }: { id: IdentityPageId; me: WhoAmI; onAuth: (m: string) => void }) {
  switch (id) {
    case "identity/users": return <UsersView me={me} onAuth={onAuth} />;
    case "identity/groups": return <GroupsView me={me} onAuth={onAuth} />;
    case "identity/organizations": return <OrganizationsView me={me} onAuth={onAuth} />;
    case "identity/tenancy": return <TenancyView onAuth={onAuth} />;
    case "identity/memberships": return <MembershipsView me={me} onAuth={onAuth} />;
    case "identity/roles": return <RolesView me={me} onAuth={onAuth} />;
    case "identity/services": return <ServiceIdentitiesView onAuth={onAuth} />;
    case "identity/sign-in": return <SignInSettingsView onAuth={onAuth} />;
    case "identity/credentials": return <CredentialsView me={me} onAuth={onAuth} />;
    case "identity/policies": return <PoliciesView me={me} onAuth={onAuth} />;
    case "identity/audit": return <IdentityAuditView me={me} onAuth={onAuth} />;
    case "sessions": return <SessionsView me={me} onAuth={onAuth} />;
  }
}
