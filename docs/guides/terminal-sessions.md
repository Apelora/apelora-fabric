# Terminal sessions into instances

This page shows how to open a terminal into a running module from the Fabric Console or the Studio. It also covers production approval, how administrators see and end sessions, and which browser origins to allow. You need a signed-in account with the `instance.console` permission. The rules behind it are in [Identity and authorization, section 7](../architecture/IDENTITY-AND-AUTHORIZATION.md#7-interactive-access-to-modules-terminal-powershell).

## Two doors, one broker

The Studio (right-click on a module, or the Monitoring tab) and the Fabric Console open the **same** sessions through the **same** API: a single-use ticket from `POST /api/v1/applications/{name}/instances/{instance}/sessions`, then a websocket to `/api/v1/sessions/connect`. The browser never talks to Incus. Whichever you use, the same rules apply:

- You need `instance.console` on the application: the **Instance Access** role, a project administrator, or an organisation owner. Automation identities and API tokens never get a session.
- In production, another person approves first. An approval works once, within 30 minutes, for that one instance.
- The ticket lasts 60 seconds, works once, and only from the page (origin) that asked for it.
- The shell runs as uid 1000 (`APELORA_CONSOLE_UID`), not root. It ends after 15 minutes without typing, and after 60 minutes in any case. Your access is checked again every 30 seconds; a removed role ends the session.
- The audit trail records who, which instance, when, why it ended and how many bytes moved. What you type or see is never recorded.

## Opening a terminal from the Fabric Console

1. Find the instance in one of two places:
   - **Instances** (needs `cluster.view` to see the list): **Terminal…** on any instance Apelora created for an application.
   - **Applications** (needs only `application.view`): **Show details**, then **Terminal…** on an instance. This is the door for people who hold Instance Access but not the infrastructure views.
2. The dialog asks the control plane what you may open. When nothing can be opened, it shows the reason: no permission, the instance is stopped, or Incus cannot be read.
3. Otherwise it lists the rules above and offers **Open terminal**. Choose it.

The terminal has the focus, and Escape goes to the program. Use **End session** to close it.

### Production: approval

1. The dialog shows **Approval needed**. Write why you need access and choose **Request approval**.
2. The request appears under Identity & Access → Access policies → Approvals for the people who may approve.
3. The dialog shows *Waiting for approval* and checks again every few seconds. You can also choose **Check now**.
4. Once it says *Approved*, choose **Open terminal**: it sends the approval with the request.

If you already have an approved, unused request for that instance, the dialog picks it up.

## The Sessions page

Identity & Access → **Sessions** (`#/sessions`) is offered to people who hold `instance.console`, `instance.desktop` or `application.operate` somewhere.

- **Open now** lists your own sessions and those on applications you operate: who, which instance, when it started, when they last typed, and how long it has left (the idle limit unless they type, or the absolute limit). It refreshes every 10 seconds.
- **End session** (after a confirmation) closes the terminal at once. The person sees *Ended by an administrator or from the sessions list*, and the audit trail records `session.terminate`. You may end your own sessions and any session on an application you may operate.
- **History from the audit trail** shows past `session.*` events (requested, opened, ended with the reason and duration, refused) for the scopes where you hold `audit.read`. It is read-only.

Sessions live in the control plane's memory. A restart ends them all, and with several control plane replicas, each lists only its own.

## Browser origins

The websocket accepts a browser page only from the control plane's own host name, or from an origin listed in `APELORA_ALLOWED_ORIGINS`. There is no wildcard.

- **Recommended:** serve each application on its own host name, and pass `/api/` to the control plane with the browser's `Host` kept (`infra/serve/apelora.conf`). Each page's origin is then the host the control plane sees, and nothing needs listing.
- **When the API is reached under another host name** (the proxy rewrites `Host`, or a load balancer sits in between), list both applications by origin:

  ```sh
  APELORA_ALLOWED_ORIGINS=https://studio.example.internal,https://console.example.internal
  ```

- **Development:** the Vite development servers of the Studio (`http://127.0.0.1:5173`) and the Console (`http://127.0.0.1:5174`) pass `/api` on (including websockets) with the browser's `Host` kept, so they work without configuration. To list them explicitly, as for a proxy that rewrites `Host`:

  ```sh
  APELORA_ALLOWED_ORIGINS=http://127.0.0.1:5173,http://127.0.0.1:5174
  ```

  Any other origin (`http://127.0.0.1:5175`, `http://localhost:5174`) is refused with `403 origin_not_allowed`.

Sessions need sign-in. A control plane in development mode (`DATABASE_URL=`) checks origins and tickets but never opens a shell.

## Content security policy

The terminal emulator writes `<style>` elements while it runs, so both sites set `$apelora_style_elements` to `'unsafe-inline'` in `infra/serve/apelora.conf`. Scripts stay limited to the site's own files.

## Troubleshooting

| You see | Why | What to do |
|---|---|---|
| *You do not have instance.console on this application* | No role with the permission on that application | Ask for Instance Access on its project or application |
| *Not allowed: …* after **Request approval** | You lack `approval.request` on that application | Ask for a role that includes it (Instance Access does) |
| *The instance is stopped…* | Sessions open only into running instances | Start the application |
| *The control plane cannot read Incus…* | Incus is unreachable, so the state is unknown | Try again; check Health |
| *You already have the most sessions allowed* | 3 sessions or unused tickets per person (`APELORA_CONSOLE_MAX_SESSIONS`) | End one on the Sessions page |
| The terminal never connects | The websocket was refused, usually by the origin check | Check that the proxy passes `Upgrade` and `Host`, or list the origin in `APELORA_ALLOWED_ORIGINS` |
| *Mock · no real instance* | The control plane runs with simulated data (the mock provider used in development) | The shell is a labelled fake; nothing runs on a server |

## Before you rely on it

- Terminal sessions have been tested against a simulated Incus, not yet against a real instance, a Windows guest or a live identity provider. Open one on a test application before you rely on it.
