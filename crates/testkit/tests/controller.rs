//! Password connections: Server Key secret + Device ID + Device Password. They reach only their own Device.

use rcli_testkit::{StatusCode, TestSystem};
use serde_json::json;

#[tokio::test]
async fn a_password_connection_reaches_its_own_device_only() {
    let sys = TestSystem::start().await;
    let mine = sys.start_agent("mine").await;
    let other = sys.start_agent("other").await;
    sys.wait_device(mine.device_id(), |d| d["status"] == "Online").await;
    sys.wait_device(other.device_id(), |d| d["status"] == "Online").await;
    let dev = sys.device_client(mine.device_id(), &mine.device_password().unwrap());

    let (status, body) = dev.get(&format!("/api/v1/devices/{}", mine.device_id())).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["device_id"], mine.device_id());

    let (status, body) = dev.get(&format!("/api/v1/devices/{}", other.device_id())).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "another Device: {body}");

    // No Server administration.
    let (status, body) = dev.get("/api/v1/server-key").await;
    assert_eq!((status, body["error"]["code"].clone()), (StatusCode::FORBIDDEN, json!("user_session_required")));
    let (status, _) = dev.get("/api/v1/users").await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn a_wrong_password_or_server_key_is_unauthorized() {
    let sys = TestSystem::start().await;
    let agent = sys.start_agent("mine").await;
    sys.wait_device(agent.device_id(), |d| d["status"] == "Online").await;
    let path = format!("/api/v1/devices/{}", agent.device_id());

    let wrong = sys.device_client(agent.device_id(), "wrongpassword");
    let (status, body) = wrong.get(&path).await;
    assert_eq!((status, body["error"]["code"].clone()), (StatusCode::UNAUTHORIZED, json!("unauthorized")));

    let wrong_key = format!("rk_{}_{}", "00".repeat(32), "0".repeat(32));
    let bad = sys.device_client(agent.device_id(), &agent.device_password().unwrap()).with_server_key(&wrong_key);
    let (status, _) = bad.get(&path).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn the_control_lock_names_a_password_connection_by_controller_and_ip() {
    let sys = TestSystem::start().await;
    let agent = sys.start_agent("mine").await;
    sys.wait_device(agent.device_id(), |d| d["status"] == "Online").await;
    let dev = sys.device_client(agent.device_id(), &agent.device_password().unwrap()).with_controller("dev-laptop");

    let (status, body) = dev.post(&format!("/api/v1/devices/{}/lock", agent.device_id())).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["holder"], "dev-laptop (127.0.0.1)");
}
