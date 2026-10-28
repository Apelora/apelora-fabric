// Sectors (ADR 0008, slice 1) through the real router: mock provider, where every registered
// sector is a mock cluster of its own (MOCK VERIFIED), and a real PostgreSQL.

#![allow(clippy::unwrap_used)] // test fixtures: a failed unwrap is the failure

use serde_json::json;

use super::identity_tests::{code, lab, world};
use super::*;

fn place() -> apelora_metrics::Place {
    apelora_metrics::Place::from_env(|key| std::env::var(key).ok()).expect("place")
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn sectors_are_registered_read_counted_as_sites_and_forgotten(pool: Pool) {
    let l = lab(pool.clone());
    let w = world(&l).await;
    let home = place();

    // Only the home sector at first, connected, and no second site.
    let (st, v) = l.req("GET", "/api/v1/sectors", &w.admin, None).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(v["items"].as_array().unwrap().len(), 1, "{v}");
    assert_eq!(v["items"][0]["home"], true);
    assert_eq!(v["items"][0]["name"], home.sector.as_str());
    assert_eq!(v["items"][0]["state"], "connected");
    assert!(v["note"].as_str().unwrap().contains("home sector only"));

    // Refused: what is not a sector, the home sector's name, a credentials name that looks like
    // a file.
    for (body, why) in [
        (
            json!({ "name": "Rack B", "site": "site-b", "url": "https://b.example:8443", "credentials": "SITE_B" }),
            "invalid_sector",
        ),
        (
            json!({ "name": "rack-b1", "site": "site-b", "url": "http://b.example:8443", "credentials": "SITE_B" }),
            "invalid_sector",
        ),
        (
            json!({ "name": "rack-b1", "site": "site-b", "url": "https://b.example:8443", "credentials": "/etc/apelora/b.key" }),
            "invalid_sector",
        ),
        (
            json!({ "name": home.sector, "site": "site-b", "url": "https://b.example:8443", "credentials": "SITE_B" }),
            "sector_exists",
        ),
    ] {
        let (st, b) = l
            .req("POST", "/api/v1/sectors", &w.admin, Some(body.clone()))
            .await;
        assert_eq!(code(&b), Some(why), "{body} → {st} {b}");
    }

    let other_site = if home.site == "site-b" {
        "site-c"
    } else {
        "site-b"
    };
    let (st, b) = l
        .req(
            "POST",
            "/api/v1/sectors",
            &w.admin,
            Some(json!({ "name": "rack-b1", "site": other_site, "url": "https://incus.b.example:8443/", "credentials": "SITE_B" })),
        )
        .await;
    assert_eq!(st, StatusCode::CREATED, "{b}");
    assert_eq!(b["home"], false);
    assert_eq!(b["url"], "https://incus.b.example:8443", "{b}");
    assert_eq!(b["state"], "connected", "the mock sector answers: {b}");
    assert_eq!(
        b["credentialVars"],
        json!([
            "APELORA_SECTOR_SITE_B_CLIENT_CERT",
            "APELORA_SECTOR_SITE_B_CLIENT_KEY",
            "APELORA_SECTOR_SITE_B_SERVER_CERT"
        ])
    );
    let (st, b) = l
        .req(
            "POST",
            "/api/v1/sectors",
            &w.admin,
            Some(json!({ "name": "rack-b1", "site": other_site, "url": "https://incus.b.example:8443", "credentials": "SITE_B" })),
        )
        .await;
    assert_eq!(
        (st, code(&b)),
        (StatusCode::CONFLICT, Some("sector_exists")),
        "{b}"
    );

    // A connected sector in another site lets the members report name it for a second
    // deployment (still not started).
    let (st, r) = l
        .req("GET", "/api/v1/cluster/members", &w.admin, None)
        .await;
    assert_eq!(st, StatusCode::OK, "{r}");
    assert_eq!(r["sites"]["connectedSites"], 2, "{r}");
    assert_eq!(r["sites"]["planned"], true, "{r}");
    assert!(
        r["sites"]["survival"].as_str().unwrap().contains("rack-b1"),
        "{r}"
    );

    // Forgetting it needs the name typed again, and is audited with the registration.
    let (st, b) = l
        .req(
            "DELETE",
            "/api/v1/sectors/rack-b1",
            &w.admin,
            Some(json!({ "confirm": "rack-b" })),
        )
        .await;
    assert_eq!(
        (st, code(&b)),
        (StatusCode::BAD_REQUEST, Some("confirmation_mismatch")),
        "{b}"
    );
    let (st, b) = l
        .req(
            "DELETE",
            "/api/v1/sectors/rack-b1",
            &w.admin,
            Some(json!({ "confirm": "rack-b1" })),
        )
        .await;
    assert_eq!(st, StatusCode::NO_CONTENT, "{b}");
    let (st, b) = l
        .req(
            "DELETE",
            "/api/v1/sectors/rack-b1",
            &w.admin,
            Some(json!({ "confirm": "rack-b1" })),
        )
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND, "{b}");
    let (_, v) = l.req("GET", "/api/v1/sectors", &w.admin, None).await;
    assert_eq!(v["items"].as_array().unwrap().len(), 1, "{v}");
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT action, target FROM audit_events WHERE action LIKE 'sector.%' ORDER BY id",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        rows,
        vec![
            ("sector.add".to_owned(), "sector:rack-b1".to_owned()),
            ("sector.remove".to_owned(), "sector:rack-b1".to_owned())
        ]
    );
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn only_platform_administrators_register_sectors(pool: Pool) {
    let l = lab(pool);
    let w = world(&l).await;
    let viewer_id = l.user("sub-vic", "vic").await;
    let viewer = l.token("sub-vic", "vic");
    let (st, g) = l
        .grant(
            &w.admin,
            viewer_id,
            "viewer",
            json!({ "type": "project", "id": w.acme_prod }),
        )
        .await;
    assert_eq!(st, StatusCode::CREATED, "{g}");
    let (st, b) = l.req("GET", "/api/v1/sectors", &viewer, None).await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{b}");
    let (st, b) = l
        .req(
            "POST",
            "/api/v1/sectors",
            &viewer,
            Some(json!({ "name": "rack-b1", "site": "site-b", "url": "https://b.example:8443", "credentials": "SITE_B" })),
        )
        .await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{b}");
}
