use rcli_testkit::{eventually, temp_dir, TestSystem};

#[tokio::test]
async fn rcli_agent_run_registers_and_connects_a_device() {
    let sys = TestSystem::start().await;
    let key = sys.rcli(&["server-key", "show"]).await.unwrap();
    let dir = temp_dir("agent-run");
    let argv = [
        "rcli", "agent", "run",
        "--server", &sys.agent_addr(),
        "--key", key.trim(),
        "--state-dir", dir.to_str().unwrap(),
        "--name", "lab-pc",
    ];
    let mut out = Vec::new();
    let agent = rcli::run(argv, &mut out);
    tokio::pin!(agent);
    let online = async {
        eventually(|| async {
            // The Device as a Controller reaches it: with the Device ID and Device Password the Agent keeps.
            use rcli_agent::secret_store::SecretStore;
            let creds: serde_json::Value = serde_json::from_slice(&std::fs::read(dir.join("credentials.json")).ok()?).ok()?;
            let id = creds["device_id"].as_str()?;
            let password = rcli_agent::secret_store::FileSecretStore::new(dir.clone()).get(rcli_agent::ipc::DEVICE_PASSWORD).ok()??;
            let (_, device) = sys.device_client(id, &password).get(&format!("/api/v1/devices/{id}")).await;
            // The version lands with the Agent's hello, a moment after Online (later on a real OS adapter).
            (device["status"] == "Online" && !device["agent_version"].is_null()).then_some(device)
        })
        .await
    };
    let device = tokio::select! {
        r = &mut agent => panic!("the Agent exited: {r:?}"),
        d = online => d,
    };
    assert_eq!(device["agent_version"], env!("CARGO_PKG_VERSION"));
    assert!(dir.join("credentials.json").exists(), "the Agent keeps its Device credentials");
}

#[tokio::test]
async fn rcli_devices_shows_device_status_as_text_and_json() {
    let sys = TestSystem::start().await;
    let creds = sys.register(&rcli_testkit::FakePlatform::new("lab-pc")).await.unwrap();
    let mut agent = rcli_testkit::TestAgent::new(sys.server_access(), rcli_testkit::FakePlatform::new("lab-pc"), creds);
    agent.start();
    sys.wait_device(agent.device_id(), |d| d["status"] == "Online").await;
    let dir = rcli_testkit::temp_dir("devices-json");
    sys.controller_config(&dir, Some((agent.device_id(), &agent.device_password().unwrap()))).await;

    let text = sys.rcli_in(&dir, &["devices"]).await.unwrap();
    let row = text.lines().find(|l| l.contains(agent.device_id())).expect(&text);
    assert!(row.contains("Online"), "{text}");

    let json: serde_json::Value = serde_json::from_str(&sys.rcli_in(&dir, &["devices", "--json"]).await.unwrap()).unwrap();
    assert_eq!(json[0]["device_id"], agent.device_id());
    assert_eq!(json[0]["name"], "", "no name was given in this Address Book");
    assert_eq!(json[0]["status"], "Online");
    assert!(json[0]["last_seen_at"].is_null());
}

#[tokio::test]
async fn rcli_reports_typed_api_errors() {
    let sys = TestSystem::start().await;
    let base = format!("http://{}", sys.server.http_addr);
    let mut out = Vec::new();
    let err = rcli::run(["rcli", "--server", &base, "--token", "bad", "user", "list"], &mut out).await.unwrap_err();
    assert!(err.to_string().contains("unauthorized"), "{err}");
}
