//! The Agent's local IPC: `rcli agent ...` talks to the running service's socket, not the Server.

use rcli_testkit::{eventually, temp_dir, TestSystem};

#[tokio::test]
async fn the_device_password_is_ten_characters_without_look_alikes() {
    let sys = TestSystem::start().await;
    let dir = temp_dir("password");
    let agent = sys.start_agent_with_socket("lab-pc", &dir).await;
    let password = agent.device_password().expect("a Device Password is generated at registration");
    assert_eq!(password.chars().count(), 10, "{password}");
    assert!(!password.chars().any(|c| "0Oo1lIL".contains(c)), "{password}");
    assert!(password.chars().all(|c| c.is_ascii_alphanumeric()), "{password}");
}

#[tokio::test]
async fn the_password_is_not_stored_in_plaintext_on_the_device() {
    let sys = TestSystem::start().await;
    let dir = temp_dir("password-at-rest");
    let agent = sys.start_agent_with_socket("lab-pc", &dir).await;
    let password = agent.device_password().unwrap();
    let mut checked = 0;
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if !path.is_file() {
            continue; // the IPC socket
        }
        let bytes = std::fs::read(&path).unwrap();
        assert!(!bytes.windows(password.len()).any(|w| w == password.as_bytes()), "plaintext password in {}", path.display());
        checked += 1;
    }
    assert!(checked >= 2, "the Secret Store key and the sealed password exist");
}

#[tokio::test]
async fn password_show_is_refused_to_non_root() {
    let sys = TestSystem::start().await;
    let dir = temp_dir("password-show");
    let agent = sys.start_agent_with_socket("lab-pc", &dir).await;
    let socket = dir.join("agent.sock");
    rcli_testkit::eventually(|| async { socket.exists().then_some(()) }).await;
    let out = sys.agent_rcli(&socket, &["agent", "password", "show"]).await;
    if rcli_testkit::running_as_root() {
        // Root may read it; that is the documented escape hatch, not the rule under test.
        assert_eq!(out.unwrap().trim(), agent.device_password().unwrap());
    } else {
        let err = out.unwrap_err();
        assert!(err.to_string().contains("root"), "{err}");
    }
}

#[tokio::test]
async fn agent_info_shows_the_device_id_and_connection_status_to_any_local_user() {
    let sys = TestSystem::start().await;
    let dir = temp_dir("info");
    let agent = sys.start_agent_with_socket("lab-pc", &dir).await;
    sys.wait_device(agent.device_id(), |d| d["status"] == "Online").await;

    let socket = dir.join("agent.sock");
    let out = eventually(|| async {
        let out = sys.agent_rcli(&socket, &["agent", "info"]).await.ok()?;
        out.contains("connected").then_some(out)
    })
    .await;
    assert!(out.contains(agent.device_id()), "{out}");

    let json: serde_json::Value = serde_json::from_str(&sys.agent_rcli(&socket, &["agent", "info", "--json"]).await.unwrap()).unwrap();
    assert_eq!(json["device_id"], agent.device_id());
    assert_eq!(json["connected"], true);
}

#[tokio::test]
async fn agent_info_shows_the_capture_permissions_the_screen_side_has() {
    let sys = TestSystem::start().await;
    let dir = temp_dir("info-permissions");
    let platform = rcli_agent::platform::FakePlatform::new("mac");
    let granted = rcli_agent::platform::CapturePermissions { screen_recording: false, accessibility: true };
    platform.update(|s| s.capture_permissions = Some(granted));
    let _agent = sys.start_agent_trusting_socket_on(platform, &dir).await;

    let socket = dir.join("agent.sock");
    let json = eventually(|| async {
        let out = sys.agent_rcli(&socket, &["agent", "info", "--json"]).await.ok()?;
        serde_json::from_str::<serde_json::Value>(&out).ok()
    })
    .await;
    assert_eq!(json["capture_permissions"], serde_json::json!({ "screen_recording": false, "accessibility": true }));
}

/// Unix socket modes; Windows named pipes use a DACL instead (see `crates/agent/src/ipc.rs`).
#[cfg(unix)]
#[tokio::test]
async fn the_socket_is_open_to_every_local_user() {
    use std::os::unix::fs::PermissionsExt;
    let sys = TestSystem::start().await;
    let dir = temp_dir("socket-mode");
    let _agent = sys.start_agent_with_socket("lab-pc", &dir).await;
    let socket = dir.join("agent.sock");
    // A root service's umask would leave the socket 0755, so other users could not even run `rcli agent info`.
    let mode = eventually(|| async { Some(std::fs::metadata(&socket).ok()?.permissions().mode() & 0o777).filter(|m| *m == 0o666) }).await;
    assert_eq!(mode, 0o666);
}

/// Ticket 16 (ADR 0013): a program running as the Device Owner changes the Device Password only with a fresh OS
/// authentication, and the CLI asks for it on a terminal, refusing without one.
#[cfg(unix)]
#[tokio::test]
async fn the_device_owner_changes_the_device_password_only_with_the_os_password() {
    use rcli_agent::{
        ipc::{self, OsAuth, Request},
        platform::FakePlatform,
    };
    let sys = TestSystem::start().await;
    let dir = temp_dir("owner-os-password");
    let platform = FakePlatform::new("laptop");
    platform.update(|s| s.os_password = "os-secret".into());
    let agent = sys.start_agent_owned_by_me_on(platform.clone(), &dir).await;
    sys.wait_device(agent.device_id(), |d| d["status"] == "Online").await;
    let socket = dir.join("agent.sock");
    let stored = || agent.status.secrets().get(ipc::DEVICE_PASSWORD).unwrap();
    let before = stored();

    // The tests have no terminal: the CLI refuses rather than change anything.
    let err = sys.agent_rcli(&socket, &["agent", "password", "set", "--password", "brand-new-1"]).await.unwrap_err();
    assert!(err.to_string().contains("terminal"), "{err}");

    let set = Request::PasswordSet { password: "brand-new-1".into() };
    let err = ipc::send(&socket, &set).await.unwrap_err();
    assert!(err.to_string().contains("OS account password"), "{err}");
    let err = ipc::send_authenticated(&socket, &set, &OsAuth::Password("guess".into())).await.unwrap_err();
    assert!(err.to_string().contains("OS account password"), "{err}");
    // The OS dialog (the GUI's way): the person at the Device cancels it.
    ipc::send_authenticated(&socket, &set, &OsAuth::Dialog).await.unwrap_err();
    assert_eq!(platform.os_dialogs().len(), 1);
    assert_eq!(stored(), before);

    ipc::send_authenticated(&socket, &set, &OsAuth::Password("os-secret".into())).await.unwrap();
    assert_eq!(stored().as_deref(), Some("brand-new-1"));

    // Opening the Device to others is an owner change too; refusing and narrowing are not.
    for opens in [
        Request::RootGrant { who: "alice".into() },
        Request::IdSet { device_id: "123456789".into() },
        Request::PromptAnswer { id: 1, accept: true },
        Request::CapabilitiesSet { who: "alice".into(), mouse_keyboard: Some(true), terminal: None, power: None },
    ] {
        let err = ipc::send(&socket, &opens).await.unwrap_err();
        assert!(err.to_string().contains("OS account password"), "{opens:?}: {err}");
    }
    let err = ipc::send(&socket, &Request::PromptAnswer { id: 1, accept: false }).await.unwrap_err();
    assert!(err.to_string().contains("no prompt 1"), "{err}");
}

/// Ticket 19: the main window reads the Install Mode and the General settings from `info`, and the Device Owner
/// switches auto-update and the Session Window with the OS password, like any Device Setting.
#[cfg(unix)]
#[tokio::test]
async fn the_device_owner_reads_the_install_mode_and_switches_the_general_settings() {
    use rcli_agent::{
        ipc::{self, OsAuth, Request},
        platform::FakePlatform,
    };
    let sys = TestSystem::start().await;
    let dir = temp_dir("owner-general");
    let platform = FakePlatform::new("laptop");
    platform.update(|s| {
        s.os_password = "os-secret".into();
        s.user_mode = Some("me".into());
    });
    let _agent = sys.start_agent_owned_by_me_on(platform, &dir).await;
    let socket = dir.join("agent.sock");
    let info = ipc::send(&socket, &Request::Info).await.unwrap();
    assert_eq!(info["install_mode"], "user", "{info}");
    assert_eq!((&info["settings"]["auto_update"], &info["settings"]["session_window"]), (&true.into(), &true.into()), "{info}");

    let off = Request::GeneralSet { auto_update: Some(false), session_window: Some(false) };
    let err = ipc::send(&socket, &off).await.unwrap_err();
    assert!(err.to_string().contains("OS account password"), "{err}");
    ipc::send_authenticated(&socket, &off, &OsAuth::Password("os-secret".into())).await.unwrap();
    let info = ipc::send(&socket, &Request::Info).await.unwrap();
    assert_eq!((&info["settings"]["auto_update"], &info["settings"]["session_window"]), (&false.into(), &false.into()), "{info}");
}

/// `rcli <args>` as the local user whose config is `dir`, on a Device with no Server, against the Agent at `socket`.
async fn local_rcli(dir: &std::path::Path, socket: &std::path::Path, args: &[&str]) -> anyhow::Result<String> {
    let mut argv = vec!["rcli", "--root", dir.to_str().unwrap(), "--config-dir", dir.to_str().unwrap()];
    argv.extend(["--agent-socket", socket.to_str().unwrap()]);
    argv.extend_from_slice(args);
    let mut out = Vec::new();
    rcli::run(argv, &mut out).await?;
    Ok(String::from_utf8(out)?)
}

/// Ticket 17: with no Server configured, the Device Owner (the OS user running the Agent, here this test) observes
/// the Local Device with Marks and clicks one: no Device Password, no Control Lock.
#[tokio::test]
async fn the_device_owner_observes_and_clicks_the_local_device_with_no_server() {
    let platform = rcli_testkit::FakePlatform::new("laptop");
    let button = |text: &str, x| rcli_testkit::UiElement { role: "button".into(), text: text.into(), x, y: 600, width: 150, height: 60, ..Default::default() };
    platform.update(|s| s.accessibility = Some(vec![button("OK", 300), button("Cancel", 600)]));
    let (dir, socket) = rcli_testkit::serve_unregistered(platform.clone());

    let out = dir.to_str().unwrap();
    let shot = eventually(|| async { local_rcli(&dir, &socket, &["shot", "local", "--marks", "--out", out]).await.ok() }).await;
    assert!(shot.contains("OK") && shot.contains("Cancel"), "{shot}");
    let obs: serde_json::Value = serde_json::from_str(&local_rcli(&dir, &socket, &["--json", "shot", "local", "--marks"]).await.unwrap()).unwrap();
    assert_eq!(obs["marks"][1]["text"], "Cancel", "{}", obs["marks"]);

    let observation = obs["observation"].to_string();
    local_rcli(&dir, &socket, &["act", "local", "click", "--mark", "2", "--observation", &observation, "--observe", "none"]).await.unwrap();
    assert!(platform.inputs().contains(&rcli_testkit::Input::Move { display: 0, x: 675, y: 630 }), "{:?}", platform.inputs());
    // Ticket 21: local control is no connection, so it opens no Session Window.
    let info = rcli_agent::ipc::send(&socket, &rcli_agent::ipc::Request::Info).await.unwrap();
    assert_eq!(info["connections"], serde_json::json!([]), "{info}");

    // Decision 10: in User Mode the lock screen is out of reach, locally as remotely.
    platform.update(|s| s.user_mode = Some("me".into()));
    platform.set_session_state(rcli_agent::platform::SessionState::Locked);
    let err = local_rcli(&dir, &socket, &["shot", "local"]).await.unwrap_err();
    assert!(err.to_string().contains("Service Mode"), "{err}");
}

/// Ticket 17: local control is for the Device Owner; another OS user is refused.
#[tokio::test]
async fn another_os_user_cannot_control_the_local_device() {
    let dir = temp_dir("local-someone-else");
    let socket = dir.join("agent.sock");
    let secrets = std::sync::Arc::new(rcli_agent::secret_store::FileSecretStore::new(dir.clone()));
    let status = rcli_agent::ipc::Status::new("", rcli_agent::ServerAccess { addr: String::new(), server_key: String::new() }, secrets);
    status.set_platform(std::sync::Arc::new(rcli_testkit::FakePlatform::new("laptop")));
    tokio::spawn(rcli_testkit::serve_ipc_for_someone_else(socket.clone(), std::sync::Arc::new(status)));
    eventually(|| async { socket.exists().then_some(()) }).await;

    let err = local_rcli(&dir, &socket, &["shot", "local"]).await.unwrap_err();
    if !rcli_testkit::running_as_root() {
        assert!(err.to_string().contains("Device Owner"), "{err}");
    }
}

/// Service Mode: the user logged in at the Device's screen controls the Local Device, and makes no owner change.
#[cfg(unix)]
#[tokio::test]
async fn the_user_at_the_screen_controls_the_local_device_in_service_mode() {
    use std::os::unix::fs::MetadataExt;
    let dir = temp_dir("local-console-user");
    let socket = dir.join("agent.sock");
    let secrets = std::sync::Arc::new(rcli_agent::secret_store::FileSecretStore::new(dir.clone()));
    let status = rcli_agent::ipc::Status::new("", rcli_agent::ServerAccess { addr: String::new(), server_key: String::new() }, secrets);
    status.set_platform(std::sync::Arc::new(rcli_testkit::FakePlatform::new("laptop")));
    let me = std::fs::metadata(&dir).unwrap().uid();
    tokio::spawn(rcli_agent::ipc::serve_at_the_console_of(socket.clone(), std::sync::Arc::new(status), me));
    eventually(|| async { socket.exists().then_some(()) }).await;

    local_rcli(&dir, &socket, &["shot", "local", "--out", dir.to_str().unwrap()]).await.unwrap();
    if !rcli_testkit::running_as_root() {
        let err = rcli_agent::ipc::send(&socket, &rcli_agent::ipc::Request::PasswordShow).await.unwrap_err();
        assert!(err.to_string().contains("Device Owner"), "{err}");
    }
}
