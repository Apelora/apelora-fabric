# Releasing

Apelora Fabric is released from `main` by pushing a version tag. The release workflow (`.github/workflows/release.yml`) builds every service and screen on its own, for x86-64 and ARM64, with SHA-256 checksums and a build-provenance attestation, and publishes them on GitHub.

## Versions

| Tag | Meaning | What the workflow does |
|---|---|---|
| `v0.1.0-alpha.N` | Work in progress: features still arriving, interfaces may change | Publishes a **pre-release** at once |
| `v0.1.0-beta.N` | Feature-complete for the version; fixes only | Publishes a **pre-release** at once |
| `v0.1.0-rc.N` | A candidate we would ship unchanged | Publishes a **pre-release** at once |
| `v0.1.0` | A release | Creates a **draft**; a maintainer reviews and publishes it |

`N` counts up from 1 and never repeats. A tag is never moved or reused: a mistake gets the next number.

**When to cut an alpha.** After each major update lands on `main` with CI green: a phase slice, a security fix, a new capability someone can try, or a change to how Apelora is installed. Small fixes wait for the next one.

## Steps

1. **`main` is green.** The tagged commit must be on `main`, and its CI run must have passed (the workflow checks both and stops otherwise).
2. **What works today is current.** `docs/architecture/current-state.md` describes this version; anything the install guide or scripts pin to a version (for example the default tag in `scripts/test-cluster/install-units.sh`) is updated in the same commit.
3. **Tag and push.**

   ```sh
   git switch main && git pull --ff-only
   TAG=v0.1.0-alpha.N                       # the next number
   git tag -a "$TAG" -m "Apelora $TAG"
   git push origin "$TAG"
   ```

4. **Watch the run.** `gh run watch --repo Apelora/apelora-fabric "$(gh run list --repo Apelora/apelora-fabric --workflow release.yml --limit 1 --json databaseId --jq '.[0].databaseId')"`
5. **Check what was published.**

   ```sh
   gh release view "$TAG" --repo Apelora/apelora-fabric
   gh release download "$TAG" --repo Apelora/apelora-fabric --dir /tmp/apelora-$TAG
   cd /tmp/apelora-$TAG && shasum -a 256 -c SHA256SUMS-apps
   gh attestation verify apelora-studio-$TAG.tar.gz --repo Apelora/apelora-fabric
   ```

6. **A final version** (`v0.1.0`) stops as a draft: read the notes, open the assets, then `gh release edit "$TAG" --draft=false --repo Apelora/apelora-fabric`.

## Release notes

The workflow writes them from the commit subjects since the previous version tag, with a link to the full comparison, how to install that version, and how to check the downloads. Write commit subjects so they read well in that list: what changed, for whom.

## If a release goes wrong

- **The workflow failed before publishing:** fix on `main`, then tag the next number. Delete the failed tag only if nothing was published: `git push origin :refs/tags/$TAG`.
- **A published pre-release is broken:** do not delete it (someone may have downloaded it). Mark it in its notes (`gh release edit "$TAG" --notes-file …`), then publish the next number.
