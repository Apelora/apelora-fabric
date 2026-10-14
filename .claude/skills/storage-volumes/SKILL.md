---
name: storage-volumes
description: Design provider-neutral persistent volumes — creation, attachment to appliances, snapshots, resize, and lifecycle/retention. Use when working on volume abstractions or storage backends.
---

# Storage volumes

## Abstraction

A `VolumeProvider` trait (create, delete, resize, attach, detach, snapshot, restore, clone, status) with backends such as local file/LVM/ZFS/Ceph added behind it. The Controller reasons only about the trait.

## Model

- `Volume`: id, size, provider, state (`creating|available|attached|deleting|error`), owner assembly
- Attachment: volume -> instance + device slot, mode `rw|ro`; a `rw` block volume attaches to at most one instance
- Snapshot: immutable, with parent chain and retention
- Lifecycle policy per volume: `delete-with-app` (default for scratch) or `retain` (default for data)

## Rules

1. Destructive operations (delete, restore-over, shrink) require explicit intent and are never implied by removing an appliance from a canvas.
2. Detach/stop before snapshot unless the backend guarantees crash-consistency; record which was achieved.
3. Resize is grow-only by default.
4. Encrypt at rest where the provider supports it; keys are referenced, never stored in manifests.
5. Operations are idempotent and reconciled like all other desired state (`rust-control-plane`).

## Checklist

- Fake provider for tests; fault injection (partial create, failed detach)
- Attachment conflict and orphan-volume tests
