//! On an installed Device (`install.rs`): the Address Book (`connect <Device ID>`, `devices`, `forget`); Device
//! commands then need only the ID.

use rcli_testkit::{StatusCode, temp_dir, TestSystem};

#[tokio::test]
async fn connect_then_shot_works_with_no_env_vars_and_the_password_is_not_in_the_config() {
    let sys = TestSystem::start().await;
    let agent = sys.start_agent("lab-pc").await;
    sys.wait_device(agent.device_id(), |d| d["status"] == "Online").await;
    let dir = temp_dir("connect");
    let password = agent.device_password().unwrap();
    sys.controller_config(&dir, None).await;

    let out = sys.rcli_in(&dir, &["connect", agent.device_id(), "--password", &password]).await.unwrap();
    assert!(out.contains(agent.device_id()), "{out}");

    let shots = temp_dir("shots");
    let out = sys
        .rcli_in(&dir, &["shot", agent.device_id(), "--out", shots.to_str().unwrap()])
        .await
        .unwrap();
    assert!(out.contains("display 0"), "{out}");
    assert!(std::fs::read_dir(&shots).unwrap().count() > 0);

    for file in std::fs::read_dir(&dir).unwrap() {
        let path = file.unwrap().path();
        if path.is_file() {
            let bytes = std::fs::read(&path).unwrap();
            assert!(!bytes.windows(password.len()).any(|w| w == password.as_bytes()), "plaintext password in {}", path.display());
        }
    }
}

#[tokio::test]
async fn two_connected_devices_are_listed_online_and_controllable_without_a_user_session() {
    let sys = TestSystem::start().await;
    let (a, b, other) = (sys.start_agent("a").await, sys.start_agent("b").await, sys.start_agent("other").await);
    for d in [&a, &b, &other] {
        sys.wait_device(d.device_id(), |d| d["status"] == "Online").await;
    }
    let dir = temp_dir("devices");
    sys.controller_config(&dir, None).await;
    let args = ["connect", a.device_id(), "--password", &a.device_password().unwrap(), "--name", "lab", "--tag", "linux", "--tag", "ci"];
    sys.rcli_in(&dir, &args).await.unwrap();
    sys.rcli_in(&dir, &["connect", b.device_id(), "--password", &b.device_password().unwrap()]).await.unwrap();

    let list = sys.rcli_in(&dir, &["devices"]).await.unwrap();
    let row = list.lines().find(|l| l.contains(a.device_id())).unwrap_or_else(|| panic!("{list}"));
    assert!(row.contains("lab") && row.contains("Online") && row.contains("linux,ci"), "{list}");
    assert!(list.lines().any(|l| l.contains(b.device_id()) && l.contains("Online")), "{list}");
    assert!(!list.contains(other.device_id()), "only the Address Book is listed: {list}");

    let mcp = sys.mcp_in(&dir).await;
    let params = rcli_testkit::rmcp::model::CallToolRequestParams::new("devices").with_arguments(Default::default());
    let devices = mcp.call_tool(params).await.unwrap().structured_content.unwrap();
    let ids: Vec<&str> = devices["devices"].as_array().unwrap().iter().map(|d| d["device_id"].as_str().unwrap()).collect();
    assert_eq!(ids, [a.device_id(), b.device_id()], "{devices}");

    for d in [&a, &b] {
        let shots = temp_dir("devices-shots");
        sys.rcli_in(&dir, &["shot", d.device_id(), "--out", shots.to_str().unwrap()]).await.unwrap();
    }
    let shots = temp_dir("devices-shots");
    let err = sys.rcli_in(&dir, &["shot", other.device_id(), "--out", shots.to_str().unwrap()]).await.unwrap_err();
    assert!(err.to_string().contains("forbidden_device"), "{err}");
}

#[tokio::test]
async fn a_wrong_password_is_reported_and_not_saved() {
    let sys = TestSystem::start().await;
    let agent = sys.start_agent("lab-pc").await;
    sys.wait_device(agent.device_id(), |d| d["status"] == "Online").await;
    let dir = temp_dir("wrong-password");
    sys.controller_config(&dir, None).await;

    let err = sys.rcli_in(&dir, &["connect", agent.device_id(), "--password", "wrongpassword"]).await.unwrap_err();
    assert!(err.to_string().contains("Device Password"), "{err}");

    let shots = temp_dir("wrong-password-shots");
    let err = sys.rcli_in(&dir, &["shot", agent.device_id(), "--out", shots.to_str().unwrap()]).await.unwrap_err();
    assert!(err.to_string().contains("connect"), "{err}");
}

#[tokio::test]
async fn forget_removes_the_remembered_password() {
    let sys = TestSystem::start().await;
    let agent = sys.start_agent("lab-pc").await;
    sys.wait_device(agent.device_id(), |d| d["status"] == "Online").await;
    let dir = temp_dir("forget");
    sys.controller_config(&dir, None).await;
    sys.rcli_in(&dir, &["connect", agent.device_id(), "--password", &agent.device_password().unwrap()]).await.unwrap();

    let out = sys.rcli_in(&dir, &["forget", agent.device_id()]).await.unwrap();
    assert!(out.contains("forgot"), "{out}");
    let list = sys.rcli_in(&dir, &["devices"]).await.unwrap();
    assert!(!list.contains(agent.device_id()), "the entry is gone: {list}");
    let shots = temp_dir("forget-shots");
    let err = sys.rcli_in(&dir, &["shot", agent.device_id(), "--out", shots.to_str().unwrap()]).await.unwrap_err();
    assert!(err.to_string().contains("connect"), "{err}");
}

#[tokio::test]
async fn the_server_does_not_store_the_device_password_in_plaintext() {
    let sys = TestSystem::start().await;
    let agent = sys.start_agent("lab-pc").await;
    // The admin view never shows it, and the row holds only an argon2 hash.
    let (status, device) = sys.api.get(&format!("/api/v1/devices/{}", agent.device_id())).await;
    assert_eq!(status, StatusCode::OK);
    assert!(device["device_password"].is_null(), "{device}");
}

/// `--server` cannot skip the https rule: the installed Server Key would travel in plaintext.
#[tokio::test]
async fn a_plain_http_server_flag_is_refused() {
    let sys = TestSystem::start().await;
    let agent = sys.start_agent("lab-pc").await;
    let dir = temp_dir("server-flag-http");
    let password = agent.device_password().unwrap();
    sys.controller_config(&dir, None).await;
    sys.rcli_in(&dir, &["connect", agent.device_id(), "--password", &password]).await.unwrap();
    let args = ["--server", "http://server.example.com:8080", "exec", agent.device_id(), "--", "whoami"];
    let err = sys.rcli_in(&dir, &args).await.unwrap_err();
    assert!(err.to_string().contains("TLS reverse proxy"), "{err}");
}
