# Staying in sync with upstream Incus

Status: in place (Phase 02) · Related: [ADR 0001](../adr/0001-incus-first.md), `upstream.lock.yaml`, `scripts/upstream/`, skills `upstream-maintenance` and `upstream-reference`

## How Apelora is linked to Incus

```text
lxc/incus (upstream, Apache-2.0)
   │  releases: v7.0.x LTS (we track), v7.x feature releases (watched)
   │
   ├── pinned in this repo ── upstream.lock.yaml  (tag + commit, verified)
   │                          scripts/upstream/*   (status, check, verify, update, checkout)
   │                          .github/workflows/upstream-check.yml (weekly PR on new release)
   │
   └── forked to Apelora/incus
          main          mirrors upstream main (synced by the workflow when a token is set)
          apelora/7.0   = pinned LTS tag + our patches (empty today)
```

- **Runtime:** hosts install Incus as a normal package of the pinned release. Apelora talks to it only over its REST API.
- **Our features** (HA, evacuation, replica management, network intents, publishing) live in Apelora's Rust services, not in Incus, so upstream updates never conflict with them.
- **Patches to Incus itself** go on `apelora/7.0` in the fork, only when a feature cannot be built through the API. They are Apache-2.0 (they become part of Incus, not of our AGPL code) and are offered upstream.

## Everyday commands

```sh
bash scripts/upstream/status.sh          # pinned vs upstream releases, local checkout
bash scripts/upstream/verify.sh          # pinned tag still resolves to the pinned commit
bash scripts/upstream/check-updates.sh   # is there a newer release in the 7.0 series?
bash scripts/upstream/bootstrap.sh       # optional read-only source in upstream/incus (git-ignored)
bash scripts/upstream/prepare-update.sh v7.0.3   # move the pin (for an update PR)
```

## When upstream releases a fix

1. The weekly workflow (or `check-updates.sh`) finds the new 7.0.x tag and opens a pull request that moves the pin, with a compare link.
2. A human reads the release notes: security fixes, API extension changes, host minimums.
3. If `apelora/7.0` carries patches: rebase them onto the new tag (below) and re-run their tests.
4. Run the adapter contract tests, and the disposable test cluster when we have one; record the evidence in `tested_with`.
5. Merge. Hosts are upgraded by operators through their package manager, never by this repository.

Moving to a new LTS series (e.g. 8.0) is a deliberate decision with its own ADR, not an automatic update.

## Carrying a patch in the fork

```sh
bash scripts/upstream/bootstrap.sh
cd upstream/incus
git fetch --unshallow origin && git fetch apelora
git switch -c apelora/7.0 apelora/apelora/7.0
# commit the change with a human Signed-off-by (DCO), Apache-2.0, no AI credit
git push apelora apelora/7.0
```

On a new upstream tag: `git rebase --onto v7.0.<new> v7.0.<old> apelora/7.0`, resolve, test, push with lease. Keep the patch set small and send each patch upstream; a patch that upstream accepts is dropped from the branch at the next rebase.

Incus contribution rules (from its own policy): Apache-2.0 compatible code only, a human DCO sign-off on every commit, no AI tool credited, and AI agents must not run on machines holding Incus repository credentials. Agents never open upstream issues or pull requests.

## Capabilities to evaluate (from the earlier Xen design)

Things the Xen-based plan offered that Incus may not provide directly. Each needs verification against the pinned Incus release before any work; most should be built in Apelora's layer, not by patching Incus.

| Capability | Where it would live | Note |
|---|---|---|
| Continuous VM checkpoint replication (warm standby with near-zero data loss) | Evaluate; possibly storage-level replication plus Apelora recovery policy | No equivalent assumed in Incus; Xen offered this for VMs |
| Driver or service isolation in separate privileged-but-small domains | Not planned | Incus VMs give a strong boundary; containers rely on kernel namespaces |
| Live migration of running stateful workloads | Apelora mobility (replace-then-retire for stateless; planned stop-and-move otherwise); Incus VM live migration where supported | Ordinary LXC live migration is limited upstream |
| Very small single-purpose guests (unikernels) | Not planned | OCI application containers cover most lightweight cases |

Record outcomes in `.agents/skills/upstream-reference/SKILL.md` and, for decisions, as ADRs.
