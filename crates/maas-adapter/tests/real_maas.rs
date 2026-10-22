// SPDX-License-Identifier: AGPL-3.0-only
//! REAL MAAS TESTED evidence: the adapter against a real MAAS region API over TLS. Ignored by
//! default; run only against the throwaway MAAS of the test cluster:
//!
//! ```sh
//! bash scripts/test-cluster/maas.sh up && bash scripts/test-cluster/maas.sh machines
//! bash scripts/test-cluster/maas.sh test
//! ```
//!
//! The machines in that MAAS are records created over the API with manual power: nothing here
//! touches hardware, and nothing is changed except reading (and asking to power off a machine
//! whose power type is manual, which MAAS refuses).

use apelora_maas_adapter::{ApiKey, MaasClient, MaasError, PowerState};

fn env(name: &str) -> String {
    std::env::var(name)
        .unwrap_or_else(|_| panic!("{name} is not set: bash scripts/test-cluster/maas.sh test"))
}

fn client() -> MaasClient {
    let ca = env("APELORA_TEST_MAAS_CA");
    MaasClient::new(
        &env("APELORA_TEST_MAAS_URL"),
        ApiKey::parse(&env("APELORA_TEST_MAAS_KEY")).expect("key"),
        Some(std::path::Path::new(&ca)),
    )
    .expect("client")
}

#[tokio::test]
#[ignore = "needs the throwaway MAAS: scripts/test-cluster/maas.sh"]
async fn the_real_maas_lists_its_machines_and_answers_for_one() {
    let c = client();
    let all = c.machines().await.expect("machines");
    eprintln!("{} machines", all.len());
    for m in &all {
        eprintln!(
            "  {} {} status={} testing={:?} power={:?} type={} arch={} cpus={} memory={} MiB uuid={:?} zone={} pool={}",
            m.system_id,
            m.hostname,
            m.status,
            m.testing_status,
            m.power_state,
            m.power_type,
            m.architecture,
            m.cpu_count,
            m.memory_mib,
            m.hardware_uuid,
            m.zone,
            m.pool
        );
    }
    let lab: Vec<_> = all
        .iter()
        .filter(|m| m.hostname.starts_with("apelora-lab-"))
        .collect();
    assert!(
        !lab.is_empty(),
        "run scripts/test-cluster/maas.sh machines first"
    );
    assert!(
        lab.iter().all(|m| !m.testing_status.is_empty()),
        "MAAS reports a testing status for every machine"
    );
    let m = lab[0];
    // One machine, by its system id: the same facts.
    let one = c.machine(&m.system_id).await.expect("machine");
    assert_eq!(
        (one.system_id.as_str(), one.hostname.as_str()),
        (m.system_id.as_str(), m.hostname.as_str())
    );
    // A machine with manual power: MAAS has no BMC to ask. The adapter must not turn that into
    // "off": it is `Unknown` or an error, never evidence.
    match c.query_power_state(&m.system_id).await {
        Ok(state) => {
            eprintln!("query_power_state of a manual machine: {state:?}");
            assert_ne!(
                state,
                PowerState::Off,
                "a machine MAAS cannot power-check must not read as Off"
            );
        }
        Err(e) => eprintln!("query_power_state of a manual machine is refused: {e}"),
    }
    // Asked to power it off, MAAS must not claim success for a machine it cannot power: a
    // success here would be taken as accepted and then not confirmed by the read-back, which is
    // the safe outcome; this prints what MAAS really does.
    match c.power_off(&m.system_id).await {
        Ok(()) => eprintln!("power_off of a manual machine: accepted (the read-back decides)"),
        Err(e) => eprintln!("power_off of a manual machine: {e}"),
    }
    assert_ne!(
        c.query_power_state(&m.system_id)
            .await
            .unwrap_or(PowerState::Unknown),
        PowerState::Off
    );
}

#[tokio::test]
#[ignore = "needs the throwaway MAAS: scripts/test-cluster/maas.sh"]
async fn the_real_maas_refuses_a_wrong_key_and_an_unknown_machine_and_is_not_reachable_without_its_certificate()
 {
    let url = env("APELORA_TEST_MAAS_URL");
    let ca = env("APELORA_TEST_MAAS_CA");
    // A wrong secret: refused, and the error does not carry the key.
    let bad = MaasClient::new(
        &url,
        ApiKey::parse("nope:nope:nopesecret").expect("key"),
        Some(std::path::Path::new(&ca)),
    )
    .expect("client");
    let e = bad.machines().await.expect_err("401");
    eprintln!("wrong key: {e}");
    assert!(matches!(e, MaasError::Refused { status: 401, .. }), "{e:?}");
    assert!(!e.to_string().contains("nopesecret"));
    // A machine MAAS does not have.
    let e = client().machine("zzzzzz").await.expect_err("404");
    eprintln!("unknown machine: {e}");
    assert!(matches!(e, MaasError::Refused { status: 404, .. }), "{e:?}");
    // Without the lab's CA the self-signed certificate is not trusted: no request is made.
    let key = ApiKey::parse(&env("APELORA_TEST_MAAS_KEY")).expect("key");
    let untrusted = MaasClient::new(&url, key, None).expect("client");
    let e = untrusted.machines().await.expect_err("untrusted");
    eprintln!("without the CA: {e}");
    assert!(matches!(e, MaasError::Unreachable(_)), "{e:?}");
}
