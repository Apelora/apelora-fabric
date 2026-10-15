---
name: mcp-security
description: Decide whether and how to add or change an MCP server or other agent tool in Apelora Fabric — hosting location, permissions, secrets, supply chain. Use before editing .mcp.json, installing a skill, or exposing any capability to an AI agent. Xen management is never exposed through an unrestricted MCP.
---

# MCP security

## Decision checklist

1. **Need:** can Claude Code's built-in tools do it? If yes, do not add a server.
2. **Location:** developer workstation only (stdio or 127.0.0.1). Never on dom0, a hypervisor host, or a network-reachable port.
3. **Data egress:** does the server send repository content or queries to a third party? Record it in `docs/development/MCP-HOSTING.md`; do not send proprietary code.
4. **Permissions:** least privilege; read-only first; OAuth over long-lived tokens; scope GitHub access to this repo.
5. **Secrets:** none in `.mcp.json` or the repo; env/keyring only; check `git diff` before committing.
6. **Supply chain:** pin exact versions (no `@latest` after bootstrap); review changelogs and the package before updating; review third-party skills (`.agents/skills/*`) as they run with agent permissions.
7. **Xen/controller:** real host operations stay behind the authenticated, audited Rust controller. A future Apelora dev MCP may only list catalog entries, validate assemblies and plan dry-runs against **mock** hosts.

## Verification

`claude mcp list`, `claude mcp get <name>`, review of `.mcp.json` diff, and an entry in `MCP-HOSTING.md` for every server.
