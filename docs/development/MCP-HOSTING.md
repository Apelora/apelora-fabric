# MCP and tooling: what runs where

MCP servers give Claude Code tools. They are **developer-workstation helpers**, not part of the product and not services the Apelora cluster runs.

| Tool | Where | Status | Notes |
|---|---|---|---|
| Playwright MCP (`@playwright/mcp`, pinned 0.0.83) | Local stdio, launched on demand | Configured in `.mcp.json` (Chromium, headless) | Needs Chromium: `npx playwright install chromium`. Drives the Vite dev server on 127.0.0.1 only. Restart Claude Code after changing `.mcp.json`. |
| Playwright Test (`@playwright/test`) | Local / CI | Used for the repeatable suite in `tests/e2e` | Same browser as above. |
| daisyUI skill | `.claude/skills/daisyui` (installed with `npx skills add saadeghi/daisyui`) | Installed | Docs only, no server. Review before upgrading; it is third-party instructions. |
| Context7 MCP (pinned 4.4.1) | Local process calling a hosted docs service | Optional, configured | Library-doc queries leave the machine; do not include proprietary code in queries. |
| Inspo / Figma MCP | User-level (not in `.mcp.json`) | Optional | Design references only; project conventions win. |
| GitHub MCP | Local Docker with OAuth, machine-local config | Not configured | Prefer OAuth over tokens; least privilege; never commit credentials. |
| Filesystem MCP | — | Not installed | Claude Code already has file tools; extra access = extra attack surface. |
| Apelora dev MCP (catalog/validate, read-only) | `tools/mcp/apelora-dev`, local stdio | Future | Must not run Incus operations. |
| Incus / control plane / cluster member APIs | Rust management plane | Future, **not MCP** | Authenticated, audited; never exposed to an AI agent unrestricted. |

## Rules

1. Bind dev servers to `127.0.0.1`. Do not open MCP or Vite ports to a network or to dom0.
2. No secrets in `.mcp.json`. Use env vars or the OS keyring.
3. Pin execution tools (`@latest` only to bootstrap); review updates.
4. Browser automation targets only the local dev server; tests assert there are no requests to external hosts.

## Verifying

```
claude mcp list
claude mcp get playwright
npx playwright --version && npx playwright test      # independent of MCP
```
