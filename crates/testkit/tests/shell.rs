use std::time::Duration;

use base64::Engine;
use rcli_testkit::{FakePlatform, StatusCode, TestSystem};
use serde_json::{json, Value};

/// An open interactive shell: the NDJSON output stream plus the session id for input/resize.
struct Shell {
    base: String,
    token: String,
    session: String,
    resp: reqwest::Response,
    buf: Vec<u8>,
    /// Decoded terminal output so far.
    screen: String,
}

impl Shell {
    async fn open(sys: &TestSystem, token: &str, device: &str, body: Value) -> Result<(Shell, Value), (StatusCode, Value)> {
        let base = format!("http://{}", sys.server.http_addr);
        let resp = reqwest::Client::new()
            .post(format!("{base}/api/v1/devices/{device}/shell"))
            .bearer_auth(token)
            .json(&body)
            .send()
            .await
            .unwrap();
        if resp.status() != StatusCode::OK {
            let status = resp.status();
            return Err((status, resp.json().await.unwrap_or(Value::Null)));
        }
        let mut shell = Shell { base, token: token.into(), session: String::new(), resp, buf: vec![], screen: String::new() };
        let first = shell.line().await.expect("first line");
        shell.session = first["session"].as_str().expect("session id").into();
        Ok((shell, first))
    }

    /// Next NDJSON line; output lines are appended to `screen`. None = stream ended.
    async fn line(&mut self) -> Option<Value> {
        loop {
            if let Some(i) = self.buf.iter().position(|b| *b == b'\n') {
                let line: Value = serde_json::from_slice(&self.buf[..i]).unwrap();
                self.buf.drain(..=i);
                if let Some(data) = line["output"].as_str() {
                    let bytes = base64::engine::general_purpose::STANDARD.decode(data).unwrap();
                    self.screen += &String::from_utf8_lossy(&bytes);
                }
                return Some(line);
            }
            let chunk = tokio::time::timeout(Duration::from_secs(10), self.resp.chunk()).await.expect("output in time");
            self.buf.extend_from_slice(&chunk.unwrap()?);
        }
    }

    /// Reads output until the screen contains `text`.
    async fn expect(&mut self, text: &str) {
        while !self.screen.contains(text) {
            let line = self.line().await;
            assert!(line.as_ref().is_some_and(|l| l.get("output").is_some()), "waiting for {text:?}, got {line:?}; screen {:?}", self.screen);
        }
    }

    /// Reads until the stream ends; returns the last line.
    async fn finish(&mut self) -> Value {
        let mut last = Value::Null;
        while let Some(l) = self.line().await {
            last = l;
        }
        last
    }

    async fn post(&self, what: &str, body: reqwest::Body, token: &str) -> StatusCode {
        let url = format!("{}/api/v1/shell-sessions/{}/{what}", self.base, self.session);
        reqwest::Client::new().post(url).bearer_auth(token).body(body).send().await.unwrap().status()
    }

    async fn input(&self, bytes: &str) {
        assert_eq!(self.post("input", bytes.to_owned().into(), &self.token).await, StatusCode::NO_CONTENT);
    }

    async fn resize(&self, cols: u32, rows: u32) {
        let body = json!({ "cols": cols, "rows": rows }).to_string();
        assert_eq!(self.post("resize", body.into(), &self.token).await, StatusCode::NO_CONTENT);
    }
}

async fn online(sys: &TestSystem, platform: FakePlatform) -> (String, rcli_testkit::TestAgent) {
    let agent = sys.start_agent_on(platform).await;
    let dev = agent.device_id().to_owned();
    sys.wait_device(&dev, |d| d["status"] == "Online").await;
    (dev, agent)
}

#[tokio::test]
async fn interactive_shell_runs_commands_as_the_logged_in_user() {
    let sys = TestSystem::start().await;
    let (dev, _agent) = online(&sys, FakePlatform::new("lab-pc")).await;
    let alice = sys.create_user("alice").await;
    // Someone else holds the Control Lock: the terminal is not exclusive.
    assert_eq!(sys.api.post(&format!("/api/v1/devices/{dev}/lock")).await.0, StatusCode::OK);

    let (mut sh, first) = Shell::open(&sys, &alice, &dev, json!({ "cols": 80, "rows": 24 })).await.unwrap();
    assert_eq!(first["as"], "user", "{first}");
    sh.expect("$ ").await;
    // The Terminal User defaults to the account running the Agent (this test process).
    let running = std::env::var("USER").expect("USER");
    sh.input("whoami\r").await;
    sh.expect(&format!("whoami\r\n{running}\r\n$ ")).await;
    sh.input("exit\r").await;
    assert_eq!(sh.finish().await, json!({ "exit_code": 0 }));
}

#[tokio::test]
async fn the_terminal_resizes_and_ctrl_c_reaches_the_shell() {
    let sys = TestSystem::start().await;
    let (dev, _agent) = online(&sys, FakePlatform::new("lab-pc")).await;

    let (mut sh, _) = Shell::open(&sys, &sys.admin_token, &dev, json!({ "cols": 100, "rows": 30 })).await.unwrap();
    sh.input("stty size\r").await;
    sh.expect("30 100\r\n").await;
    sh.resize(132, 43).await;
    sh.input("stty size\r").await;
    sh.expect("43 132\r\n").await;
    sh.input("half a line\x03").await;
    sh.expect("half a line^C\r\n$ ").await;
    sh.input("exit 3\r").await;
    assert_eq!(sh.finish().await, json!({ "exit_code": 3 }));
}

#[tokio::test]
async fn root_shells_are_not_gated_by_server_roles() {
    use rcli_testkit::eventually;
    let sys = TestSystem::start().await;
    let dir = rcli_testkit::temp_dir("shell-root");
    let agent = sys.start_agent_trusting_socket("lab-pc", &dir).await;
    let dev = agent.device_id().to_owned();
    sys.wait_device(&dev, |d| d["status"] == "Online").await;
    let socket = dir.join("agent.sock");
    eventually(|| async { socket.exists().then_some(()) }).await;
    // The Device allows root always; who asks does not matter.
    sys.agent_rcli(&socket, &["agent", "root", "always"]).await.unwrap();
    let alice = sys.create_user("alice").await;

    let (mut sh, first) = Shell::open(&sys, &alice, &dev, json!({ "as": "root" })).await.unwrap();
    assert_eq!(first["as"], "root");
    sh.input("whoami\r").await;
    sh.expect("root\r\n").await;
    sh.input("exit\r").await;
    assert_eq!(sh.finish().await, json!({ "exit_code": 0 }));
}

#[tokio::test]
async fn closing_the_stream_ends_the_session_and_only_its_owner_can_type() {
    let sys = TestSystem::start().await;
    let (dev, _agent) = online(&sys, FakePlatform::new("lab-pc")).await;
    let alice = sys.create_user("alice").await;

    let (mut sh, _) = Shell::open(&sys, &alice, &dev, json!({})).await.unwrap();
    sh.expect("$ ").await;
    assert_eq!(sh.post("input", "whoami\r".into(), &sys.admin_token).await, StatusCode::NOT_FOUND);
    let session = sh.session.clone();
    let (base, token) = (sh.base.clone(), sh.token.clone());
    drop(sh);

    let url = format!("{base}/api/v1/shell-sessions/{session}/input");
    rcli_testkit::eventually(|| async {
        let status = reqwest::Client::new().post(&url).bearer_auth(&token).body("x").send().await.unwrap().status();
        // An ended session takes no input.
        (status == StatusCode::NOT_FOUND).then_some(())
    })
    .await;
}

#[tokio::test]
async fn the_stream_reports_a_device_disconnect() {
    let sys = TestSystem::start().await;
    let (dev, mut agent) = online(&sys, FakePlatform::new("lab-pc")).await;
    let (mut sh, _) = Shell::open(&sys, &sys.admin_token, &dev, json!({})).await.unwrap();
    sh.expect("$ ").await;
    agent.stop();
    assert_eq!(sh.finish().await["error"]["code"], "device_disconnected");
}

#[tokio::test]
async fn rcli_shell_relays_keystrokes_and_exits_with_the_remote_code() {
    let sys = TestSystem::start().await;
    let (_dev, _agent) = online(&sys, FakePlatform::new("lab-pc")).await;
    let alice = sys.create_user("alice").await;
    let base = format!("http://{}", sys.server.http_addr);
    let rcli = |token: &str, args: &[&str], keys: &[&str]| {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        keys.iter().for_each(|k| tx.send(k.as_bytes().to_vec()).unwrap());
        let argv: Vec<String> = ["rcli", "--server", &base, "--token", token].iter().chain(args).map(|s| s.to_string()).collect();
        async move {
            let mut out = Vec::new();
            let result = rcli::run_with_input(argv, Some(rx), &mut out, std::sync::Arc::new(std::sync::Mutex::new(std::io::stderr()))).await;
            (result, String::from_utf8(out).unwrap())
        }
    };

    let running = std::env::var("USER").expect("USER");
    let (result, out) = rcli(&alice, &["shell", "lab-pc"], &["whoami\r", "exit 5\r"]).await;
    assert!(out.contains(&format!("whoami\r\n{running}\r\n")), "{out:?}");
    let exit = result.unwrap_err();
    assert_eq!(exit.downcast_ref::<rcli::RemoteExit>().map(|e| e.exit_code()), Some(5), "{exit:#}");

    // Server roles no longer gate Device work; the Device's root setting (default `ask`, with no session to show
    // the prompt in) does, whoever asks.
    let (result, _) = rcli(&sys.admin_token, &["shell", "lab-pc", "--as", "root"], &["exit\r"]).await;
    assert!(format!("{:#}", result.unwrap_err()).contains("root_denied"));
}

/// `rcli agent disconnect alice` ends alice's shell only; bob, also connected, keeps his.
#[tokio::test]
async fn disconnect_ends_only_that_controllers_shell() {
    let sys = TestSystem::start().await;
    let (agent, dir) = rcli_testkit::online(&sys, "disconnect-one", FakePlatform::new("lab-pc")).await;
    let dev = agent.device_id().to_owned();
    let (alice, bob) = (sys.create_user("alice").await, sys.create_user("bob").await);
    let (mut alices, _) = Shell::open(&sys, &alice, &dev, json!({})).await.unwrap();
    let (mut bobs, _) = Shell::open(&sys, &bob, &dev, json!({})).await.unwrap();
    alices.expect("$ ").await;
    bobs.expect("$ ").await;

    let out = sys.agent_rcli(&dir.join("agent.sock"), &["agent", "disconnect", "alice"]).await.unwrap();
    assert!(out.contains("alice (Dashboard)") && !out.contains("bob"), "{out}");

    assert!(alices.finish().await.get("exit_code").is_some(), "alice's shell ended");
    bobs.input("echo still-here\r").await;
    bobs.expect("still-here").await;
}

/// Ticket 18: switching the terminal off ends the shells that are already open.
#[tokio::test]
async fn switching_the_terminal_off_ends_an_open_shell() {
    let sys = TestSystem::start().await;
    let (agent, dir) = rcli_testkit::online(&sys, "caps-open-shell", FakePlatform::new("lab-pc")).await;
    let alice = sys.create_user("alice").await;
    let (mut shell, _) = Shell::open(&sys, &alice, agent.device_id(), json!({})).await.unwrap();
    shell.expect("$ ").await;

    sys.agent_rcli(&dir.join("agent.sock"), &["agent", "capability", "terminal", "off"]).await.unwrap();

    shell.input("echo still-here\r").await;
    let last = shell.finish().await;
    assert!(last.get("exit_code").is_some() && !shell.screen.contains("still-here\r\n"), "{last}; screen {:?}", shell.screen);
}
