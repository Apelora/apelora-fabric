# Prompt: build Apelora Studio (frontend only)

> Historical prompt for the Studio milestone. Backend work now follows `docs/development/IMPLEMENTATION-PHASES.md` (Incus-first, ADR 0001).

Use with Claude Code from the repository root. Read `CLAUDE.md`, `docs/architecture/STUDIO-MVP.md`, `docs/development/MCP-HOSTING.md`, `.mcp.json` and the relevant skills first.

Goal: a fully working browser-only Studio (Vite, React, TypeScript, React Flow, Tailwind 4, daisyUI 5) with a white shell, a categorized searchable catalog, a framed light-gray canvas, compact 128×96 modules with typed left/right ports, automatic height growth, resizing, validated connections, a property inspector, JSON import/export, validation diagnostics, undo/redo and local persistence.

Rules: no Rust, hypervisor integration, Oxia, HA or authentication; never fabricate deployment results; verify in a real browser with Playwright; report changed files, tests run and results, screenshots, licensing/security concerns and remaining limitations; commit without any Claude attribution; stop for visual approval after the milestone.
