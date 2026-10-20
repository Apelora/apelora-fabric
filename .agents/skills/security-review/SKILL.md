---
name: security-review
description: Review Apelora changes for security — authentication, authorization, secrets handling, input validation, and the Incus trust boundary. Use before merging code that touches the API, the deployment worker, the Incus adapter, imports, or secrets, and for threat-modeling new features.
---

# Security review

## Trust boundaries

Browser (Studio / Fabric Console) -> Apelora API -> deployment worker -> Incus adapter (mTLS) -> Incus cluster -> instances. Everything crossing a boundary is untrusted until validated. The Incus client certificate held by the backend is the highest-value secret: it must never reach the browser, logs or the repository (see `security-tenancy`).

## Review checklist

- **AuthN**: every route authenticated; tokens short-lived; Controller<->Node uses mutual TLS or equivalent, with node identity enrolled explicitly.
- **AuthZ**: object-level checks on every resource id (no IDOR); roles enforced server-side, never in Studio only.
- **Input**: assembly imports, manifests, names and paths validated; size/depth limits; no path traversal; no shell string-building; no injection into Incus instance config, profiles or `raw.*` keys.
- **Secrets**: never in manifests, assemblies, logs, errors, or domain configs on disk with loose permissions; stored encrypted; redacted in API responses.
- **Privilege**: tenant workloads are unprivileged containers in restricted Incus projects; instances cannot reach the Incus API or the management network (`fabric-networking`, `security-tenancy`); image sources verified by digest/signature.
- **Supply chain**: pinned dependencies, `cargo audit`/`npm audit` clean, lockfiles committed.
- **Web**: CSP, CSRF protection for cookie auth, sanitized SVG (catalog logos), no `dangerouslySetInnerHTML` with user data.
- **Audit**: security-relevant actions logged with actor and resource.
- **Enrolment**: a join token is returned once (`Cache-Control: no-store`), never stored, logged or audited, and reaches the server on standard input or a hidden prompt, never a command line; a new member stays held from placement until it is admitted after checks; BMC credentials stay in the provisioner (ADR 0002).

## Output

List findings by severity with file/line, exploit scenario, and a concrete fix. Report vulnerabilities privately per `SECURITY.md`, not in public issues.

## Automated secret check

`bash scripts/security/scan-secrets.sh` (also in CI) fails on private keys, Apelora API tokens (`apl_` + 64 hex) and Incus join tokens in tracked files, printing only file and line. If it fires on a pushed commit: rotate the secret first, then remove it from history.
