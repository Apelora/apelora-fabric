// SPDX-License-Identifier: AGPL-3.0-only
//! Read-only checks of the instance detail and log reads against a real Incus server, using the
//! `APELORA_INCUS_*` settings (the disposable test cluster: `docs/development/TEST-CLUSTER.md`).
//! Ignored by default; they change nothing (GET requests only) and print what they read.
//!
//! `source ~/.config/apelora/test-cluster/env && cargo test -p apelora-incus-adapter --test real_reads -- --ignored --nocapture`

#![allow(clippy::unwrap_used, clippy::expect_used)]

use apelora_incus_adapter::Provider;
use apelora_incus_adapter::config::provider_from_env;

fn real() -> Provider {
    let p = provider_from_env(|k| std::env::var(k).ok()).expect("Incus settings");
    assert!(
        matches!(p, Provider::Incus(_)),
        "refusing to run against the mock: set APELORA_INCUS_URL"
    );
    p
}

#[tokio::test]
#[ignore = "needs a real Incus server (read-only)"]
async fn every_visible_instance_has_a_detail_its_log_files_and_a_console_log() {
    let p = real();
    // A missing instance: no detail, and its logs are "not found", not an error of another kind.
    let project = std::env::var("APELORA_DEPLOY_PROJECTS")
        .ok()
        .and_then(|s| s.split(',').next().map(str::to_owned))
        .unwrap_or_else(|| "default".into());
    assert_eq!(
        p.instance_view(&project, "apelora-no-such-instance").await,
        Ok(None)
    );
    assert!(matches!(
        p.instance_logs(&project, "apelora-no-such-instance").await,
        Err(apelora_incus_adapter::AdapterError::NotFound(_))
    ));
    let instances = p.instances().await.expect("instances");
    println!("{} instances visible", instances.len());
    for i in instances.iter().filter(|i| !i.unreachable) {
        let v = p
            .instance_view(&i.project, &i.name)
            .await
            .expect("detail")
            .expect("exists");
        assert_eq!((v.name.as_str(), v.kind), (i.name.as_str(), i.kind));
        println!(
            "{}/{} {:?} {} on {}: limits {:?}, usage {}, {} addresses, {} disks, {} snapshots",
            v.project,
            v.name,
            v.kind,
            v.status,
            v.location,
            v.limits,
            v.usage.is_some(),
            v.addresses.len(),
            v.disks.len(),
            v.snapshots.len()
        );
        let files = p
            .instance_logs(&i.project, &i.name)
            .await
            .expect("log list");
        println!("  log files: {files:?}");
        for f in &files {
            let tail = p
                .instance_log(&i.project, &i.name, f, 4096)
                .await
                .expect("log file");
            println!(
                "  {f}: {} bytes, truncated {}, shown {}",
                tail.size,
                tail.truncated,
                tail.content.len()
            );
        }
        match p.console_log(&i.project, &i.name, 4096).await {
            Ok(c) => println!("  console: {} bytes, truncated {}", c.size, c.truncated),
            Err(e) => println!("  console: refused: {e}"),
        }
    }
}
