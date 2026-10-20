---
name: storage-volumes
description: Design provider-neutral persistent volumes — creation, attachment to modules, snapshots, resize, and lifecycle/retention. Use when working on volume abstractions or storage backends.
---

# Storage volumes

## Abstraction

Incus owns pools and drivers; Apelora asks for volumes through `apelora_incus_adapter::Provider` (`ensure_volume`, `attach_volume`, `volume`, `delete_volume`; the mock injects failures through `MockStorage`). Designs name a volume class, never a pool: the control plane resolves `data-local` to `APELORA_VOLUME_POOL`. Design and verified Incus behaviour: `docs/architecture/STORAGE.md` (§10 lists what the pinned release really does).

## As built

- `apelora_domain::storage::VolumeMount` in `InstanceSpec.volumes`; `validate_volumes` checks names, sizes, paths and Incus name length; `check_resize` is grow-only.
- One volume per instance, `<instance>-<volume>`, device `vol-<volume>`, labels `user.apelora.managed|application|module|volume`.
- Worker order (Incus does not follow a member-local volume): create the instance stopped → volume on its member → attach → start. Stateless modules cannot have volumes.
- Deleting an instance keeps its volumes; deleting a volume is explicit and only for its own application.

## Model

- `Volume`: id, size, provider, state (`creating|available|attached|deleting|error`), owner assembly
- Attachment: volume -> instance + device slot, mode `rw|ro`; a `rw` block volume attaches to at most one instance
- Snapshot: immutable, with parent chain and retention
- Lifecycle policy per volume: `delete-with-app` (default for scratch) or `retain` (default for data)

## Rules

1. Destructive operations (delete, restore-over, shrink) require explicit intent and are never implied by removing a module from a canvas.
2. Detach/stop before snapshot unless the backend guarantees crash-consistency; record which was achieved.
3. Resize is grow-only by default.
4. Encrypt at rest where the provider supports it; keys are referenced, never stored in manifests.
5. Operations are idempotent and reconciled like all other desired state (`rust-control-plane`).

## Checklist

- Fake provider for tests; fault injection (partial create, failed detach)
- Attachment conflict and orphan-volume tests
