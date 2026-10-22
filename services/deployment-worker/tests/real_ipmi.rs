// SPDX-License-Identifier: AGPL-3.0-only
//! IPMI fencing against a BMC that is really there (ignored by default). The settings come from
//! the environment, the same way the worker reads them:
//!
//! ```sh
//! APELORA_IPMI_TEST_HOST=10.0.0.21 APELORA_IPMI_TEST_USER=apelora-fence \
//! APELORA_IPMI_TEST_UUID=<the server's system GUID> APELORA_BMC_SECRET_IPMI_TEST=… \
//! cargo test -p apelora-deployment-worker --test real_ipmi -- --ignored
//! ```
//!
//! `APELORA_IPMI_TEST_PORT` defaults to 623. The test reads the power state, and refuses a wrong
//! GUID. It switches the server off only with `APELORA_IPMI_TEST_FENCE=1`: use a server nobody
//! needs.

use apelora_deployment_worker::bmc::{PowerState, bmc_from_env};

fn env(k: &str) -> Option<String> {
    std::env::var(k).ok().filter(|v| !v.trim().is_empty())
}

fn mapping(uuid: &str) -> std::path::PathBuf {
    let need = |k: &str| env(k).unwrap_or_else(|| panic!("{k} is not set"));
    let port: u16 = env("APELORA_IPMI_TEST_PORT").map_or(623, |p| p.parse().expect("port"));
    let dir = std::env::temp_dir().join(format!("apelora-real-ipmi-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("dir");
    let file = dir.join(format!("map-{uuid}.json"));
    let text = serde_json::json!({ "test-1": {
        "provider": "ipmi", "host": need("APELORA_IPMI_TEST_HOST"), "port": port,
        "user": need("APELORA_IPMI_TEST_USER"), "secretRef": "APELORA_BMC_SECRET_IPMI_TEST",
        "uuid": uuid,
    }});
    std::fs::write(&file, text.to_string()).expect("write");
    file
}

fn bmc(uuid: &str) -> apelora_deployment_worker::bmc::Bmc {
    let file = mapping(uuid);
    let (b, _) = bmc_from_env(|k| match k {
        "APELORA_BMC_MAP" => Some(file.display().to_string()),
        _ => env(k),
    })
    .expect("settings");
    b
}

#[tokio::test]
#[ignore = "needs a BMC: APELORA_IPMI_TEST_HOST and friends"]
async fn a_real_bmc_is_read_refuses_a_wrong_guid_and_is_fenced_only_when_allowed() {
    let uuid = env("APELORA_IPMI_TEST_UUID").expect("APELORA_IPMI_TEST_UUID");
    let b = bmc(&uuid);
    let before = b.power_state("test-1").await.expect("power state");
    println!("power state: {before:?}");
    assert!(matches!(before, PowerState::On | PowerState::Off));

    let wrong = bmc("00000000-0000-4000-8000-000000000001");
    let e = wrong.power_state("test-1").await.expect_err("another GUID");
    assert!(e.contains("refusing"), "{e}");
    let password = env("APELORA_BMC_SECRET_IPMI_TEST").expect("secret");
    assert!(!e.contains(&password), "the password is never in an error");

    if env("APELORA_IPMI_TEST_FENCE").as_deref() == Some("1") {
        let r = b.fence("test-1").await.expect("fenced");
        println!("fenced: {r:?}");
        assert_eq!(r.provider, "ipmi");
        assert_eq!(r.switched_off, before == PowerState::On);
        assert_eq!(b.power_state("test-1").await, Ok(PowerState::Off));
    } else {
        println!("not switched off (set APELORA_IPMI_TEST_FENCE=1 for a disposable server)");
    }
}
