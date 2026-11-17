//! The in-process system for the Dashboard's Playwright tests: Server on Postgres (testcontainers), a manual
//! clock, and FakePlatform Agents. Usage: `rcli-devstack <http-port> <info.json>`.
//! Writes `{api, admin_token, now}` to `info.json` once the fleet is ready; runs until SIGINT/SIGTERM.
//! Fleet: `alpha` Online (LockScreen, user "an", full inventory), `bravo` Offline,
//! `charlie` Online (logged in, a second 1024x768 display; its Control Lock belongs to the direct-control test),
//! `golf` Online (power test), `hotel` Offline in alpha's LAN (wake test),
//! `india` Online (its Control Lock belongs to the control-reminder test), `juliet` Online with no displays
//! (headless: the Dashboard hides its Screen tab).
//! `update` in info.json is an Agent Build to upload, for the update screen tests; `devices` maps each name to its
//! Device ID and Device Password, so a test User can add the fleet to their Address Book.
//! The clock starts at real time and is frozen after seeding.
//! With `<ca-http-port>`, a second Server on the same database serves there as one whose HTTPS certificate comes from
//! its CA (ADR 0015), for the Connect Guide's second shape.

use rcli_testkit::{platform::SessionState, Clock, DisplayInfo, FakePlatform, TestAgent, TestSystem};
use rcli_agent::platform::{Disk, Display, Inventory, NetworkInterface, Platform};
use serde_json::json;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let (port, out, ca_port) = match &args[..] {
        [_, port, out] => (port, out, None),
        [_, port, out, ca_port] => (port, out, Some(ca_port)),
        _ => anyhow::bail!("usage: rcli-devstack <http-port> <info.json> [<ca-http-port>]"),
    };
    let _ = std::fs::remove_file(out);
    let mut sys = TestSystem::start_on(format!("127.0.0.1:{port}").parse()?).await;
    let _ca_server = match ca_port {
        Some(ca_port) => {
            let files = rcli_testkit::temp_dir("devstack-https");
            let config = rcli_server::Config {
                http_addr: format!("127.0.0.1:{ca_port}").parse()?,
                agent_addr: "127.0.0.1:0".parse()?,
                https_files: Some((files.join("cert.pem"), files.join("key.pem"))),
                ..sys.config.clone()
            };
            Some(rcli_server::start(config, sys.clock.clone()).await?)
        }
        None => None,
    };
    sys.clock.advance(chrono::Utc::now() - sys.clock.now());
    // The jump past the harness's real-time start expired its admin session.
    sys.sign_in_admin().await;

    let alpha = FakePlatform::new("alpha");
    alpha.set_session_state(SessionState::Locked);
    alpha.update(|s| {
        s.inventory = Inventory {
            os: "FakeOS".into(),
            os_version: "1.0".into(),
            hostname: "alpha".into(),
            cpu_model: "Fake CPU 9000".into(),
            cpu_cores: 8,
            ram_bytes: 16 << 30,
            disks: vec![Disk { name: "disk0".into(), mount: "/".into(), total_bytes: 512 << 30 }],
            gpus: vec!["Fake GPU".into()],
            network_interfaces: vec![NetworkInterface {
                name: "eth0".into(),
                mac: "02:00:00:00:00:01".into(),
                ips: vec!["10.0.0.11".into()],
            }],
            serial: "FAKE-0001".into(),
            displays: vec![Display { name: "Display 1".into(), width: 1920, height: 1080 }],
        };
        s.resources.logged_in_user = Some("an".into());
        s.resources.uptime_secs = Some(3 * 3600);
        s.resources.disk_used_percent.insert("/".into(), 40.0);
    });
    let hotel = FakePlatform::new("hotel");
    hotel.update(|s| {
        s.inventory.network_interfaces =
            vec![NetworkInterface { name: "eth0".into(), mac: "02:00:00:00:00:05".into(), ips: vec!["10.0.0.15/24".into()] }]
    });
    let mut agents = vec![];
    let charlie = FakePlatform::new("charlie");
    charlie.set_displays(vec![
        DisplayInfo { index: 0, x: 0, y: 0, width: 1920, height: 1080, scale: 1.0, primary: true },
        DisplayInfo { index: 1, x: 1920, y: 0, width: 1024, height: 768, scale: 1.0, primary: false },
    ]);
    // juliet is a headless server: no displays, so the Dashboard hides its Screen tab.
    let juliet = FakePlatform::new("juliet");
    juliet.set_displays(vec![]);
    let fleet = [
        alpha,
        FakePlatform::new("bravo"),
        charlie,
        FakePlatform::new("golf"),
        hotel,
        FakePlatform::new("india"),
        juliet,
    ];
    for platform in fleet {
        // The Dashboard reads displays from the inventory; keep it in step with what the platform reports.
        let displays: Vec<Display> =
            platform.displays()?.iter().map(|d| Display { name: format!("Display {}", d.index + 1), width: d.width, height: d.height }).collect();
        platform.update(|s| s.inventory.displays = displays);
        let creds = sys.register(&platform).await?;
        let mut agent = TestAgent::new(sys.server_access(), platform, creds);
        agent.start();
        sys.wait_device_details(agent.device_id(), |d| !d["inventory"].is_null() && !d["resources"].is_null()).await;
        agents.push(agent);
    }
    // bravo and hotel drop off and stay away past the Offline threshold.
    agents[1].stop();
    agents[4].stop();
    let (bravo, hotel) = (agents[1].device_id().to_owned(), agents[4].device_id().to_owned());
    for id in [&bravo, &hotel] {
        sys.wait_device(id, |d| !d["last_seen_at"].is_null()).await;
    }
    sys.advance(31);
    sys.wait_device(&bravo, |d| d["status"] == "Offline").await;
    sys.wait_device(&hotel, |d| d["status"] == "Offline").await;
    // The MAC and subnet an Address Book keeps from hotel's last connection.
    let hotel_lan = json!({ "mac": "02:00:00:00:00:05", "subnet": "10.0.0.0/24" });

    // Each Device's ID and password, so a test User can add the fleet to their Address Book.
    let names = ["alpha", "bravo", "charlie", "golf", "hotel", "india", "juliet"];
    let mut devices: serde_json::Map<String, serde_json::Value> = names
        .iter()
        .zip(&agents)
        .map(|(name, a)| (name.to_string(), json!({ "device_id": a.device_id(), "password": a.device_password() })))
        .collect();
    // hotel is Offline when test Users add it, so their entries carry the MAC and subnet it had when last Online.
    devices["hotel"].as_object_mut().unwrap().extend(hotel_lan.as_object().unwrap().clone());
    // A real target from the Dashboard picker that no test Agent runs on.
    let (version, target, build) = ("9.9.9", "windows-aarch64", "fake agent build");

    let info = json!({
        "api": format!("http://{}", sys.server.http_addr),
        "admin_token": sys.admin_token,
        "now": sys.clock.now().timestamp(),
        "update": {"version": version, "target": target, "build": build},
        "devices": devices,
    });
    std::fs::write(out, info.to_string())?;
    println!("rcli-devstack ready: {info}");

    // SIGTERM handling is Unix-only; Windows dev runs stop it with Ctrl-C.
    #[cfg(unix)]
    {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = term.recv() => {} }
    }
    #[cfg(not(unix))]
    tokio::signal::ctrl_c().await?;
    let _ = std::fs::remove_file(out);
    Ok(()) // dropping `sys` removes the Postgres container
}
