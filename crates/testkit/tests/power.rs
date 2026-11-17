//! Power Actions: Restart, Shut down, Sleep (need the Control Lock holder or an admin while held), Lock screen (no lock).

use rcli_testkit::{platform::PowerAction, StatusCode, TestSystem};
use serde_json::json;

fn power(dev: &str) -> String {
    format!("/api/v1/devices/{dev}/power")
}

#[tokio::test]
async fn a_user_restarts_a_device() {
    let sys = TestSystem::start().await;
    let agent = sys.start_agent("lab-pc").await;
    let dev = agent.device_id();
    sys.wait_device(dev, |d| d["status"] == "Online").await;
    let alice = sys.api.with_token(Some(&sys.create_user("alice").await));

    let (status, body) = alice.post_json(&power(dev), &json!({ "action": "restart" })).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body, json!({ "action": "restart" }));
    assert_eq!(agent.platform.power_actions(), vec![PowerAction::Restart]);
}

#[tokio::test]
async fn while_a_control_lock_is_held_only_the_holder_may_restart_shut_down_or_sleep() {
    let sys = TestSystem::start().await;
    let agent = sys.start_agent("lab-pc").await;
    let dev = agent.device_id();
    sys.wait_device(dev, |d| d["status"] == "Online").await;
    let alice = sys.api.with_token(Some(&sys.create_user("alice").await));
    let bob = sys.api.with_token(Some(&sys.create_user("bob").await));
    let (status, _) = alice.post(&format!("/api/v1/devices/{dev}/lock")).await;
    assert_eq!(status, StatusCode::OK);

    for action in ["restart", "shutdown", "sleep"] {
        let (status, body) = bob.post_json(&power(dev), &json!({ "action": action })).await;
        assert_eq!(status, StatusCode::CONFLICT, "{action}: {body}");
        assert_eq!((body["error"]["code"].clone(), body["error"]["holder"].clone()), (json!("control_lock_held"), json!("alice")));
    }
    assert!(agent.platform.power_actions().is_empty());

    // Lock screen needs no Control Lock.
    let (status, body) = bob.post_json(&power(dev), &json!({ "action": "lock" })).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = alice.post_json(&power(dev), &json!({ "action": "sleep" })).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    // Server roles do not bypass the Control Lock: only the holder may restart, shut down or sleep.
    let (status, body) = sys.api.post_json(&power(dev), &json!({ "action": "shutdown" })).await;
    assert_eq!(status, StatusCode::CONFLICT, "admin: {body}");
    assert_eq!(body["error"]["holder"], "alice");
    assert_eq!(agent.platform.power_actions(), vec![PowerAction::Lock, PowerAction::Sleep]);
}

#[tokio::test]
async fn without_a_control_lock_anyone_may_power_off_and_bad_actions_are_rejected() {
    let sys = TestSystem::start().await;
    let agent = sys.start_agent("lab-pc").await;
    let dev = agent.device_id();
    sys.wait_device(dev, |d| d["status"] == "Online").await;
    let bob = sys.api.with_token(Some(&sys.create_user("bob").await));

    let (status, body) = bob.post_json(&power(dev), &json!({ "action": "hibernate" })).await;
    assert_eq!((status, body["error"]["code"].clone()), (StatusCode::BAD_REQUEST, json!("bad_request")));
    let (status, body) = bob.post_json(&power(dev), &json!({ "action": "shutdown" })).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(agent.platform.power_actions(), vec![PowerAction::Shutdown]);
}

#[tokio::test]
async fn rcli_power_runs_the_action_and_prints_the_lock_holder_when_refused() {
    let sys = TestSystem::start().await;
    let agent = sys.start_agent("lab-pc").await;
    sys.wait_device(agent.device_id(), |d| d["status"] == "Online").await;
    let alice = sys.create_user("alice").await;
    let bob = sys.create_user("bob").await;

    let out = sys.rcli_as(&bob, &["power", "lab-pc", "lock"]).await.unwrap();
    assert_eq!(out, "lab-pc: lock sent\n");
    sys.rcli_as(&alice, &["lock", "acquire", "lab-pc"]).await.unwrap();
    let err = sys.rcli_as(&bob, &["power", "lab-pc", "restart"]).await.unwrap_err();
    assert!(err.to_string().contains("alice"), "{err}");
    assert_eq!(agent.platform.power_actions(), vec![PowerAction::Lock]);
}
