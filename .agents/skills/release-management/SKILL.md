---
name: release-management
description: Cut and publish Apelora Fabric releases — version tags (v0.1.0-alpha.N, -beta.N, -rc.N, final), the release workflow (checks, artefacts, checksums, provenance, notes), and what to update before tagging. Use when the owner asks for a release, after a major update lands on main, or when changing .github/workflows/release.yml.
---

# Release management

**Read first:** `docs/development/RELEASING.md` (versions, steps, recovery) and `.github/workflows/release.yml`.

## Rules

| Area | Rule |
|---|---|
| Versions | `vMAJOR.MINOR.PATCH` with an optional `-alpha.N`, `-beta.N` or `-rc.N`; `N` counts up and never repeats; a tag is never moved or reused |
| Cadence | An alpha after each major update on `main` (phase slice, security fix, new capability, install change) — not for every small fix |
| Gate | Tag only a commit on `main` whose CI passed; the workflow refuses anything else |
| Pre-releases | alpha, beta and rc tags publish at once as pre-releases; a final version stops as a draft for a maintainer |
| Artefacts | Each service and screen on its own, both architectures, SHA-256 checksums, build-provenance attestation; nothing else is attached |
| Notes | Generated from commit subjects since the previous tag: write subjects that say what changed for whom |
| Before tagging | `docs/architecture/current-state.md` describes the version; anything pinned to a version (e.g. the default tag in `scripts/test-cluster/install-units.sh`) is updated in the same commit |
| Permission | Publishing is outward-facing: tag only when the owner asked for releases or approved this one |
| Workflow changes | Actions pinned by commit hash with the version in a comment; least-privilege `permissions` per job; `actionlint` passes (it runs in CI) |

## Procedure

1. `git switch main && git pull --ff-only`; confirm CI on `HEAD` passed (`gh run list --workflow ci.yml --commit "$(git rev-parse HEAD)"`).
2. Pick the next tag: `git tag --list 'v*' --sort=-v:refname | head -3`.
3. `git tag -a "$TAG" -m "Apelora $TAG" && git push origin "$TAG"`; watch `release.yml`.
4. Verify: `gh release view "$TAG"`, download the assets, check `SHA256SUMS-*`, `gh attestation verify <archive> --repo Apelora/apelora-fabric`.
5. If the version changes behaviour an existing installation relies on (migrations, permissions, refused settings), add an *Upgrading from the previous version* section to the top of the notes (`gh release edit "$TAG" --notes-file …`).
6. Tell the owner the release link and what it contains.

## Acceptance

- The release exists with every archive for both architectures, both checksum files and the notes.
- Checksums match and the attestation verifies.
- Failures are reported plainly; a broken published pre-release is marked in its notes and superseded, never deleted.
