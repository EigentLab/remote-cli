//! Wake (ADR 0011): an Address Book entry keeps the Device's MAC and subnet from its last connection; the Server picks
//! an Online Agent on that subnet as Wake Relay from what it holds in memory.

use rcli_testkit::{FakePlatform, StatusCode, TestAgent, TestSystem, platform::NetworkInterface};
use serde_json::json;

const TARGET_MAC: [u8; 6] = [0xaa, 0xbb, 0xcc, 0x00, 0x00, 0x01];

fn on_lan(hostname: &str, mac: &str, ip: &str) -> FakePlatform {
    let p = FakePlatform::new(hostname);
    p.update(|s| {
        s.inventory.network_interfaces = vec![
            NetworkInterface { name: "lo".into(), mac: "00:00:00:00:00:00".into(), ips: vec!["127.0.0.1/8".into()] },
            NetworkInterface { name: "eth0".into(), mac: mac.into(), ips: vec![ip.into(), "fe80::1/64".into()] },
        ]
    });
    p
}

/// An Online relay on 192.168.1.0/24 and one elsewhere, waiting until the Server holds their LANs.
async fn relays(sys: &TestSystem) -> (TestAgent, TestAgent) {
    let relay = sys.start_agent_on(on_lan("relay", "02:00:00:00:00:05", "192.168.1.5/24")).await;
    let elsewhere = sys.start_agent_on(on_lan("elsewhere", "02:00:00:00:00:09", "10.0.0.5/24")).await;
    for a in [&relay, &elsewhere] {
        sys.wait_device_details(a.device_id(), |d| !d["wake"].is_null()).await;
    }
    (relay, elsewhere)
}

async fn online_target(sys: &TestSystem) -> TestAgent {
    let target = sys.start_agent_on(on_lan("target", "AA:BB:CC:00:00:01", "192.168.1.20/24")).await;
    sys.wait_device_details(target.device_id(), |d| !d["wake"].is_null()).await;
    target
}

async fn take_offline(sys: &TestSystem, target: &mut TestAgent) {
    let id = target.device_id().to_owned();
    target.stop();
    sys.wait_device(&id, |d| !d["last_seen_at"].is_null()).await;
    sys.advance(31);
    sys.wait_device(&id, |d| d["status"] == "Offline").await;
}

#[tokio::test]
async fn an_offline_device_in_the_cli_address_book_is_woken_through_a_relay_on_its_subnet() {
    let sys = TestSystem::start().await;
    let (relay, elsewhere) = relays(&sys).await;
    let mut target = online_target(&sys).await;
    let id = target.device_id().to_owned();
    let dir = rcli_testkit::temp_dir("wake-cli");
    sys.controller_config(&dir, Some((&id, &target.device_password().unwrap()))).await;

    // Before going Offline the Device stops reporting its LAN, so the Server holds nothing to wake it with: only the
    // Address Book's MAC and subnet from the connection above can.
    target.platform.update(|s| s.inventory.network_interfaces.clear());
    sys.wait_device_details(&id, |d| d["inventory"]["network_interfaces"].as_array().is_some_and(|n| n.is_empty())).await;
    take_offline(&sys, &mut target).await;

    let out = sys.rcli_in(&dir, &["wake", &id]).await.unwrap();
    assert!(out.contains("relay") && out.contains("aa:bb:cc:00:00:01"), "{out}");
    assert_eq!(relay.platform.wakes_sent(), vec![TARGET_MAC]);
    assert!(elsewhere.platform.wakes_sent().is_empty());
    target.start();
    sys.wait_device(&id, |d| d["status"] == "Online").await;
}

#[tokio::test]
async fn an_offline_device_in_a_users_address_book_is_woken_through_a_relay_on_its_subnet() {
    let sys = TestSystem::start().await;
    let (relay, _elsewhere) = relays(&sys).await;
    let mut target = online_target(&sys).await;
    let alice = sys.api.with_token(Some(&sys.create_user("alice").await));
    let add = json!({ "device_id": target.device_id(), "password": target.device_password().unwrap() });
    assert_eq!(alice.post_json("/api/v1/address-book", &add).await.0, StatusCode::CREATED);
    take_offline(&sys, &mut target).await;

    let (status, body) = alice.post(&format!("/api/v1/devices/{}/wake", target.device_id())).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!((&body["relay"]["name"], &body["mac"]), (&json!("relay"), &json!("aa:bb:cc:00:00:01")));
    assert_eq!(relay.platform.wakes_sent(), vec![TARGET_MAC]);
}

#[tokio::test]
async fn wake_says_why_it_cannot() {
    let sys = TestSystem::start().await;
    let (relay, elsewhere) = relays(&sys).await;
    let mut target = online_target(&sys).await;
    let id = target.device_id().to_owned();
    let client = sys.device_client(&id, &target.device_password().unwrap());
    let wake = format!("/api/v1/devices/{id}/wake");

    let (status, body) = client.post(&wake).await;
    assert_eq!((status, &body["error"]["code"]), (StatusCode::CONFLICT, &json!("device_online")), "{body}");
    take_offline(&sys, &mut target).await;

    // No MAC and subnet given, and no User's Address Book to read them from.
    let (status, body) = client.post(&wake).await;
    assert_eq!((status, &body["error"]["code"]), (StatusCode::CONFLICT, &json!("no_wake_address")), "{body}");
    assert!(!body["error"]["hint"].as_str().unwrap().contains("read the message"), "{body}");

    let far = json!({ "mac": "aa:bb:cc:00:00:01", "subnet": "172.16.0.0/16" });
    let (status, body) = client.post_json(&wake, &far).await;
    assert_eq!((status, &body["error"]["code"]), (StatusCode::CONFLICT, &json!("no_wake_relay")), "{body}");
    assert!(body["error"]["message"].as_str().unwrap().contains("172.16.0.0/16"), "{body}");
    assert!(relay.platform.wakes_sent().is_empty() && elsewhere.platform.wakes_sent().is_empty());
}
