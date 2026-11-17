use rcli_agent::ServerAccess;
use rcli_testkit::{eventually, FakePlatform, StatusCode, TestSystem};
use serde_json::json;

#[tokio::test]
async fn a_device_registers_with_the_server_key_and_gets_a_nine_digit_device_id() {
    let sys = TestSystem::start().await;
    let agent = sys.start_agent("lab-pc").await;
    let id = agent.device_id();
    assert_eq!(id.len(), 9, "{id}");
    assert!(id.chars().all(|c| c.is_ascii_digit()), "{id}");

    let device = sys.wait_device(id, |d| d["status"] == "Online").await;
    assert_eq!(device["device_id"], id);
    assert_eq!(device["name"], "lab-pc");

    // The 9-digit Device ID addresses the Device through the Controller API.
    let (status, shown) = sys.api.get(&format!("/api/v1/devices/{id}")).await;
    assert_eq!(status, StatusCode::OK, "{shown}");
    assert_eq!(shown["device_id"], id);
}

#[tokio::test]
async fn a_wrong_server_key_secret_is_refused_and_leaves_no_device() {
    let sys = TestSystem::start().await;
    let key = sys.server_key().to_owned();
    let (prefix, _) = key.rsplit_once('_').unwrap();
    let wrong = format!("{prefix}_{}", "0".repeat(32));
    let server_access = ServerAccess { addr: sys.agent_addr(), server_key: wrong };

    let err = rcli_agent::register(&server_access, &FakePlatform::new("impostor")).await.unwrap_err();
    assert!(err.to_string().contains("invalid_server_key"), "{err}");
    assert_eq!(device_count(&sys).await, 0, "nothing was registered");
}

#[tokio::test]
async fn a_server_key_with_another_fingerprint_refuses_the_server() {
    let sys = TestSystem::start().await;
    let key = sys.server_key().to_owned();
    let (_, secret) = key.rsplit_once('_').unwrap();
    let wrong = format!("rk_{}_{secret}", "00".repeat(32));
    let server_access = ServerAccess { addr: sys.agent_addr(), server_key: wrong };

    let err = rcli_agent::register(&server_access, &FakePlatform::new("lab-pc")).await.unwrap_err();
    assert!(err.to_string().contains("fingerprint"), "{err}");
    assert_eq!(device_count(&sys).await, 0, "nothing was sent to the Server");
}

/// How many Devices the Server holds; no API lists them.
async fn device_count(sys: &TestSystem) -> i64 {
    let db = sqlx::PgPool::connect(&sys.config.database_url).await.unwrap();
    sqlx::query_scalar("SELECT count(*) FROM devices").fetch_one(&db).await.unwrap()
}

#[tokio::test]
async fn controller_api_requires_admin_token() {
    let sys = TestSystem::start().await;
    for token in [None, Some("wrong")] {
        let (status, body) = sys.api.with_token(token).get("/api/v1/address-book").await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["error"]["code"], "unauthorized");
        let (status, _) = sys.api.with_token(token).get("/api/v1/server-key").await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }
}

#[tokio::test]
async fn a_user_reads_the_server_key() {
    let sys = TestSystem::start().await;
    let out = sys.rcli(&["server-key", "show"]).await.unwrap();
    let key = out.trim();
    assert!(key.starts_with("rk_"), "{out}");
    assert_eq!(key, sys.server_key(), "{out}");
}

#[tokio::test]
async fn a_device_is_online_while_connected_and_offline_30s_after_drop() {
    let sys = TestSystem::start().await;
    let mut agent = sys.start_agent("build-box").await;

    let device = sys.wait_device(agent.device_id(), |d| d["status"] == "Online").await;
    assert_eq!(device["name"], "build-box");

    agent.stop();
    sys.wait_device(agent.device_id(), |d| !d["last_seen_at"].is_null()).await;
    sys.advance(29);
    sys.wait_device(agent.device_id(), |d| d["status"] == "Online").await;
    sys.advance(1);
    sys.wait_device(agent.device_id(), |d| d["status"] == "Offline").await;

    agent.start();
    sys.wait_device(agent.device_id(), |d| d["status"] == "Online" && d["last_seen_at"].is_null()).await;
}

#[tokio::test]
async fn a_new_device_is_in_no_address_book_and_reached_only_by_its_password() {
    let sys = TestSystem::start().await;
    let agent = sys.start_agent("lab-pc").await;
    let (id, password) = (agent.device_id(), agent.device_password().unwrap());
    let alice = sys.api.with_token(Some(&sys.create_user_without_devices("alice").await));
    assert_eq!(alice.get("/api/v1/address-book").await.1, json!([]), "a new install is in no Address Book");

    // No API lists Devices, and a User reaches none they have not added.
    assert_eq!(alice.get("/api/v1/devices").await.0, StatusCode::NOT_FOUND);
    for path in [format!("/api/v1/devices/{id}"), format!("/api/v1/devices/{id}/lock")] {
        let (status, body) = alice.get(&path).await;
        assert_eq!((status, body["error"]["code"].clone()), (StatusCode::NOT_FOUND, json!("device_not_found")), "{path}");
    }
    let (status, _) = alice.post_json(&format!("/api/v1/devices/{id}/exec"), &json!({ "command": "echo hi" })).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let add = json!({ "device_id": id, "password": password });
    assert_eq!(alice.post_json("/api/v1/address-book", &add).await.0, StatusCode::CREATED);
    let shown = eventually(|| async {
        let (status, shown) = alice.get(&format!("/api/v1/devices/{id}")).await;
        (status == StatusCode::OK && shown["status"] == "Online").then_some(shown)
    })
    .await;
    assert_eq!(shown["device_id"], id);

    // The Server keeps only what it needs to connect.
    let db = sqlx::PgPool::connect(&sys.config.database_url).await.unwrap();
    let columns: Vec<String> = sqlx::query_scalar(
        "SELECT column_name::text FROM information_schema.columns WHERE table_name = 'devices' ORDER BY column_name",
    )
    .fetch_all(&db)
    .await
    .unwrap();
    let want = ["agent_target", "agent_version", "cert_fingerprint", "device_number", "device_password_hash", "id", "last_seen_at"];
    assert_eq!(columns, want);
}
