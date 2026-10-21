# Signing in to the Fabric Console and the Studio

How people sign in to Apelora, how an administrator sets up sign-in with the organisation's identity provider, and what the messages on the sign-in page mean. Design and security details are in [Identity and authorization, section 3](../architecture/IDENTITY-AND-AUTHORIZATION.md#3-sign-in).

The screenshots on this page were taken against a stubbed control plane (`tests/e2e/screenshots.spec.ts`); the providers and addresses in them are examples.

## Which way applies to you

| The control plane runs… | Fabric Console | Studio |
|---|---|---|
| without a database (development mode) | No sign-in; the Overview shows *Authentication: Disabled (development only)* | Connect with any text as the token (nothing is checked) |
| with a database (normal) | Sign-in page: your organisation's identity provider, or an API token | *Connect to a control plane*: your identity provider, or an access token |

## 1. Development mode: no sign-in

When the control plane starts without a database, authentication is off and it only listens on the loopback address. It logs `DATABASE_URL is not set: authentication is DISABLED (development only, loopback)`.

Inside this repository, `.cargo/config.toml` sets `DATABASE_URL` to the local development database for every `cargo` command, so a plain `cargo run` turns authentication **on**. For development mode, set it to empty:

```sh
DATABASE_URL= cargo run -p apelora-control-plane   # API at http://127.0.0.1:8080, mock data
npm run dev:console                                # Fabric Console at http://127.0.0.1:5174
```

The Console opens without a sign-in page. Never use this mode on a shared machine or network; the control plane refuses to start without a database on any address other than loopback.

## 2. Signing in with an API token

An API token is a long random secret that an administrator creates on the server. It is the way in until an identity provider is set up, and it stays useful for a first administrator.

Create one (shown **once**; copy it straight into the sign-in page):

```sh
DATABASE_URL=… cargo run -p apelora-control-plane -- token create --name me --role fabric-admin
```

- Roles: `fabric-admin`, `operator`, `studio-designer`, `auditor`. A `fabric-admin` token acts as a platform super admin, including Identity & Access.
- `--expires-days <d>` sets an expiry; `token list` and `token revoke <id>` manage tokens.
- With the local development database (`docker compose -f infra/dev/compose.yaml up -d`), `.cargo/config.toml` already supplies `DATABASE_URL`.

In the Console, paste the token into **API token** and choose **Sign in**. If an identity provider is set up, choose **Use an API token instead** first. The token is kept only for this browser tab (session storage): closing the tab signs you out, and a new tab asks again.

<p align="center">
  <img src="../screenshots/23-console-sign-in-token-not-accepted-1440x900.png" alt="Fabric Console sign-in page with an API token field, marked in red, under the message: That token was not accepted. It may have expired or been revoked." width="720">
</p>

*A token that was mistyped, revoked or expired is refused with this message.*

## 3. Signing in with your organisation's identity provider

Once an administrator has added your organisation's identity provider (section 4), the sign-in page offers it: with one provider a **Continue with Example Corp** button, with several a row of buttons named after each provider under *Sign in with your organisation*. Choose it, sign in at your provider (with multi-factor if it asks), and you come back to the page you were on.

<p align="center">
  <img src="../screenshots/22-console-sign-in-1440x900.png" alt="Fabric Console sign-in page: an illustration of connected servers on the left, and on the right a card with the logo, a Continue with Example Corp button and a Use an API token instead link" width="720">
</p>

<p align="center">
  <img src="../screenshots/26-console-sign-in-phone-390x844.png" alt="The same sign-in page on a phone, after Use an API token instead: the provider button above an API token field" width="260">
</p>

- The browser uses the authorization code flow with PKCE, `state` and `nonce`; Apelora never sees your password.
- The access token is kept only for this browser tab, like an API token.
- Your account is created at your first sign-in, **without any role**. An administrator then gives you a role (Identity & Access → Users → *Assign role…*, or Project memberships). Until then you see little or nothing.

### The Studio

The Studio starts with designs kept in the browser; the header chip says *This browser · not connected*. To work with the control plane, choose that chip. *Connect to a control plane* offers the same identity providers as the Console (**Continue with …**); after signing in at the provider you come back to the Studio page you were on. Or choose *Use an access token instead* (or, without a provider, paste one straight away) and **Connect**. The token stays in that tab only; *Sign out* in the same dialog forgets it. If Apelora refuses the provider's token, the chip's tooltip says why. The designer's deploy dialog also asks for an API token when the control plane refuses the request.

## 4. For administrators: setting up sign-in

You need the permission `platform.security.manage` (the *Platform Super Admin* role, or a `fabric-admin` API token). Open **Identity & Access → Sign-in**. Changes apply at once, without a restart, and are recorded in the audit history.

<p align="center">
  <img src="../screenshots/24-console-sign-in-settings-1440x900.png" alt="Identity & Access, Sign-in: three providers. Example Corp answers; Contractors is not reachable (discovery failed: connection refused); Old directory is off and has no button. Below, the API tokens section with Turn off API tokens." width="900">
</p>

Each provider shows its state: **Answers**, **Not reachable** (with the error), or **Off**. *Sign-in button shown* means it has a browser client ID; *Tokens only, no button* means its tokens are accepted (for example from scripts) but the sign-in page shows no button for it. A provider set by environment variables (`APELORA_OIDC_*`, see `.env.example`) appears with *set by environment variables* and cannot be changed here.

### Adding an identity provider

Choose **Add identity provider**. Any OpenID Connect provider works.

<p align="center">
  <img src="../screenshots/25-console-identity-provider-dialog-1440x900.png" alt="Add an identity provider dialog: name Example Corp, issuer URL with Check connection, a green note The provider answers (2 signing keys), audience apelora-api, browser client ID apelora-console, and the open Claims and multi-factor rules section" width="900">
</p>

| Field | What to enter |
|---|---|
| Name on the sign-in button | Shown as *Continue with …* (or the name alone when there are several providers) |
| Issuer URL | The provider's issuer, exactly as in its discovery document (`<issuer>/.well-known/openid-configuration`). `https://` only; plain `http://` only for `127.0.0.1` or `localhost` |
| Audience | The name tokens must be issued for (`aud`); several separated by commas |
| Browser client ID (optional) | A **public** client at the provider (no client secret) that uses authorization code with PKCE. Without it there is no sign-in button |
| Scopes | Default `openid profile email` |
| Groups claim, Username claim | Where the token carries the person's groups and user name (defaults `groups`, `preferred_username`) |
| Methods / assurance levels that count as multi-factor | `amr` values (default `mfa, otp, hwk`) and `acr` values; access policies can require multi-factor |
| Clients a token must be issued to (`azp`) | Optional; refuses tokens issued to other clients |

At the provider, for the browser client:

- **Redirect addresses:** the Console's address with a trailing slash, as shown under the field, for example `https://console.example.org/` (locally `http://127.0.0.1:5174/`), and the Studio's root address, for example `https://studio.example.org/` (locally `http://127.0.0.1:5173/`).
- **Cross-origin requests:** allow the Console's and the Studio's origins. The browser fetches the discovery document and exchanges the code at the token endpoint itself.
- **Token lifetime:** at most **one hour**. Apelora refuses longer-lived tokens.

**Check connection** reads the provider's discovery document and signing keys before you save. *The provider answers* names the number of keys and where sign-in goes; otherwise the dialog shows the error. **Add provider** saves it.

**Edit…** changes a provider, **Turn off** stops accepting it (and removes its button), **Remove…** deletes it after you type its name. People's accounts and roles in Apelora stay.

### The first administrator who signs in with a provider

1. Sign in to the Console with a `fabric-admin` API token and add the provider.
2. In a new tab, sign in with the provider. Your account now exists, without roles.
3. In the first tab, open Identity & Access → Users, choose *Assign role…* for that account and give it *Platform Super Admin* at platform level.

Alternatively, on the server: `DATABASE_URL=… cargo run -p apelora-control-plane -- identity bootstrap --issuer <issuer> --subject <sub> [--username <u>]` grants Platform Super Admin to the person with that `sub` claim, only while no platform super admin exists.

### Turning API tokens off

Once people sign in with a provider, turn API tokens off under **API tokens → Turn off API tokens…**. Every API token stops working at once; automation uses a service identity instead. The server refuses changes that would lock everyone out and changes nothing:

| You try to… | The server answers |
|---|---|
| turn API tokens off while signed in with one | *you signed in with an API token: sign in with your identity provider first, then turn API tokens off* |
| turn API tokens off while no provider answers | *nothing was changed: no identity provider that answers is enabled, so nobody could sign in* |
| turn off or remove the last answering provider while API tokens are off | *nothing was changed: API tokens are off and no other identity provider answers, so nobody could sign in* |
| turn API tokens on when the environment turned them off | *API tokens are turned off by APELORA_LEGACY_TOKENS=disabled; change it there* |

`APELORA_LEGACY_TOKENS=disabled` in the control plane's environment turns API tokens off for good; the Console then shows the setting without a button.

## 5. Trying it locally

The repository includes a development identity provider (`infra/dev/dex.yaml`). Its user and password are for development only.

```sh
docker compose -f infra/dev/compose.yaml --profile oidc up -d   # PostgreSQL and the identity provider on 127.0.0.1:5556
cargo run -p apelora-control-plane                            # authentication on (database from .cargo/config.toml)
cargo run -p apelora-control-plane -- token create --name me --role fabric-admin
npm run dev:console
```

1. Open **http://127.0.0.1:5174** (not `localhost`: the provider only knows that address) and sign in with the token.
2. Identity & Access → Sign-in → **Add identity provider**: issuer `http://127.0.0.1:5556/dex`, audience `apelora-console`, browser client ID `apelora-console`. **Check connection**, then **Add provider**.
3. In a new tab, open the Console, choose **Continue with …** and sign in as `dev@apelora.test` with password `apelora-dev-only`.
4. Give that account a role from the first tab (section 4).

Stop it with `docker compose -f infra/dev/compose.yaml --profile oidc down`.

## 6. Troubleshooting

### Messages on the sign-in page

| Message | Meaning and what to do |
|---|---|
| That token was not accepted. It may have expired or been revoked. | The API token is wrong, revoked or expired. Ask for a new one |
| Your session ended. Sign in again. | The token you were using stopped working (expired, revoked, or API tokens were turned off) |
| You signed in, but Apelora did not accept the identity provider's token: … | The provider signed you in, but the control plane refused its token. The reason follows (below) |
| … the token lives longer than Apelora accepts | The provider issues tokens valid for more than one hour. Shorten the token lifetime at the provider |
| … the token has expired / is not valid yet | Usually clock drift between the provider and the control plane |
| … the token is not meant for this API | The token's audience is not in the provider's *Audience* field |
| … the token was issued to a client that is not allowed | The token's `azp` is not in *Clients a token must be issued to* |
| … no identity provider set up here issues tokens for … | The token's issuer differs from every enabled provider's *Issuer URL* (check trailing slashes and paths) |
| … this account has been disabled | An administrator disabled your account in Identity & Access → Users |
| … the identity provider … cannot be reached: … | The control plane cannot read that provider's signing keys; see *Not reachable* below |
| This sign-in was not started in this tab. Sign in again. | The provider sent the browser back to a tab that did not start the sign-in (or the tab's storage was cleared). Start again from the button |
| The identity provider did not sign you in: … | The provider reported an error, for example you cancelled |
| The identity provider did not answer (…) / names a different issuer than this control plane expects | The browser could not read the provider's discovery document, or it names another issuer; check the issuer URL and cross-origin settings |
| No way to sign in is turned on for this control plane. | No provider has a browser client ID and API tokens are off. An administrator must fix it on the server |

### A provider shows *Not reachable*

The control plane could not read the provider's discovery document or keys; its tokens are refused until it answers. The error is shown in the list, and the control plane logs `identity provider not reachable: its tokens are refused until it answers (retried every minute)`. It retries every minute and logs `identity provider accepted for sign-in` once it answers. Check the issuer URL, DNS, TLS and firewalls between the control plane and the provider.

### Server log: `credential refused`

Every refused token is logged at `warn` level as `credential refused`, never with the token itself. The fields:

| Field | Content |
|---|---|
| `method`, `path` | The request that was refused |
| `credential` | `identity-provider token`, `service token`, `API token` or `unrecognised` |
| `issuer` | The token's issuer for identity-provider tokens, otherwise `-` |
| `code`, `reason` | The error code (for example `invalid_token`) and the reason in words, the same as on the sign-in page |

Refusals are not written to the audit history (anyone could fill it with bad requests). A disabled account that signs in is recorded there as `auth.refused`. For more detail, run the control plane with `RUST_LOG=apelora_control_plane=debug,info`.
