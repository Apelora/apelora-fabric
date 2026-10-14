---
name: security-review
description: Review Apelora changes for security — authentication, authorization, secrets handling, input validation, and hypervisor privilege boundaries. Use before merging code that touches the API, Node agent, imports, or secrets, and for threat-modeling new features.
---

# Security review

## Trust boundaries

Browser (Studio) -> Controller API -> Node agent (dom0) -> Xen -> guests. Everything crossing a boundary is untrusted until validated. The Node agent in dom0 is the highest-value target.

## Review checklist

- **AuthN**: every route authenticated; tokens short-lived; Controller<->Node uses mutual TLS or equivalent, with node identity enrolled explicitly.
- **AuthZ**: object-level checks on every resource id (no IDOR); roles enforced server-side, never in Studio only.
- **Input**: assembly imports, manifests, names and paths validated; size/depth limits; no path traversal; no shell string-building; no template injection into Xen domain configs.
- **Secrets**: never in manifests, assemblies, logs, errors, or domain configs on disk with loose permissions; stored encrypted; redacted in API responses.
- **Privilege**: Node runs least-privilege; guests cannot reach dom0 or the management network (`fabric-networking`); image sources verified by digest/signature.
- **Supply chain**: pinned dependencies, `cargo audit`/`npm audit` clean, lockfiles committed.
- **Web**: CSP, CSRF protection for cookie auth, sanitized SVG (catalog logos), no `dangerouslySetInnerHTML` with user data.
- **Audit**: security-relevant actions logged with actor and resource.

## Output

List findings by severity with file/line, exploit scenario, and a concrete fix. Report vulnerabilities privately per `SECURITY.md`, not in public issues.
