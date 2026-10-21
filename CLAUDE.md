# Apelora Fabric — Claude Code entry point

The project rules live in `AGENTS.md` so that every coding agent (Claude Code, Codex) follows the same rules. They are imported here:

@AGENTS.md

Claude-specific notes:
- Skills are in `.agents/skills/`; `.claude/skills/<name>` and `.cursor/skills/<name>` are symlinks to them. Add new skills under `.agents/skills/` and symlink them, never copy.
- Subagents are in `.claude/agents/` (`domain-architect`, `ui-engineer`, `qa-reviewer`).
