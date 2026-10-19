// SPDX-License-Identifier: AGPL-3.0-only
//! Apelora deployment worker (reconciler).
//!
//! Phase 01 skeleton: starts read-only against the mock provider and exits. Durable jobs,
//! leases and reconciliation arrive in Phases 03 and 08.

use apelora_incus_adapter::{IncusProvider, MockIncus};

fn main() {
    tracing_subscriber::fmt().init();
    let provider = MockIncus::default();
    tracing::info!(
        provider = ?provider.mode(),
        read_only = true,
        "deployment worker skeleton: nothing to reconcile yet"
    );
}
