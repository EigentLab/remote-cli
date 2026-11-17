//! A Dashboard User's Address Book: Devices added by Device ID and Device Password, per User.

use rcli_testkit::{LiveViewer, StatusCode, TestSystem};
use serde_json::json;

#[tokio::test]
async fn a_user_adds_a_device_by_id_and_password_and_opens_its_live_view_and_terminal() {
    let sys = TestSystem::start().await;
    let agent = sys.start_agent("lab-pc").await;
    sys.wait_device(agent.device_id(), |d| d["status"] == "Online").await;
    let alice = sys.api.with_token(Some(&sys.create_user_without_devices("alice").await));
    let (_, empty) = alice.get("/api/v1/address-book").await;
    assert_eq!(empty, json!([]), "a new User's Address Book is empty");

    let add = json!({ "device_id": agent.device_id(), "password": agent.device_password().unwrap(), "name": "lab" });
    let (status, added) = alice.post_json("/api/v1/address-book", &add).await;
    assert_eq!(status, StatusCode::CREATED, "{added}");

    let (_, list) = alice.get("/api/v1/address-book").await;
    assert_eq!(list.as_array().unwrap().len(), 1, "{list}");
    let entry = &list[0];
    assert_eq!((&entry["device_id"], &entry["name"], &entry["status"]), (&json!(agent.device_id()), &json!("lab"), &json!("Online")));
    let id = entry["id"].as_str().unwrap();

    let live = LiveViewer::open(&alice, id, false).await.expect("open live view");
    live.wait_packets(5).await;
    live.close(&alice).await;
    let (status, out) = alice.post_json(&format!("/api/v1/devices/{id}/exec"), &json!({ "command": "echo hi" })).await;
    assert_eq!((status, out["stdout"].clone()), (StatusCode::OK, json!("hi\n")), "{out}");

    // Renamed, then removed.
    let (status, renamed) = alice.patch_json(&format!("/api/v1/address-book/{id}"), &json!({ "name": "lab 2" })).await;
    assert_eq!((status, renamed["name"].clone()), (StatusCode::OK, json!("lab 2")), "{renamed}");
    assert_eq!(alice.delete(&format!("/api/v1/address-book/{id}")).await.0, StatusCode::NO_CONTENT);
    assert_eq!(alice.get("/api/v1/address-book").await.1, json!([]));
}

#[tokio::test]
async fn one_users_address_book_is_not_visible_to_another() {
    let sys = TestSystem::start().await;
    let agent = sys.start_agent("lab-pc").await;
    sys.wait_device(agent.device_id(), |d| d["status"] == "Online").await;
    let alice = sys.api.with_token(Some(&sys.create_user_without_devices("alice").await));
    let bob = sys.api.with_token(Some(&sys.create_user_without_devices("bob").await));
    let add = json!({ "device_id": agent.device_id(), "password": agent.device_password().unwrap() });
    let (status, added) = alice.post_json("/api/v1/address-book", &add).await;
    assert_eq!(status, StatusCode::CREATED, "{added}");

    assert_eq!(bob.get("/api/v1/address-book").await.1, json!([]));
    let path = format!("/api/v1/address-book/{}", agent.device_id());
    assert_eq!(bob.patch_json(&path, &json!({ "name": "mine" })).await.0, StatusCode::NOT_FOUND);
    assert_eq!(bob.delete(&path).await.0, StatusCode::NOT_FOUND);
    assert_eq!(alice.get("/api/v1/address-book").await.1.as_array().unwrap().len(), 1, "bob removed nothing");
}

#[tokio::test]
async fn a_wrong_device_password_adds_nothing() {
    let sys = TestSystem::start().await;
    let agent = sys.start_agent("lab-pc").await;
    sys.wait_device(agent.device_id(), |d| d["status"] == "Online").await;
    let alice = sys.api.with_token(Some(&sys.create_user_without_devices("alice").await));

    let add = json!({ "device_id": agent.device_id(), "password": "wrongpassword" });
    let (status, body) = alice.post_json("/api/v1/address-book", &add).await;
    assert_eq!((status, body["error"]["code"].clone()), (StatusCode::UNAUTHORIZED, json!("unauthorized")));
    let unknown = json!({ "device_id": "000000000", "password": "wrongpassword" });
    assert_eq!(alice.post_json("/api/v1/address-book", &unknown).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(alice.get("/api/v1/address-book").await.1, json!([]));
}
