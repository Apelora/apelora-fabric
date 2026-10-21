---
name: secure-remote-access
description: Guardrails for giving people a terminal, PowerShell or RDP session into a running module from Apelora. Use before changing or reviewing any interactive access to instances (right-click "Open terminal", exec/console endpoints, session tickets). Terminal (Linux) and PowerShell (Windows, unverified) are built; RDP is not.
---

# Secure remote access to modules

**Built (MOCK VERIFIED and CONTRACT VERIFIED, never run on a real Incus):** brokered terminal and PowerShell sessions, `instance.console`/`instance.desktop`, the **Instance Access** role, production approval, tickets, limits, audit, Studio right-click and Monitoring entries, and a second door in the Fabric Console (Terminal… on Instances and Applications, a Sessions page with "End session" and history from the audit trail). Code: `services/control-plane/src/api/sessions.rs`, `crates/incus-adapter/src/shell.rs`, `crates/domain/src/access.rs`, the shared browser side `packages/terminal` (`@apelora/terminal`: protocol, close reasons, the xterm `TerminalScreen`), `apps/studio/src/components/sessions/`, `apps/fabric-console/src/components/TerminalDialog.tsx` (+ `TerminalSession.tsx`), `apps/fabric-console/src/views/Sessions.tsx`. Reference: `docs/architecture/IDENTITY-AND-AUTHORIZATION.md` section 7; operators: `docs/guides/terminal-sessions.md`.
**Not built:** the RDP gateway (the API reports it unavailable and says why), SSH daemons (deliberately never), session recording. Anything below must keep meeting this contract.

## Architecture

- **Brokered by the control plane. Never direct.** The browser talks only to the Apelora API. The API opens the session to the instance through Incus (exec / console websocket for Linux containers and VMs; the VM serial/VGA console for Windows). The browser never receives an Incus address, certificate or credential, and instances never get a route to the Incus API (see `security-tenancy`).
- **Linux modules**: terminal in the browser (xterm.js) over an authenticated websocket to the API, which attaches to an Incus exec session as an unprivileged user chosen by policy (not root by default). No SSH daemon is added to modules for this; if SSH is needed it is an explicit module property, key-only.
- **Windows modules**: PowerShell through the same brokered exec path where the guest agent supports it; **RDP through a gateway the control plane runs** (WebSocket/HTTP tunnel to the instance's RDP port, TLS, NLA required). Never publish 3389 on a tenant network or the internet. RDP credentials are never stored by Apelora and never logged; the user types them into the session or a secret reference is resolved server-side.
- **Hypervisor-neutral**: put "how to open a console" behind a provider interface (Incus now; other runtimes later) in `platforms.ts`-style provider facts, not in the assembly model.

## Authorization (ACLs)

1. New permissions in the catalogue, class **Tenant**, floor **Application**: `instance.console` (terminal/PowerShell) and `instance.desktop` (RDP). Not included in the viewer, designer or auditor roles. Granted to operator and above, or to a custom role.
2. Production environments require **approval** (`approval: true` or a policy by environment kind) and may require recent MFA through an access policy.
3. Check the exact instance: caller's scope on its application, the instance belongs to that application, the instance is `managed` and running. Anything else answers 404.
4. **Service identities and legacy tokens never get interactive sessions.**
5. Deny when Incus is unobservable (stale/unknown status).

## Session controls

- Short-lived, single-use session ticket issued by an authenticated POST (CSRF-safe), bound to user, instance and origin; the websocket upgrade checks Origin and the ticket. Tickets are never in URLs that get logged.
- **Origins:** the API's own host (each application served on its own host name with `Host` kept by the proxy), or an origin listed in `APELORA_ALLOWED_ORIGINS` (Studio and Console by name, never a wildcard: `*` is refused at startup). The Studio and the Console are different origins (5173 and 5174 in development); a ticket issued to one does not work from the other. Guarded by `the_console_origin_is_configured_explicitly_and_a_third_origin_is_refused` and `the_studio_and_the_console_are_listed_explicitly_and_a_third_origin_is_refused`.
- **One browser implementation:** both applications use `@apelora/terminal`; it opens the websocket on the next task so React's development double mount never spends the single-use ticket on a socket it closes at once. Do not add a second socket or protocol implementation.
- Idle timeout, absolute maximum duration, concurrent-session cap per user, and an admin "terminate session" action.
- Every session is audited: who, which instance, start, end, outcome, source address, approval id. **Do not record keystrokes or screen content by default**; if recording is later required, it is a separate, explicit, access-controlled feature with retention and a visible indicator to the user.
- Rate-limit session creation; limit message size and rate on the websocket; no unbounded buffering.
- Output is rendered as terminal text only — never as HTML.

## UI rules

- Right-click on a module (`features/editor/menus.ts`): show "Open terminal" / "Open PowerShell" (Linux/Windows) and "Remote desktop (RDP)" for Windows classes only in the **runtime** view of a deployed application, never on the design canvas of an undeployed draft. Disabled with a reason (no permission, not running, status unknown, approval needed). Approval-gated actions say who must approve.
- The same actions appear on the instance row in the Monitoring tab.
- Fabric Console: **Terminal…** on Apelora instances of an application (Instances list for `instance.console` holders; Applications → details, which needs only `application.view`). The dialog asks `GET /applications/{name}/access` and shows the server's reason; in production it requests the approval, follows its state (`GET /approvals`, every 5 s while pending) and opens with the approved id. The Sessions page (`#/sessions`, Identity & Access) lists `GET /sessions` (last input, time left) with **End session** (confirmation) and reads history from `GET /identity/audit?action=session.`. A detail page mounts `TerminalDialog` with `{ application, instance }`.
- Windows detection comes from the module class manifest, not from the name.

## Tests required

Allowed; denied by scope; other tenant's instance → 404; viewer/auditor/service identity/legacy token refused; production without approval refused; stopped or unobservable instance refused; ticket reuse and expiry; wrong Origin rejected; idle and max-duration cut-off; audit rows written for success and denial; no credential or ticket in logs; axe on the dialogs. State plainly whether this ran against real Incus or a mock.
