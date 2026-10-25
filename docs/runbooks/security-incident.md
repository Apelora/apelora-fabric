# A security incident: a stolen credential, a misused account or a session you did not expect

Use this page when a credential may be stolen, an account or session is misused, or a session
appears that you did not expect.

Order of work: **contain, keep the evidence, find out what was done, recover, learn.** Contain
first and quickly: everything below that contains is reversible, and everything is audited.

## 0. Know what Apelora can and cannot tell you

- Every change and every refusal is in the audit trail (append-only, written in the same
  transaction as the change). Sessions record who, which instance, when, from which address, why
  it ended and how many bytes moved.
- A terminal session's **content is not recorded**. You can learn that someone had a shell on an
  instance and for how long, not what they typed. Look at what the instance itself logged
  (*Instance → logs*, or on the server) and at what changed afterwards.
- Identity-provider tokens cannot be revoked by Apelora before they expire (at most an hour).
  Disable the **person** in Apelora to cut them off at once, and disable the account at the provider.
- Terminal tickets and running sessions live in the control plane's memory: restarting it ends
  them all.
- Legacy `apl_` tokens are platform-wide credentials and are exempt from approvals. If any exist,
  list them (*Administration*) and revoke what is not needed; set `APELORA_LEGACY_TOKENS=disabled`
  once everyone uses sign-in.

## 1. Contain

Pick what fits; do several.

| What is suspect | Do | Takes effect |
|---|---|---|
| A person's account | *Identity & Access → Users → Disable*. Their Apelora sessions are revoked with it. | Next request; a running terminal ends within 30 s (`access-revoked`) |
| A person's open terminal | *Identity & Access → Sessions → End*, or `DELETE /api/v1/sessions/{id}` | At once |
| Someone's role | Revoke the assignment (*Identity & Access → Project memberships*), or remove them from the group that grants it | Next request; a running terminal ends within 30 s |
| An API token | *Administration → Tokens → Revoke*, or `DELETE /api/v1/tokens/{id}` | Next request |
| A person's personal token (`apt_…`) | Their *Your tokens* page, or an administrator revoking it; it is evaluated live against their roles, so disabling the person also stops it | Next request |
| A service identity (automation) | *Service identities → Disable*, then rotate its token | Next request |
| An emergency-access grant | Revoke the assignment; grants end by themselves within 4 hours | Next request |
| A sign-in method | *Identity & Access → Sign-in*: turn off the identity provider, the directory, or local accounts. The server refuses a change that would lock everyone out. | At once |
| All terminal access | Set `APELORA_CONSOLE_ACCESS=disabled` and restart the control plane. This also ends every running session. | On restart |
| A person's pending approvals | *Approvals → Cancel/Reject* | At once |
| An Incus certificate (deploy or admin) | Rotate it: [certificates.md](certificates.md) | After rotation |
| A server | [member-removal.md](member-removal.md) or power it off and treat it as failed: [member-failure.md](member-failure.md) | — |

Keep at least one trusted super administrator enabled and able to sign in; Apelora refuses to
disable the last enabled one.

## 2. Keep the evidence

1. **Export the audit trail** for the period of concern: *Audit → Export* (CSV or JSON lines), or
   `GET /api/v1/audit/export?…`. The export is itself audited (`audit.exported`). Store it outside
   the control plane.
2. **Support bundle:** *Administration → Support bundle* (secrets are scrubbed; it is audited).
3. Note request ids from anything suspicious: they join the audit row, the log line, the job and
   the Incus operation ([troubleshooting.md](troubleshooting.md)).
4. Do not delete anything to "clean up". Apelora blocks `TRUNCATE` on the audit table; the same
   care applies to logs on the servers.

## 3. Find out what was done

Filter the audit trail by actor, then read in time order. The actions that matter most:

| Question | Look for |
|---|---|
| Did they get in? | `auth.oidc.signed_in`, `auth.password.succeeded` and `auth.password.failed`, `auth.directory.*`, `auth.locked`, `auth.refused` (a disabled account) |
| Did they try things they could not do? | `access.denied`, `session.denied`, refused approvals |
| Did they give themselves or others more access? | `role.assign`, `role.revoke`, `role.create`, `group.member.add`, `group.member.remove`, `emergency_access.request` and `emergency_access.grant`, `token.create` (and `token.create_admin`), `personal_token.create`, `service_identity.create` |
| Did they get a shell? | `session.request`, `session.open`, `session.close` (instance, source address, duration) |
| Did they change or remove workloads? | `application.deploy.requested`, `application.operation`, `application.delete`, `application.volume.delete`, `application.volume.restore`, `instance.create`/`start`/`stop`/`delete`/`move`, `cluster.member.evacuate`, `cluster.member.remove` |
| Did they read data out? | `audit.exported`, support bundle downloads |
| Did they change security settings? | sign-in provider, directory, policy and approval-rule changes |

For each terminal session, the instance's own logs and the changes that follow it are what shows
the effect. If the instance may have been altered, rebuild it from the design rather than trusting
it: redeploy, and restore its volumes from a backup taken before the incident
([storage-incident.md](storage-incident.md)).

## 4. Recover

1. Rotate what the person or script could reach: service tokens, their personal tokens, the
   Incus certificates if the control plane host was involved ([certificates.md](certificates.md)),
   the identity provider's client secret and any webhook secrets (the alert receiver's signing
   secret lives on the workers).
2. If the control plane host itself is suspect, restore onto a clean host from Apelora's database
   backup and re-issue every credential ([apelora-database-backup.md](apelora-database-backup.md));
   the old host's secrets must be treated as lost.
3. Re-enable what you turned off one thing at a time, and watch the audit trail as you do.
4. Make sure a person who needs access again gets it through a normal assignment, with the
   approval that production asks for, not a shortcut.

## 5. Learn

- Write what happened, when, how it was found and how long containment took.
- Close the gap: a shorter session limit (`APELORA_CONSOLE_IDLE_SECS`, `APELORA_CONSOLE_MAX_SECS`),
  an approval rule or MFA requirement under *Access policies*, a narrower role, fewer legacy
  tokens.
- If this runbook missed a step or got one wrong, correct it.

## Before you rely on it

- Each control on this page is tested on its own, but the procedure as a whole has not been
  rehearsed, and not with an identity provider, a directory or MAAS involved. Rehearse it once with
  your team before you need it.
