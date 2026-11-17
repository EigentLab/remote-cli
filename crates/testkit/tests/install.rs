//! `rcli install` makes a machine both a Device and a Controller (ADR 0006): it registers the Device, prints its
//! Device ID and Device Password, and writes the Controller configuration machine-wide, so every local user can run
//! `rcli connect` and `rcli mcp` at once. Each test installs into a temp directory (`rcli --root`), not `/`.

use rcli_testkit::rmcp::model::CallToolRequestParams;
use rcli_testkit::{temp_dir, TestSystem};
use serde_json::{json, Value};

/// The API Address an Admin set to this test Server (plain http is allowed only to loopback, as for the CLI).
async fn set_api_address(sys: &TestSystem) -> String {
    let url = format!("http://{}", sys.server.http_addr);
    sys.rcli(&["api-address", "set", &url]).await.unwrap();
    url
}

fn printed<'a>(out: &'a str, label: &str) -> &'a str {
    out.lines().find_map(|l| l.strip_prefix(label)).unwrap_or_else(|| panic!("no {label} in {out}")).trim()
}

#[tokio::test]
#[cfg_attr(any(target_os = "linux", target_os = "macos"), ignore = "Linux and macOS install in User Mode: the Controller configuration is per user (ADR 0012)")]
async fn after_install_every_local_user_connects_and_runs_mcp_with_no_setup() {
    let sys = TestSystem::start().await;
    set_api_address(&sys).await;
    let target = sys.start_agent("lab-pc").await;
    sys.wait_device(target.device_id(), |d| d["status"] == "Online").await;
    let password = target.device_password().unwrap();

    let machine = temp_dir("desk");
    let out = sys.install(&machine).await.unwrap();
    // The machine is a Device too: the printed Device ID comes Online once its Agent runs.
    let device_id = printed(&out, "Device ID:").to_owned();
    assert!(printed(&out, "Device Password:").len() >= 8, "{out}");
    let _agent = sys.start_installed(&machine).await;
    sys.wait_device(&device_id, |d| d["status"] == "Online").await;

    for user in ["alice", "bob"] {
        let home = temp_dir(user);
        let out = sys.rcli_user(&machine, &home, &["connect", target.device_id(), "--password", &password]).await.unwrap();
        assert!(out.contains(target.device_id()), "{out}");
        let out = sys.rcli_user(&machine, &home, &["exec", target.device_id(), "--", "echo", "hi"]).await.unwrap();
        assert_eq!(out, "hi\n");

        let mcp = sys.mcp_user(&machine, &home).await;
        let args = json!({ "device": target.device_id(), "command": "echo hi" }).as_object().unwrap().clone();
        let r = mcp.call_tool(CallToolRequestParams::new("exec").with_arguments(args)).await.unwrap();
        assert_eq!(r.structured_content.as_ref().map(|v| v["stdout"].clone()), Some(Value::from("hi\n")), "{r:?}");
        mcp.cancel().await.unwrap();
    }
}

#[tokio::test]
async fn with_no_api_address_the_install_succeeds_and_controller_commands_say_why_until_the_agent_learns_it() {
    let sys = TestSystem::start().await;
    let target = sys.start_agent("lab-pc").await;
    sys.wait_device(target.device_id(), |d| d["status"] == "Online").await;
    let password = target.device_password().unwrap();
    let machine = temp_dir("early");
    let out = sys.install(&machine).await.unwrap();
    assert!(out.contains("Device ID:") && out.contains("no API Address yet"), "{out}");
    let mut agent = sys.start_installed(&machine).await;

    for args in [vec!["connect", target.device_id(), "--password", &password], vec!["devices"]] {
        let err = sys.rcli_in(&machine, &args).await.unwrap_err();
        assert!(err.to_string().contains("the Server has no API Address yet"), "{args:?}: {err}");
    }

    // An Admin sets it; the Agent gets it on its next connection and keeps the machine-wide configuration current.
    set_api_address(&sys).await;
    agent.stop();
    agent.start();
    rcli_testkit::eventually(|| async {
        sys.rcli_in(&machine, &["connect", target.device_id(), "--password", &password]).await.ok()
    })
    .await;
    let out = sys.rcli_in(&machine, &["exec", target.device_id(), "--", "echo", "hi"]).await.unwrap();
    assert_eq!(out, "hi\n");
}

/// Old instructions fail loudly instead of half-working: no alias (ADR 0006).
#[tokio::test]
async fn setup_and_agent_install_and_uninstall_no_longer_exist() {
    let sys = TestSystem::start().await;
    let machine = temp_dir("old-commands");
    let (server, key) = (sys.agent_addr(), sys.server_key());
    for args in [
        vec!["setup", "--server", &server, "--key", &key],
        vec!["agent", "install", "--server", &server, "--key", &key],
        vec!["agent", "uninstall"],
    ] {
        let err = sys.rcli_in(&machine, &args).await.unwrap_err();
        assert!(err.to_string().contains("unrecognized subcommand"), "{args:?}: {err}");
    }
    assert_eq!(files(&machine), Vec::<std::path::PathBuf>::new());
}

/// Every regular file under `dir`.
fn files(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut found = vec![];
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        match std::fs::symlink_metadata(&path).unwrap().is_dir() {
            true => found.extend(files(&path)),
            false => found.push(path),
        }
    }
    found
}

/// How many Devices the Server holds; no API lists them.
async fn device_count(sys: &TestSystem) -> i64 {
    let db = sqlx::PgPool::connect(&sys.config.database_url).await.unwrap();
    sqlx::query_scalar("SELECT count(*) FROM devices").fetch_one(&db).await.unwrap()
}

#[tokio::test]
async fn a_wrong_server_key_fails_the_install_and_leaves_nothing_behind() {
    let sys = TestSystem::start().await;
    set_api_address(&sys).await;
    let machine = temp_dir("typo");
    let (_, secret) = sys.server_key().rsplit_once('_').map(|(f, s)| (f.to_owned(), s.to_owned())).unwrap();
    let wrong = sys.server_key().replace(&secret, &"0".repeat(secret.len()));

    let err = sys.install_with_key(&machine, &wrong).await.unwrap_err();
    assert!(format!("{err:#}").contains("Server Key"), "{err:#}");
    assert_eq!(files(&machine), Vec::<std::path::PathBuf>::new(), "a failed install left files behind");
    assert_eq!(device_count(&sys).await, 0, "nothing registered");

    // A retry with the right key installs cleanly.
    let out = sys.install(&machine).await.unwrap();
    assert!(out.contains("Device ID:"), "{out}");
}

#[tokio::test]
async fn a_reinstall_with_a_wrong_server_key_fails_and_leaves_the_existing_install_untouched() {
    let sys = TestSystem::start().await;
    set_api_address(&sys).await;
    let machine = temp_dir("retype");
    sys.install(&machine).await.unwrap();
    let snapshot = |dir: &std::path::Path| files(dir).into_iter().map(|p| (std::fs::read(&p).unwrap(), p)).collect::<Vec<_>>();
    let before = snapshot(&machine);
    let (_, secret) = sys.server_key().rsplit_once('_').map(|(f, s)| (f.to_owned(), s.to_owned())).unwrap();
    let wrong = sys.server_key().replace(&secret, &"0".repeat(secret.len()));

    let err = sys.install_with_key(&machine, &wrong).await.unwrap_err();
    assert!(format!("{err:#}").contains("Server Key"), "{err:#}");
    assert!(snapshot(&machine) == before, "a failed reinstall changed the existing install");
    assert_eq!(device_count(&sys).await, 1);
}

#[tokio::test]
#[cfg_attr(any(target_os = "linux", target_os = "macos"), ignore = "Linux and macOS install in User Mode: the Controller configuration is per user (ADR 0012)")]
async fn uninstall_removes_the_agent_and_the_machine_wide_configuration_and_keeps_remembered_passwords() {
    let sys = TestSystem::start().await;
    set_api_address(&sys).await;
    let target = sys.start_agent("lab-pc").await;
    sys.wait_device(target.device_id(), |d| d["status"] == "Online").await;
    let machine = temp_dir("leaving");
    let out = sys.install(&machine).await.unwrap();
    let device_id = printed(&out, "Device ID:").to_owned();
    let _agent = sys.start_installed(&machine).await;
    sys.wait_device(&device_id, |d| d["status"] == "Online").await;
    let home = temp_dir("carol");
    sys.rcli_user(&machine, &home, &["connect", target.device_id(), "--password", &target.device_password().unwrap()]).await.unwrap();

    let out = sys.rcli_in(&machine, &["uninstall"]).await.unwrap();
    assert!(!out.contains("warning"), "{out}");

    // The Device is gone from the Server.
    rcli_testkit::eventually(|| async {
        let (status, _) = sys.api.get(&format!("/api/v1/devices/{device_id}")).await;
        (status == rcli_testkit::StatusCode::NOT_FOUND).then_some(())
    })
    .await;
    assert_eq!(files(&machine), Vec::<std::path::PathBuf>::new(), "uninstall left files behind");
    let err = sys.rcli_user(&machine, &home, &["devices"]).await.unwrap_err();
    assert!(err.to_string().contains("not installed"), "{err}");

    // Reinstalling brings Controller commands back with the password Carol remembered.
    sys.install(&machine).await.unwrap();
    let out = sys.rcli_user(&machine, &home, &["exec", target.device_id(), "--", "echo", "hi"]).await.unwrap();
    assert_eq!(out, "hi\n");
}

#[tokio::test]
#[cfg_attr(any(target_os = "linux", target_os = "macos"), ignore = "Linux and macOS install in User Mode: the Controller configuration is per user (ADR 0012)")]
async fn remembered_device_passwords_stay_with_the_user_who_connected() {
    let sys = TestSystem::start().await;
    set_api_address(&sys).await;
    let target = sys.start_agent("lab-pc").await;
    sys.wait_device(target.device_id(), |d| d["status"] == "Online").await;
    let password = target.device_password().unwrap();
    let machine = temp_dir("shared");
    sys.install(&machine).await.unwrap();
    let (alice, bob) = (temp_dir("alice"), temp_dir("bob"));
    sys.rcli_user(&machine, &alice, &["connect", target.device_id(), "--password", &password]).await.unwrap();

    // Bob shares the install, not Alice's connection.
    let err = sys.rcli_user(&machine, &bob, &["exec", target.device_id(), "--", "echo", "hi"]).await.unwrap_err();
    assert!(err.to_string().contains("rcli connect"), "{err}");

    // The machine-wide file is readable by every local user and holds no Device Password; Alice's files are hers.
    let config = rcli::config::machine_path(Some(&machine));
    let bytes = std::fs::read(&config).unwrap();
    assert!(!bytes.windows(password.len()).any(|w| w == password.as_bytes()), "a Device Password in the machine-wide file");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&config), 0o644, "read-only for local users");
        for file in std::fs::read_dir(alice.join("passwords")).unwrap() {
            let path = file.unwrap().path();
            assert_eq!(mode(&path), 0o600, "{} is readable by other local users", path.display());
        }
    }
}

/// Ticket 12: an install with no Server serves only local control. Once the Device Owner (the OS user running the
/// Agent, here this unprivileged test) sets a Server, the Device gets its Device ID and Device Password, and keeps
/// them and the Server across a restart.
#[tokio::test]
async fn an_agent_with_no_server_registers_once_its_device_owner_sets_one() {
    let sys = TestSystem::start().await;
    let state = temp_dir("no-server");
    let socket = state.join("agent.sock");
    let start = || {
        let config = rcli::agent::DeviceConfig {
            server: String::new(),
            key: String::new(),
            state_dir: state.clone(),
            controller_config: state.join("controller.json"),
            name: "laptop".into(),
            socket: socket.clone(),
            session_socket: None,
            session_pipe: None,
        };
        tokio::spawn(rcli::agent::serve_device_on(config, std::sync::Arc::new(rcli_testkit::FakePlatform::new("laptop"))))
    };
    use rcli_agent::ipc::{self, OsAuth, Request};
    let info = || async { serde_json::from_str::<Value>(&sys.agent_rcli(&socket, &["agent", "info", "--json"]).await.ok()?).ok() };
    let agent = start();
    let before = rcli_testkit::eventually(info).await;
    assert_eq!((&before["device_id"], &before["connected"]), (&json!(""), &json!(false)), "{before}");

    // An owner change: the Device Owner gives the OS password (ticket 16); FakePlatform's is "os-password".
    let os_password = OsAuth::Password("os-password".into());
    let set = Request::ServerSet { server: sys.agent_addr(), key: sys.server_key() };
    ipc::send_authenticated(&socket, &set, &os_password).await.unwrap();
    let id = rcli_testkit::eventually(|| async { info().await?["device_id"].as_str().filter(|id| !id.is_empty()).map(str::to_owned) }).await;
    let password = ipc::send_authenticated(&socket, &Request::PasswordShow, &os_password).await.unwrap();
    let password = password["password"].as_str().unwrap().to_owned();
    assert!(password.len() >= 8, "{password}");
    sys.add_to_address_book(&id, &password, "laptop").await;
    sys.wait_device(&id, |d| d["status"] == "Online").await;
    let controller: Value = serde_json::from_slice(&std::fs::read(state.join("controller.json")).unwrap()).unwrap();
    assert_eq!(controller["server"], sys.agent_addr());

    agent.abort();
    let _agent = start();
    rcli_testkit::eventually(|| async {
        let info = info().await?;
        (info["device_id"] == id.as_str() && info["connected"] == true && info["server"] == sys.agent_addr()).then_some(())
    })
    .await;
}

/// Ticket 15: `rcli service install` moves a User Mode install to Service Mode and `rcli service uninstall` moves it
/// back; the Device ID and Device Password stay the same, and the Server still knows one Device.
#[tokio::test]
#[cfg_attr(windows, ignore = "the Windows install seam installs Service Mode")]
async fn switching_to_service_mode_and_back_keeps_the_device_id_and_device_password() {
    let sys = TestSystem::start().await;
    let machine = temp_dir("switch");
    let identity = |out: &str| (printed(out, "Device ID:").to_owned(), printed(out, "Device Password:").to_owned());
    let before = identity(&sys.install(&machine).await.unwrap());

    let out = sys.rcli_in(&machine, &["service", "install"]).await.unwrap();
    assert_eq!(identity(&out), before, "{out}");
    let out = sys.rcli_in(&machine, &["service", "uninstall"]).await.unwrap();
    assert_eq!(identity(&out), before, "{out}");
    assert_eq!(device_count(&sys).await, 1);
}

/// The TLS proxy an IP address install puts in front of the Controller API: serves `cert_pem` on `listener` and
/// forwards to `backend`, telling it (as Caddy does) that the first request on each connection came over https.
fn https_front(listener: tokio::net::TcpListener, cert_pem: &str, key_pem: &str, backend: std::net::SocketAddr) {
    use rustls::pki_types::{pem::PemObject, CertificateDer, PrivateKeyDer};
    let chain = vec![CertificateDer::from_pem_slice(cert_pem.as_bytes()).unwrap()];
    let key = PrivateKeyDer::from_pem_slice(key_pem.as_bytes()).unwrap();
    let config = rustls::ServerConfig::builder().with_no_client_auth().with_single_cert(chain, key).unwrap();
    let acceptor = tokio_rustls::TlsAcceptor::from(std::sync::Arc::new(config));
    tokio::spawn(async move {
        while let Ok((tcp, _)) = listener.accept().await {
            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                let (Ok(mut tls), Ok(mut up)) = (acceptor.accept(tcp).await, tokio::net::TcpStream::connect(backend).await) else {
                    return;
                };
                let mut request_line = Vec::new();
                while !request_line.ends_with(b"\r\n") {
                    let Ok(byte) = tls.read_u8().await else { return };
                    request_line.push(byte);
                }
                request_line.extend_from_slice(b"X-Forwarded-Proto: https\r\n");
                if up.write_all(&request_line).await.is_err() {
                    return;
                }
                let _ = tokio::io::copy_bidirectional(&mut tls, &mut up).await;
            });
        }
    });
}

/// ADR 0015: a Server on an IP address serves the Controller API with a certificate from its CA; an install keeps
/// that CA (checked against the Server Key) and its Controller calls accept it, and no other CA's certificate.
#[tokio::test]
async fn an_install_calls_the_controller_api_on_an_ip_address_over_https_from_the_servers_ca() {
    let mut sys = TestSystem::start().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let api = format!("https://127.0.0.1:{}", listener.local_addr().unwrap().port());
    sys.rcli(&["api-address", "set", &api]).await.unwrap();
    // Told to, the Server writes a certificate for the API Address host at start.
    let files = temp_dir("https");
    let (cert, key) = (files.join("cert.pem"), files.join("key.pem"));
    sys.config.https_files = Some((cert.clone(), key.clone()));
    sys.restart(Some(TestSystem::master_key())).await;
    let read = |p: &std::path::Path| std::fs::read_to_string(p).unwrap();
    https_front(listener, &read(&cert), &read(&key), sys.server.http_addr);

    let target = sys.start_agent("lab-pc").await;
    sys.wait_device(target.device_id(), |d| d["status"] == "Online").await;
    let password = target.device_password().unwrap();
    let machine = temp_dir("ip-desk");
    sys.install(&machine).await.unwrap();
    let path = rcli::config::controller_path(&machine, Some(&machine));
    let mut installed = rcli::config::installed(&path).unwrap();
    assert_eq!(installed.api_address, api);
    let fingerprint = sys.server_key().split('_').nth(1).unwrap().to_owned();
    assert_eq!(rcli_agent::ca_fingerprint(&installed.ca_pem).unwrap(), fingerprint, "the install keeps the Server's CA");

    sys.rcli_in(&machine, &["connect", target.device_id(), "--password", &password]).await.unwrap();
    let out = sys.rcli_in(&machine, &["exec", target.device_id(), "--", "echo", "hi"]).await.unwrap();
    assert_eq!(out, "hi\n");

    // The same address served with a certificate from another CA is refused.
    let other = rcgen::KeyPair::generate().unwrap();
    let mut ca = rcgen::CertificateParams::new(vec![]).unwrap();
    ca.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    let ca = ca.self_signed(&other).unwrap();
    let leaf_key = rcgen::KeyPair::generate().unwrap();
    let leaf = rcgen::CertificateParams::new(vec!["127.0.0.1".to_owned()]).unwrap().signed_by(&leaf_key, &ca, &other).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    installed.api_address = format!("https://127.0.0.1:{}", listener.local_addr().unwrap().port());
    https_front(listener, &leaf.pem(), &leaf_key.serialize_pem(), sys.server.http_addr);
    rcli::config::save_machine(&path, &installed).unwrap();
    let err = sys.rcli_in(&machine, &["connect", target.device_id(), "--password", &password]).await.unwrap_err();
    assert!(format!("{err:?}").contains("UnknownIssuer"), "{err:?}");
}

/// ADR 0015, the Connect Guide's command for a Server whose HTTPS certificate comes from its CA: it writes the CA
/// from the Server Settings to a file for `curl`, and the scripted install then makes the machine a Device.
#[cfg(unix)]
#[tokio::test]
async fn the_connect_guide_command_carrying_the_ca_installs_from_a_server_on_an_ip_address() {
    let mut sys = TestSystem::start().await;
    let (_, settings) = sys.api.get("/api/v1/server-settings").await;
    assert_eq!(settings["https_ca_pem"], Value::Null, "a proxy with a certificate of its own: {settings}");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let api = format!("https://127.0.0.1:{}", listener.local_addr().unwrap().port());
    sys.rcli(&["api-address", "set", &api]).await.unwrap();
    let files = temp_dir("guide-https");
    let (cert, key) = (files.join("cert.pem"), files.join("key.pem"));
    sys.config.https_files = Some((cert.clone(), key.clone()));
    sys.restart(Some(TestSystem::master_key())).await;
    // The script installs against the Server Address (here with the restarted Server's Agent port).
    let (status, body) = sys.api.put_json("/api/v1/server-settings/server-address", &json!({ "address": sys.agent_addr() })).await;
    assert!(status.is_success(), "{body}");
    let read = |p: &std::path::Path| std::fs::read_to_string(p).unwrap();
    https_front(listener, &read(&cert), &read(&key), sys.server.http_addr);
    let (_, settings) = sys.api.get("/api/v1/server-settings").await;
    assert_eq!(settings["https_ca_pem"], sys.server.ca_pem.as_str(), "the certificate comes from the CA");

    // The Latest Build is a stand-in recording its arguments, replayed through the real `rcli` below.
    let args_file = temp_dir("guide-args").join("args");
    let build = format!("#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\n", args_file.display());
    sys.upload_build("0.1.0", &format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH), build.as_bytes()).await;

    // The Dashboard's Linux and macOS command (connect-guide/page.tsx), run without `sudo`.
    let ca: Vec<String> = sys.server.ca_pem.lines().map(|l| format!("'{l}'")).collect();
    let server_key = sys.server_key();
    let command = format!(
        "printf '%s\\n' {} > rcli-ca.pem && curl -fsSL --cacert rcli-ca.pem {api}/install.sh | sh -s -- --key {server_key}",
        ca.join(" ")
    );
    let run = tokio::process::Command::new("sh").args(["-c", &command]).current_dir(temp_dir("guide-cwd")).output().await.unwrap();
    assert!(run.status.success(), "{}", String::from_utf8_lossy(&run.stderr));

    let recorded = std::fs::read_to_string(&args_file).unwrap();
    let args: Vec<&str> = recorded.lines().collect();
    assert_eq!(args[..2], ["install", "--server"]);
    let out = sys.rcli_in(&temp_dir("guide-desk"), &[&args[..], &["--name", "guided"]].concat()).await.unwrap();
    assert!(out.contains("Device ID:") && out.contains("Device Password:"), "{out}");
}
