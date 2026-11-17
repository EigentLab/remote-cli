//! Local IPC: the running Agent serves `rcli agent ...` on a Unix socket (a named pipe on Windows).
//! One JSON request per connection, one JSON reply, newline-terminated; only `tokio::io` traits are used, so a
//! pipe transport can reuse `handle`.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex, RwLock,
    },
    time::{Duration, Instant},
};

use rcli_protocol::{agent::IdentityUpdate, password::MIN_LENGTH, server_key::ServerKey};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader},
    sync::{mpsc, oneshot, watch},
};

use crate::{secret_store::SecretStore, settings, ServerAccess};

/// The Device Password's name in the Secret Store.
pub const DEVICE_PASSWORD: &str = "device_password";

/// How long a change may wait for the Server's answer.
const CHANGE_TIMEOUT: Duration = Duration::from_secs(10);

/// A Device identity change that needs the Server's acknowledgement, sent on the live connection.
pub struct Change {
    pub update: IdentityUpdate,
    /// The Device ID in effect after the change, or the message to show.
    pub reply: oneshot::Sender<Result<String, String>>,
}

/// One request to the running Agent; on the wire `{"command": "<name>", ...fields}\n`, one per connection.
/// Missing text fields read as empty, so a non-root caller is told it needs root before anything else.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "command")]
pub enum Request {
    /// The Device ID, connection status and connected Controllers (any user); settings and secrets for root.
    #[serde(rename = "info")]
    Info,
    /// The prompts waiting for the person at the Device (any user).
    // shortcut: the spec's IPC split lets any local user see only the ID, connection status, who is controlling and
    // disconnect. Story 27 needs the person at the Device — whoever that is, not only root — to see a prompt naming
    // who asks for root, and story 28 lets them grant or refuse it, so this listing is deliberately open. It exposes
    // requester names and pending requests to any local user; seeing a request does not grant it (accepting one
    // stays root-only, refusing is like `disconnect`), and gating the list on root would leave the prompt
    // unanswerable when nobody is an Administrator at that moment. Revisit if the split is enforced literally.
    #[serde(rename = "prompts")]
    Prompts,
    /// Answers a waiting prompt: anyone may refuse, only root may accept.
    #[serde(rename = "prompt.answer")]
    PromptAnswer {
        #[serde(default)]
        id: u64,
        #[serde(default)]
        accept: bool,
    },
    /// Ends the Controllers called `who`, or every Controller when it is empty (any user).
    #[serde(rename = "disconnect")]
    Disconnect {
        #[serde(default)]
        who: String,
    },
    #[serde(rename = "password.show")]
    PasswordShow,
    #[serde(rename = "password.set")]
    PasswordSet {
        #[serde(default)]
        password: String,
    },
    #[serde(rename = "id.set")]
    IdSet {
        #[serde(default)]
        device_id: String,
    },
    #[serde(rename = "terminal-user.set")]
    TerminalUserSet {
        #[serde(default)]
        user: String,
    },
    #[serde(rename = "root.set")]
    RootSet { root: settings::RootAccess },
    #[serde(rename = "accept.set")]
    AcceptSet { accept: settings::AcceptMode },
    /// Switches Controller capabilities: in the Device Settings when `who` is empty (root), else for the connections
    /// called `who` only, within the Device Settings (anyone may switch one off, only root back on). Unset = unchanged.
    #[serde(rename = "capabilities.set")]
    CapabilitiesSet {
        #[serde(default)]
        who: String,
        #[serde(default)]
        mouse_keyboard: Option<bool>,
        #[serde(default)]
        terminal: Option<bool>,
        #[serde(default)]
        power: Option<bool>,
    },
    /// Settings > General: auto-update and the Session Window (an owner change). Unset = unchanged.
    #[serde(rename = "general.set")]
    GeneralSet {
        #[serde(default)]
        auto_update: Option<bool>,
        #[serde(default)]
        session_window: Option<bool>,
    },
    /// Root for the Controller called `who`, for this session only.
    #[serde(rename = "root.grant")]
    RootGrant {
        #[serde(default)]
        who: String,
    },
    /// Ends whoever's Control Lock it is (the Device Owner, ticket 11).
    #[serde(rename = "lock.release")]
    LockRelease,
    /// Another Server Address (`host:port`) and Server Key; the Agent reconnects at once.
    #[serde(rename = "server.set")]
    ServerSet {
        #[serde(default)]
        server: String,
        #[serde(default)]
        key: String,
    },
    /// Local control (decision 15): an Action request body as the Controller API takes it, run on this Device by
    /// the Device Owner; the reply is the Observation, or `{"error": {"code", "message", "hint", ..}}`.
    #[serde(rename = "act")]
    Act {
        #[serde(default)]
        body: Value,
    },
}

impl Request {
    /// An owner change (ADR 0013): the Device Owner needs a fresh OS authentication for it; root does not (sudo or
    /// UAC asked an admin already). Anything that opens the Device to others is one (story 17); refusing,
    /// disconnecting and narrowing are not.
    fn is_owner_change(&self) -> bool {
        use Request::*;
        matches!(
            self,
            PasswordShow
                | PasswordSet { .. }
                | IdSet { .. }
                | TerminalUserSet { .. }
                | RootSet { .. }
                | AcceptSet { .. }
                | GeneralSet { .. }
                | RootGrant { .. }
                | ServerSet { .. }
                | PromptAnswer { accept: true, .. }
        ) || matches!(self, CapabilitiesSet { who, mouse_keyboard, terminal, power }
                if who.trim().is_empty() || [mouse_keyboard, terminal, power].contains(&&Some(true)))
    }
}

/// How the Device Owner proves it is them for an owner change, sent beside the request as `os_auth`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OsAuth {
    /// The Agent shows the OS authentication dialog (the GUI): rcli never sees the password.
    Dialog,
    /// The OS account password the CLI asked for on its tty.
    Password(String),
}

/// The reply to an owner change sent without `os_auth`: the CLI then asks on its tty.
pub const OS_PASSWORD_REQUIRED: &str =
    "this asks for your OS account password: run it on a terminal or in the rcli window (a script cannot do this)";

/// The `code` of a reply refused only because the caller is not the Device Owner or root: an elevated retry can
/// succeed.
pub const ROOT_REQUIRED: &str = "root_required";

/// An error reply of the Agent: `send` returns it inside its `anyhow::Error`, with the reply's `code` ("" when none).
#[derive(Debug)]
pub struct Refused {
    pub code: String,
    pub message: String,
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Refused {}

/// Who calls over the IPC, from the peer credential.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Caller {
    Other,
    /// The OS user the Agent runs as (User Mode), not root.
    Owner,
    /// Service Mode: the user logged in at the Device's screen. They control the Local Device, nothing else.
    Console,
    Root,
}

/// What a prompt asks the person at the Device.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PromptKind {
    /// Root for a terminal command or shell (ticket 07).
    Root,
    /// Let a new Controller in, in `password+accept` mode (ticket 08).
    Accept,
}

/// Prompts for the person at the Device (tickets 07/08), waiting for the GUI's answer over the IPC.
#[derive(Default)]
pub struct Prompts {
    state: Mutex<PromptState>,
}

#[derive(Default)]
struct PromptState {
    next_id: u64,
    pending: Vec<PendingPrompt>,
    answers: HashMap<u64, std::sync::mpsc::Sender<bool>>,
}

struct PendingPrompt {
    id: u64,
    kind: PromptKind,
    who: String,
    deadline: Instant,
}

impl Prompts {
    /// Publishes a prompt and waits for the person's answer, for at most `timeout`; no answer rejects (30 s rule).
    pub fn ask(&self, kind: PromptKind, who: &str, timeout: Duration) -> bool {
        let (tx, rx) = std::sync::mpsc::channel();
        let id = {
            let mut state = self.state.lock().unwrap();
            state.next_id += 1;
            let id = state.next_id;
            state.pending.push(PendingPrompt { id, kind, who: who.to_owned(), deadline: Instant::now() + timeout });
            state.answers.insert(id, tx);
            id
        };
        let accepted = rx.recv_timeout(timeout).unwrap_or(false);
        self.forget(id);
        accepted
    }

    fn forget(&self, id: u64) {
        let mut state = self.state.lock().unwrap();
        state.pending.retain(|p| p.id != id);
        state.answers.remove(&id);
    }

    /// The prompts waiting for an answer right now (id, kind, who, seconds left).
    pub fn list(&self) -> Value {
        let mut state = self.state.lock().unwrap();
        let now = Instant::now();
        state.pending.retain(|p| p.deadline > now);
        serde_json::Value::Array(
            state
                .pending
                .iter()
                .map(|p| {
                    json!({
                        "id": p.id,
                        "kind": p.kind,
                        "who": p.who,
                        "seconds_left": p.deadline.duration_since(now).as_secs(),
                    })
                })
                .collect(),
        )
    }

    /// Answers a waiting prompt; `Err` names an unknown or already-answered one.
    pub fn answer(&self, id: u64, accept: bool) -> Result<(), String> {
        let sender = self.state.lock().unwrap().answers.remove(&id);
        match sender {
            Some(sender) => {
                self.forget(id);
                let _ = sender.send(accept);
                Ok(())
            }
            None => Err(format!("no prompt {id} is waiting for an answer")),
        }
    }
}

/// Sets `api_address` in the machine-wide Controller configuration at `path`, if `rcli install` wrote one and it
/// differs. Rewritten in place, so the file keeps the read-only-for-users permissions the install gave it.
fn write_api_address(path: &Path, url: &str) -> anyhow::Result<()> {
    let mut config: Value = match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.into()),
    };
    if config["api_address"] == url {
        return Ok(());
    }
    config["api_address"] = url.into();
    std::fs::write(path, serde_json::to_vec_pretty(&config)?)?;
    Ok(())
}

/// The Server Address and Server Key saved in the state dir: `rcli install` writes it in User Mode (no key on a
/// command line), `server.set` rewrites it.
pub const SERVER_FILE: &str = "server.json";

/// The Server saved in `state_dir`, if any.
pub fn saved_server(state_dir: &Path) -> Option<ServerAccess> {
    let saved: Value = serde_json::from_slice(&std::fs::read(state_dir.join(SERVER_FILE)).ok()?).ok()?;
    Some(ServerAccess { addr: saved["server"].as_str()?.into(), server_key: saved["key"].as_str()?.into() })
}

/// Writes `bytes` readable by this OS user only: the file holds the Server Key.
fn write_private(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, bytes)?;
    #[cfg(unix)]
    std::fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(0o600))?;
    Ok(())
}

/// What the local callers see of the running Agent: `rcli agent …`, the GUI and scripts.
pub struct Status {
    device_id: RwLock<String>,
    api_address: RwLock<String>,
    /// The Server the Agent connects to; a change reconnects it without a restart.
    server_access: watch::Sender<ServerAccess>,
    connected: AtomicBool,
    secrets: Arc<dyn SecretStore>,
    /// Where `rcli agent run` keeps its credentials, so a new Device ID survives a restart (None in tests).
    credentials: Mutex<Option<PathBuf>>,
    /// The machine-wide Controller configuration `rcli install` wrote; the Agent keeps its API Address current.
    controller_config: Mutex<Option<PathBuf>>,
    /// Where `server.set` saves the Server Address and Server Key (None in tests).
    server_file: Mutex<Option<PathBuf>>,
    changes: mpsc::UnboundedSender<Change>,
    changes_rx: Mutex<Option<mpsc::UnboundedReceiver<Change>>>,
    /// Controllers the person at the Device disconnected, forwarded to the Server over the live connection.
    disconnects: mpsc::UnboundedSender<String>,
    disconnects_rx: Mutex<Option<mpsc::UnboundedReceiver<String>>>,
    /// Control Lock force-releases by the Device Owner, forwarded to the Server over the live connection.
    lock_releases: mpsc::UnboundedSender<()>,
    lock_releases_rx: Mutex<Option<mpsc::UnboundedReceiver<()>>>,
    next_request: AtomicU64,
    /// The Device Settings the Agent enforces; kept here so `rcli agent …` can change them live.
    settings: Arc<settings::Handle>,
    /// Root granted to the current session, by the person at the Device. Cleared when the Agent reconnects or that
    /// Controller is disconnected.
    grant: Arc<crate::Grant>,
    /// Controllers currently using this Device, so `rcli agent info` / `disconnect` can see and end them.
    controllers: Arc<crate::Controllers>,
    /// Prompts for the person at the Device (tickets 07/08), shown and answered by the GUI over the IPC.
    prompts: Arc<Prompts>,
    /// The running Agent's Platform, so `info` can show the capture permissions of the process that captures.
    platform: Mutex<Option<Arc<dyn crate::platform::Platform>>>,
    /// Runs Actions one at a time, for Controllers over the Server and local control alike.
    executor: Arc<Mutex<crate::executor::Executor>>,
}

impl Status {
    pub fn new(device_id: &str, server_access: ServerAccess, secrets: Arc<dyn SecretStore>) -> Self {
        let (changes, changes_rx) = mpsc::unbounded_channel();
        let (disconnects, disconnects_rx) = mpsc::unbounded_channel();
        let (lock_releases, lock_releases_rx) = mpsc::unbounded_channel();
        Self {
            device_id: RwLock::new(device_id.into()),
            api_address: RwLock::default(),
            server_access: watch::channel(server_access).0,
            connected: AtomicBool::new(false),
            secrets,
            credentials: Mutex::new(None),
            controller_config: Mutex::new(None),
            server_file: Mutex::new(None),
            changes,
            changes_rx: Mutex::new(Some(changes_rx)),
            disconnects,
            disconnects_rx: Mutex::new(Some(disconnects_rx)),
            lock_releases,
            lock_releases_rx: Mutex::new(Some(lock_releases_rx)),
            next_request: AtomicU64::new(0),
            settings: Arc::new(settings::Handle::new()),
            grant: Arc::new(crate::Grant::default()),
            controllers: Arc::new(crate::Controllers::default()),
            prompts: Arc::new(Prompts::default()),
            platform: Mutex::new(None),
            executor: Default::default(),
        }
    }

    pub(crate) fn executor(&self) -> Arc<Mutex<crate::executor::Executor>> {
        self.executor.clone()
    }

    /// The Platform the Agent runs on; `run` sets it.
    pub fn set_platform(&self, platform: Arc<dyn crate::platform::Platform>) {
        *self.platform.lock().unwrap() = Some(platform);
    }

    /// Where Device Settings are saved (the Agent's state directory); they apply to the running Agent at once.
    pub fn set_settings_dir(&self, dir: std::path::PathBuf) {
        self.settings.set_dir(dir);
    }

    pub fn settings(&self) -> Arc<settings::Handle> {
        self.settings.clone()
    }

    /// The Secret Store.
    pub fn secrets(&self) -> Arc<dyn SecretStore> {
        self.secrets.clone()
    }

    /// The session root grant: shared with `run`, cleared whenever the connection is re-established.
    pub fn grant(&self) -> Arc<crate::Grant> {
        self.grant.clone()
    }

    /// The connected Controllers, shared with `run` (so `info` / `disconnect` see the live set).
    pub fn controllers(&self) -> Arc<crate::Controllers> {
        self.controllers.clone()
    }

    /// The prompts waiting for the person at the Device; the GUI lists and answers them here.
    pub fn prompts(&self) -> Arc<Prompts> {
        self.prompts.clone()
    }

    pub fn set_connected(&self, connected: bool) {
        self.connected.store(connected, Ordering::Relaxed);
    }

    pub fn device_id(&self) -> String {
        self.device_id.read().unwrap().clone()
    }

    pub fn server_access(&self) -> ServerAccess {
        self.server_access.borrow().clone()
    }

    /// The API Address the Server sent on the current connection; empty before that, or while it has none.
    pub fn api_address(&self) -> String {
        self.api_address.read().unwrap().clone()
    }

    pub fn set_api_address(&self, url: &str) {
        *self.api_address.write().unwrap() = url.to_owned();
        if let Some(path) = self.controller_config.lock().unwrap().as_deref() {
            if let Err(e) = write_api_address(path, url) {
                tracing::warn!("cannot record the API Address in {}: {e:#}", path.display());
            }
        }
    }

    /// The machine-wide Controller configuration whose `api_address` this Agent keeps current. A machine where
    /// `rcli install` wrote none (a development `rcli agent run`) keeps none.
    pub fn set_controller_config(&self, path: PathBuf) {
        *self.controller_config.lock().unwrap() = Some(path);
    }

    pub fn subscribe(&self) -> watch::Receiver<ServerAccess> {
        self.server_access.subscribe()
    }

    /// Points the Agent at another Server: the running session reconnects with the new values (and an unregistered
    /// Agent registers). Saved for the next start when `rcli agent run` gave a server file.
    pub fn set_server_access(&self, server_access: ServerAccess) {
        if let Some(path) = self.server_file.lock().unwrap().as_deref() {
            let json = json!({ "server": server_access.addr, "key": server_access.server_key }).to_string();
            if let Err(e) = write_private(path, json.as_bytes()) {
                tracing::warn!("cannot save the Server in {}: {e:#}", path.display());
            }
        }
        self.server_access.send_replace(server_access);
    }

    /// Where a Server set at the Device is saved (`SERVER_FILE` in the state dir).
    pub fn set_server_file(&self, path: PathBuf) {
        *self.server_file.lock().unwrap() = Some(path);
    }

    /// Tells the Agent to rewrite `credentials.json` (so a new Device ID survives a restart).
    pub fn set_credentials_path(&self, path: PathBuf) {
        *self.credentials.lock().unwrap() = Some(path);
    }

    /// The change receiver, taken once by `run`.
    pub fn take_changes(&self) -> Option<mpsc::UnboundedReceiver<Change>> {
        self.changes_rx.lock().unwrap().take()
    }

    /// The disconnect receiver, taken once by `run`.
    pub fn take_disconnects(&self) -> Option<mpsc::UnboundedReceiver<String>> {
        self.disconnects_rx.lock().unwrap().take()
    }

    /// The lock-release receiver, taken once by `run`.
    pub fn take_lock_releases(&self) -> Option<mpsc::UnboundedReceiver<()>> {
        self.lock_releases_rx.lock().unwrap().take()
    }

    /// Asks the Server (over the live connection) to end `controller`'s Control Lock and sessions.
    pub fn request_disconnect(&self, controller: &str) {
        let _ = self.disconnects.send(controller.to_owned());
    }

    /// Stores a Device Password the Server set from the Dashboard.
    pub fn store_password(&self, password: &str) -> anyhow::Result<()> {
        self.secrets.set(DEVICE_PASSWORD, password)
    }

    pub fn set_device_id(&self, device_id: &str) {
        *self.device_id.write().unwrap() = device_id.to_owned();
        let path = self.credentials.lock().unwrap().clone();
        let Some(path) = path else { return };
        let Ok(bytes) = std::fs::read(&path) else { return };
        let Ok(mut creds) = serde_json::from_slice::<crate::Credentials>(&bytes) else { return };
        creds.device_id = device_id.to_owned();
        if let Ok(bytes) = serde_json::to_vec(&creds) {
            let _ = std::fs::write(&path, bytes);
        }
    }

    /// Sends a change to the Server and waits for its answer; `Err` is the message to show.
    async fn identity(&self, password: Option<&str>, device_id: Option<&str>) -> Result<String, String> {
        if !self.connected.load(Ordering::Relaxed) {
            return Err("the Agent is not connected to the Server; try again once it is Online".into());
        }
        let request_id = self.next_request.fetch_add(1, Ordering::Relaxed) + 1;
        let update = IdentityUpdate {
            request_id,
            device_password: password.unwrap_or_default().into(),
            device_id: device_id.unwrap_or_default().into(),
        };
        let (reply, answer) = oneshot::channel();
        if self.changes.send(Change { update, reply }).is_err() {
            return Err("the Agent is not running".into());
        }
        match tokio::time::timeout(CHANGE_TIMEOUT, answer).await {
            Ok(Ok(result)) => result,
            // shortcut: a connection lost after the Server applied the change also lands here; re-read `rcli agent info`
            // to see the Device ID. Add an idempotent re-check if that race ever shows up.
            Ok(Err(_)) | Err(_) => Err("the Agent did not get an answer from the Server; check `rcli agent info`".into()),
        }
    }

    fn info(&self, is_root: bool) -> Value {
        let mut info = json!({
            "device_id": self.device_id(),
            "server": self.server_access.borrow().addr,
            "api_address": self.api_address(),
            "connected": self.connected.load(Ordering::Relaxed),
            "controllers": self.controllers.list(),
            // What each Session Window shows (ticket 21); any local user may see who is controlling.
            "connections": self.controllers.connections(self.settings.get().capabilities, &self.grant),
        });
        // Everything outside the ID, status and controllers is root-only: the Device Settings and the Server Key (the
        // GUI reveals them elevated).
        if is_root {
            info["settings"] = json!(self.settings.get());
            info["server_key"] = self.server_access.borrow().server_key.clone().into();
        }
        info
    }
}

/// Where the Agent listens when no other path is given.
pub fn default_socket() -> PathBuf {
    // A User Mode Agent listens in its user's state dir (ADR 0012); the root service in /var/run.
    #[cfg(target_os = "macos")]
    return user_socket().filter(|p| p.exists()).unwrap_or_else(|| PathBuf::from("/var/run/rcli/agent.sock"));
    // A User Mode Agent listens on its user's own pipe (ADR 0012); the SYSTEM service on the shared one.
    #[cfg(windows)]
    return own_pipe().filter(|p| pipe_exists(p)).unwrap_or_else(|| PathBuf::from(r"\\.\pipe\rcli-agent"));
    #[cfg(not(any(target_os = "macos", windows)))]
    // A User Mode Agent listens in its user's runtime dir (ADR 0012); the root service in /run.
    user_socket().filter(|p| p.exists()).unwrap_or_else(|| PathBuf::from("/run/rcli/agent.sock"))
}

/// Linux User Mode: the socket in `$XDG_RUNTIME_DIR` (`/run/user/<uid>`, this user's only).
#[cfg(not(any(target_os = "macos", windows)))]
pub fn user_socket() -> Option<PathBuf> {
    std::env::var_os("XDG_RUNTIME_DIR").filter(|d| !d.is_empty()).map(|d| PathBuf::from(d).join("rcli/agent.sock"))
}

/// macOS User Mode: the socket in the user's state dir (0700, this user's only), where `rcli install` points it.
#[cfg(target_os = "macos")]
pub fn user_socket() -> Option<PathBuf> {
    std::env::var_os("HOME").filter(|h| !h.is_empty()).map(|h| PathBuf::from(h).join(USER_SOCKET))
}

/// The macOS User Mode socket, relative to the home.
pub const USER_SOCKET: &str = "Library/Application Support/rcli/agent.sock";

/// Windows User Mode: the pipe the Agent of Windows account `user` listens on (pipe names are machine-wide).
pub fn user_pipe(user: &str) -> PathBuf {
    PathBuf::from(format!(r"\\.\pipe\rcli-agent-{user}"))
}

/// This account's User Mode pipe.
#[cfg(windows)]
fn own_pipe() -> Option<PathBuf> {
    std::env::var("USERNAME").ok().filter(|u| !u.is_empty()).map(|u| user_pipe(&u))
}

/// Whether a server listens on `pipe`, without connecting to it (a connection would use up one of its instances).
#[cfg(windows)]
fn pipe_exists(pipe: &Path) -> bool {
    use windows::Win32::{Foundation::{GetLastError, ERROR_FILE_NOT_FOUND}, System::Pipes::WaitNamedPipeW};
    let name: Vec<u16> = pipe.as_os_str().to_string_lossy().encode_utf16().chain(std::iter::once(0)).collect();
    // Busy (every instance taken) still means it exists.
    unsafe { WaitNamedPipeW(windows::core::PCWSTR(name.as_ptr()), 1).as_bool() || GetLastError() != ERROR_FILE_NOT_FOUND }
}

/// The string SID of the account process `pid` runs as (`None`: this process), if its token can be read.
#[cfg(windows)]
pub fn process_user_sid(pid: Option<u32>) -> Option<String> {
    use windows::Win32::{
        Foundation::{CloseHandle, HANDLE},
        Security::TOKEN_QUERY,
        System::Threading::{GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION},
    };
    unsafe {
        let process = match pid {
            Some(pid) => OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?,
            None => GetCurrentProcess(),
        };
        let mut token = HANDLE::default();
        let opened = OpenProcessToken(process, TOKEN_QUERY, &mut token).is_ok();
        if pid.is_some() {
            let _ = CloseHandle(process);
        }
        if !opened {
            return None;
        }
        let sid = token_user_sid(token);
        let _ = CloseHandle(token);
        sid
    }
}

/// The string SID of the account `token` (opened with `TOKEN_QUERY`) belongs to.
#[cfg(windows)]
pub fn token_user_sid(token: windows::Win32::Foundation::HANDLE) -> Option<String> {
    use windows::Win32::{
        Foundation::{LocalFree, HLOCAL},
        Security::{Authorization::ConvertSidToStringSidW, GetTokenInformation, TokenUser, TOKEN_USER},
    };
    unsafe {
        // u64s: TOKEN_USER holds a pointer, so the buffer must be pointer-aligned.
        let mut buf = [0u64; 64];
        let mut len = 0;
        GetTokenInformation(token, TokenUser, Some(buf.as_mut_ptr() as *mut _), size_of_val(&buf) as u32, &mut len).ok()?;
        let user = &*(buf.as_ptr() as *const TOKEN_USER);
        let mut text = windows::core::PWSTR::null();
        ConvertSidToStringSidW(user.User.Sid, &mut text).ok()?;
        let sid = text.to_string().ok();
        let _ = LocalFree(Some(HLOCAL(text.0 as *mut core::ffi::c_void)));
        sid
    }
}

/// Only the Device Owner may read the Device Password or change the Device: the OS user this Agent runs as (User
/// Mode), or root. `info` is for any local user.
#[cfg(unix)]
pub async fn serve(path: PathBuf, status: Arc<Status>) -> anyhow::Result<()> {
    let me = unsafe { libc::geteuid() };
    // Read at each request: the user at the screen changes while the root Agent runs.
    serve_for(path, status, me, move || console_uid().filter(|_| me == 0)).await
}

/// `serve` with the Device Owner's uid given: tests stand in another OS user than the one they run as.
#[cfg(unix)]
pub async fn serve_owned_by(path: PathBuf, status: Arc<Status>, owner: u32) -> anyhow::Result<()> {
    serve_for(path, status, owner, || None).await
}

/// `serve` as a Service Mode Agent with `console` logged in at the Device's screen: tests cannot run as root.
#[cfg(unix)]
pub async fn serve_at_the_console_of(path: PathBuf, status: Arc<Status>, console: u32) -> anyhow::Result<()> {
    serve_for(path, status, 0, move || Some(console)).await
}

#[cfg(unix)]
async fn serve_for(path: PathBuf, status: Arc<Status>, owner: u32, console: impl Fn() -> Option<u32> + Send + Sync + 'static) -> anyhow::Result<()> {
    serve_with(path, status, move |stream| match stream.peer_cred().map(|c| c.uid()) {
        Ok(0) => Caller::Root,
        Ok(uid) if uid == owner => Caller::Owner,
        Ok(uid) if console() == Some(uid) => Caller::Console,
        _ => Caller::Other,
    })
    .await
}

/// The user logged in at the Device's screen; None at the login screen.
#[cfg(target_os = "macos")]
fn console_uid() -> Option<u32> {
    crate::macos_session::console_uid()
}

#[cfg(target_os = "linux")]
fn console_uid() -> Option<u32> {
    crate::linux::session::console_user_uid()
}

#[cfg(all(unix, not(any(target_os = "macos", target_os = "linux"))))]
fn console_uid() -> Option<u32> {
    None
}

/// Tests run unprivileged: this stands in for "the caller is root", nothing else.
#[cfg(unix)]
pub async fn serve_trusting_all(path: PathBuf, status: Arc<Status>) -> anyhow::Result<()> {
    serve_with(path, status, |_| Caller::Root).await
}

#[cfg(unix)]
async fn serve_with(
    path: PathBuf,
    status: Arc<Status>,
    caller: impl Fn(&tokio::net::UnixStream) -> Caller + Send + Sync + 'static,
) -> anyhow::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let _ = std::fs::remove_file(&path);
    let listener = tokio::net::UnixListener::bind(&path)?;
    // Any local user may connect (info, disconnect); the peer credential, not the file mode, gates the rest.
    std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o666))?;
    loop {
        let Ok((stream, _)) = listener.accept().await else { continue };
        let status = status.clone();
        let caller = caller(&stream);
        tokio::spawn(async move {
            if let Err(e) = handle(stream, status, caller).await {
                tracing::debug!("IPC client ended: {e}");
            }
        });
    }
}

/// Windows: a named pipe any local user may open (info, disconnect); the Device Owner is an elevated caller or the
/// account this Agent runs as (User Mode, ADR 0012).
// shortcut: a busy pipe is not retried by `send`; one request per connection keeps that rare.
#[cfg(windows)]
pub async fn serve(path: PathBuf, status: Arc<Status>) -> anyhow::Result<()> {
    serve_windows(path, status, false).await
}

/// Tests run unprivileged: this stands in for "the caller is root", nothing else.
#[cfg(windows)]
pub async fn serve_trusting_all(path: PathBuf, status: Arc<Status>) -> anyhow::Result<()> {
    serve_windows(path, status, true).await
}

#[cfg(windows)]
async fn serve_windows(path: PathBuf, status: Arc<Status>, trust_all: bool) -> anyhow::Result<()> {
    let mut server = pipe_server(&path, true)?;
    let me = process_user_sid(None);
    // Service Mode: the Agent is the SYSTEM service.
    let service = me.as_deref() == Some("S-1-5-18");
    loop {
        server.connect().await?;
        let client = std::mem::replace(&mut server, pipe_server(&path, false)?);
        let same_user = || me.is_some() && client_pid(&client).and_then(|pid| process_user_sid(Some(pid))) == me;
        let caller = match () {
            _ if trust_all || client_is_elevated(&client) => Caller::Root,
            _ if same_user() => Caller::Owner,
            _ if service && client_pid(&client).is_some_and(crate::windows_session::in_console_session) => Caller::Console,
            _ => Caller::Other,
        };
        let status = status.clone();
        tokio::spawn(async move {
            if let Err(e) = handle(client, status, caller).await {
                tracing::debug!("IPC client ended: {e}");
            }
        });
    }
}

/// One pipe instance. The first refuses to start if another process already owns the name (pipe squatting).
#[cfg(windows)]
fn pipe_server(path: &Path, first: bool) -> anyhow::Result<tokio::net::windows::named_pipe::NamedPipeServer> {
    use windows::Win32::Security::{
        Authorization::{ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1},
        PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES,
    };
    // SYSTEM and Administrators: full access; Everyone: read and write, so any local user can send a request. The
    // helper pipe has the same callers, so both use one DACL (crate::windows_session::PIPE_DACL).
    let dacl: Vec<u16> = crate::windows_session::PIPE_DACL.encode_utf16().chain(std::iter::once(0)).collect();
    let mut sd = PSECURITY_DESCRIPTOR::default();
    unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(windows::core::PCWSTR(dacl.as_ptr()), SDDL_REVISION_1, &mut sd, None)?;
    }
    let mut attrs = SECURITY_ATTRIBUTES { nLength: size_of::<SECURITY_ATTRIBUTES>() as u32, lpSecurityDescriptor: sd.0, bInheritHandle: false.into() };
    let server = unsafe {
        tokio::net::windows::named_pipe::ServerOptions::new()
            .first_pipe_instance(first)
            .create_with_security_attributes_raw(path, &mut attrs as *mut _ as *mut _)
    };
    unsafe {
        let _ = windows::Win32::Foundation::LocalFree(Some(windows::Win32::Foundation::HLOCAL(sd.0)));
    }
    Ok(server?)
}

/// The process at the other end of `pipe`.
#[cfg(windows)]
fn client_pid(pipe: &tokio::net::windows::named_pipe::NamedPipeServer) -> Option<u32> {
    use std::os::windows::io::AsRawHandle;
    let mut pid = 0;
    let handle = windows::Win32::Foundation::HANDLE(pipe.as_raw_handle());
    unsafe { windows::Win32::System::Pipes::GetNamedPipeClientProcessId(handle, &mut pid) }.ok()?;
    Some(pid)
}

/// Whether the process at the other end of `pipe` runs elevated (an Administrator shell or SYSTEM).
// shortcut: a user-session Agent cannot open an elevated caller's token, so there nobody counts as root; the
// installed SYSTEM service (Service Mode, ticket 15) can.
#[cfg(windows)]
fn client_is_elevated(pipe: &tokio::net::windows::named_pipe::NamedPipeServer) -> bool {
    use windows::Win32::{
        Foundation::{CloseHandle, HANDLE},
        Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY},
        System::Threading::{OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION},
    };
    unsafe {
        let Some(pid) = client_pid(pipe) else { return false };
        let Ok(process) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else { return false };
        let mut token = HANDLE::default();
        let opened = OpenProcessToken(process, TOKEN_QUERY, &mut token).is_ok();
        let _ = CloseHandle(process);
        if !opened {
            return false;
        }
        let mut elevation = TOKEN_ELEVATION::default();
        let mut len = 0;
        let size = size_of::<TOKEN_ELEVATION>() as u32;
        let ok = GetTokenInformation(token, TokenElevation, Some(&mut elevation as *mut _ as *mut _), size, &mut len).is_ok();
        let _ = CloseHandle(token);
        ok && elevation.TokenIsElevated != 0
    }
}

/// Answers one request on `conn`. `caller` is from the peer credential.
async fn handle<S>(conn: S, status: Arc<Status>, caller: Caller) -> anyhow::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let (read, mut write) = tokio::io::split(conn);
    let mut line = String::new();
    BufReader::new(read).read_line(&mut line).await?;
    let request: Value = serde_json::from_str(&line)?;
    let reply = match caller == Caller::Owner {
        true => match authenticate_owner_change(&status, &request).await {
            Ok(()) => dispatch(&status, &request, true).await,
            Err(e) => json!({ "error": e }),
        },
        false => {
            let local = caller == Caller::Console && matches!(serde_json::from_value::<Request>(request.clone()), Ok(Request::Act { .. }));
            dispatch(&status, &request, caller == Caller::Root || local).await
        }
    };
    write.write_all(format!("{reply}\n").as_bytes()).await?;
    write.flush().await?;
    Ok(())
}

/// ADR 0013: a program running as the Device Owner (an AI agent, a script) makes an owner change only with a fresh
/// OS authentication of that account.
async fn authenticate_owner_change(status: &Status, request: &Value) -> Result<(), String> {
    if !serde_json::from_value::<Request>(request.clone()).is_ok_and(|r| r.is_owner_change()) {
        return Ok(());
    }
    let Ok(auth) = serde_json::from_value::<OsAuth>(request["os_auth"].clone()) else {
        return Err(OS_PASSWORD_REQUIRED.into());
    };
    let Some(platform) = status.platform.lock().unwrap().clone() else {
        return Err("the Agent is starting; try again".into());
    };
    // The dialog waits for the person at the Device.
    let checked = tokio::task::spawn_blocking(move || {
        let password = match &auth {
            OsAuth::Password(p) => Some(p.as_str()),
            OsAuth::Dialog => None,
        };
        platform.authenticate("rcli wants to change the Device Password or the Device Settings.", password)
    })
    .await;
    match checked {
        Ok(Ok(true)) => Ok(()),
        Ok(Ok(false)) => Err("the OS account password was not given or not accepted".into()),
        Ok(Err(e)) => Err(format!("{e:#}")),
        Err(e) => Err(e.to_string()),
    }
}

/// The reply to one request. Any local user may read the Device ID and status; everything else is root-only.
async fn dispatch(status: &Status, request: &Value, is_root: bool) -> Value {
    let parsed = match serde_json::from_value::<Request>(request.clone()) {
        Ok(parsed) => parsed,
        Err(e) => return json!({ "error": format!("bad request: {e}") }),
    };
    match parsed {
        Request::Info => {
            let mut info = status.info(is_root);
            let platform = status.platform.lock().unwrap().clone();
            if let Some(platform) = platform {
                info["install_mode"] = if platform.user_mode().is_some() { "user" } else { "service" }.into();
                // A session helper round trip blocks; `null` = no grant needed, or no helper to ask.
                let permissions = tokio::task::spawn_blocking(move || platform.capture_permissions().ok().flatten()).await;
                info["capture_permissions"] = json!(permissions.ok().flatten());
            }
            info
        }
        // Who is asking is status-adjacent: any local user may list the waiting prompts (see the `Prompts` variant).
        Request::Prompts => status.prompts().list(),
        Request::PromptAnswer { id, accept } => {
            // Anyone at the Device may refuse a prompt (like `disconnect`); only root may accept one,
            // so a local process cannot grant root or let a Controller in behind the person's back.
            if accept && !is_root {
                return json!({ "code": ROOT_REQUIRED, "error": "accepting a prompt requires root (run `sudo rcli agent prompt answer …`)" });
            }
            match status.prompts().answer(id, accept) {
                Ok(()) => json!({ "message": format!("prompt {id} {}", if accept { "accepted" } else { "rejected" }) }),
                Err(e) => json!({ "error": e }),
            }
        }
        Request::Disconnect { who } => {
            // Any local user may end a Controller: the person at the Device has the last word, no root needed.
            let who = who.trim();
            let removed = if who.is_empty() { status.controllers().clear() } else { status.controllers().remove(who) };
            for c in &removed {
                status.grant().revoke(c);
                status.request_disconnect(c);
            }
            let message =
                if removed.is_empty() { "no matching Controller was connected".to_owned() } else { format!("disconnected {}", removed.join(", ")) };
            json!({ "disconnected": removed, "message": message })
        }
        Request::CapabilitiesSet { who, mouse_keyboard, terminal, power } => {
            let apply = |c: &mut settings::Capabilities| {
                c.mouse_keyboard = mouse_keyboard.unwrap_or(c.mouse_keyboard);
                c.terminal = terminal.unwrap_or(c.terminal);
                c.power = power.unwrap_or(c.power);
            };
            let who = who.trim();
            // Switching one connection off is like `disconnect`, open to the person at the Device; anything else is an
            // owner change.
            let widens = who.is_empty() || [mouse_keyboard, terminal, power].contains(&Some(true));
            if widens && !is_root {
                return json!({ "code": ROOT_REQUIRED, "error": "switching a capability on requires the Device Owner, the OS user running the Agent, or root (`sudo rcli agent capability …`)" });
            }
            if who.is_empty() {
                return match status.settings().update(|s| apply(&mut s.capabilities)) {
                    Ok(s) => json!({ "capabilities": s.capabilities, "message": "the Controller capabilities were changed" }),
                    Err(e) => json!({ "error": format!("cannot save the Device Settings: {e:#}") }),
                };
            }
            match status.controllers().set_capabilities(who, apply).as_slice() {
                [] => json!({ "error": format!("no Controller called {who} is connected") }),
                changed => json!({ "changed": changed, "message": format!("changed the capabilities of {}", changed.join(", ")) }),
            }
        }
        // Local control is the Device Owner's own screen: no Server, password, Control Lock or Session Window, and
        // never the OS password (decision 14), so it is answered here, before the owner changes.
        Request::Act { .. } if !is_root => {
            json!({ "error": "controlling the Local Device requires the Device Owner (the OS user running the Agent, or root) or, in Service Mode, the user logged in at its screen" })
        }
        Request::Act { body } => local_act(status, body).await,
        _ if !is_root => {
            let command = request["command"].as_str().unwrap_or_default().replace('.', " ");
            json!({ "code": ROOT_REQUIRED, "error": format!("this requires the Device Owner, the OS user running the Agent, or root (`sudo rcli agent {command}`)") })
        }
        Request::PasswordShow => match status.secrets.get(DEVICE_PASSWORD) {
            Ok(Some(password)) => json!({ "password": password }),
            Ok(None) => json!({ "error": "no Device Password is stored" }),
            Err(e) => json!({ "error": format!("{e:#}") }),
        },
        Request::PasswordSet { password } => {
            if password.chars().count() < MIN_LENGTH {
                return json!({ "error": format!("the Device Password must be at least {MIN_LENGTH} characters") });
            }
            match status.identity(Some(&password), None).await {
                Ok(_) => match status.secrets.set(DEVICE_PASSWORD, &password) {
                    Ok(()) => json!({ "message": "the Device Password was changed" }),
                    Err(e) => json!({ "error": format!("the Server changed it, but the Device could not store it: {e:#}") }),
                },
                Err(e) => json!({ "error": e }),
            }
        }
        Request::IdSet { device_id } => match status.identity(None, Some(&device_id)).await {
            Ok(id) => json!({ "device_id": id, "message": format!("the Device ID is now {id}") }),
            Err(e) => json!({ "error": e }),
        },
        Request::TerminalUserSet { user } => {
            let user = user.trim().to_owned();
            if user.is_empty() || user.len() > 64 || user.chars().any(|c| c.is_whitespace() || c == ':') {
                return json!({ "error": "the Terminal User must be a local account name (1-64 characters, no spaces)" });
            }
            match status.settings().update(|s| s.terminal_user = user.clone()) {
                Ok(s) => json!({ "terminal_user": s.terminal_user, "message": format!("terminal commands and shells now run as {user}") }),
                Err(e) => json!({ "error": format!("cannot save the Device Settings: {e:#}") }),
            }
        }
        Request::RootSet { root } => match status.settings().update(|s| s.root = root) {
            Ok(s) => json!({ "root": s.root.name(), "message": format!("root access is now {}", s.root.name()) }),
            Err(e) => json!({ "error": format!("cannot save the Device Settings: {e:#}") }),
        },
        Request::AcceptSet { accept } => match status.settings().update(|s| s.accept = accept) {
            Ok(s) => json!({ "accept": s.accept.name(), "message": format!("new connections now ask for {}", s.accept.name()) }),
            Err(e) => json!({ "error": format!("cannot save the Device Settings: {e:#}") }),
        },
        Request::GeneralSet { auto_update, session_window } => match status.settings().update(|s| {
            s.auto_update = auto_update.unwrap_or(s.auto_update);
            s.session_window = session_window.unwrap_or(s.session_window);
        }) {
            Ok(s) => json!({ "auto_update": s.auto_update, "session_window": s.session_window, "message": "the General settings were changed" }),
            Err(e) => json!({ "error": format!("cannot save the Device Settings: {e:#}") }),
        },
        Request::RootGrant { who } => {
            // The person at the Device: allow root for the Controller named here, for this session only.
            let who = who.trim();
            if who.is_empty() {
                return json!({ "error": "say who the root grant is for" });
            }
            status.grant().grant(who);
            json!({ "message": format!("{who} may use root for this session") })
        }
        Request::LockRelease => {
            if !status.connected.load(Ordering::Relaxed) {
                return json!({ "error": "the Agent is not connected to the Server; try again once it is" });
            }
            let _ = status.lock_releases.send(());
            json!({ "message": "the Control Lock was released" })
        }
        Request::ServerSet { server, key } => {
            let (addr, key) = (server.trim(), key.trim());
            if addr.is_empty() || ServerKey::parse(key).is_none() {
                return json!({ "error": "give the Server Address (host:port) and a Server Key rk_<fingerprint>_<secret>" });
            }
            status.set_server_access(ServerAccess { addr: addr.into(), server_key: key.into() });
            json!({ "server": addr, "message": format!("the Agent is reconnecting to {addr}") })
        }
    }
}

/// Runs an Action request body on this Device as the Controller `local`; errors read as the Controller API's.
async fn local_act(status: &Status, body: Value) -> Value {
    use rcli_protocol::act;
    let failed = |code: &str, message: String| json!({ "error": { "code": code, "message": message, "hint": rcli_protocol::hint(code) } });
    let parsed = match act::parse(body) {
        Ok(parsed) => parsed,
        Err(act::Refused::BadRequest(m)) => return failed("bad_request", m),
        Err(act::Refused::Invalid(m)) => return failed("invalid_action", m),
    };
    let Some(platform) = status.platform.lock().unwrap().clone() else {
        return failed("platform", "the Agent is still starting; try again".into());
    };
    // Images come back in the reply: there is no media channel to the Local Device.
    let req = rcli_protocol::agent::ActRequest { controller: "local".into(), media_session: String::new(), ..parsed.act };
    let executor = status.executor();
    // Like the remote path: User Mode cannot reach the lock and login screens (decision 10).
    let acted = tokio::task::spawn_blocking(move || match crate::out_of_user_mode_reach(platform.as_ref()) {
        Some(e) => Err(e),
        None => Ok(executor.lock().unwrap().act(platform.as_ref(), &req)),
    });
    let mut result = match acted.await {
        Ok(Ok(result)) => result,
        Ok(Err(e)) => return failed(&e.code, e.message),
        Err(e) => return failed("platform", format!("{e}")),
    };
    let Some(e) = result.error.take() else { return act::observation(result) };
    let mut reply = failed(&e.code, e.message);
    if let Some(step) = result.failed_step.and_then(|s| parsed.step_of.get(s as usize)) {
        reply["error"]["step"] = (*step).into();
    }
    // A timed-out Wait Condition still shows the screen as it was.
    if e.code == "wait_timeout" {
        reply["error"].as_object_mut().unwrap().extend(act::observation(result).as_object().cloned().unwrap_or_default());
    }
    reply
}

/// Sends one request to the Agent at `path` and returns its reply (an `error` reply is returned as `Err`).
pub async fn send(path: &Path, request: &Request) -> anyhow::Result<Value> {
    send_value(path, serde_json::to_value(request)?).await
}

/// `send` for an owner change, with the Device Owner's fresh OS authentication (ADR 0013).
pub async fn send_authenticated(path: &Path, request: &Request, auth: &OsAuth) -> anyhow::Result<Value> {
    let mut request = serde_json::to_value(request)?;
    request["os_auth"] = serde_json::to_value(auth)?;
    send_value(path, request).await
}

async fn send_value(path: &Path, request: Value) -> anyhow::Result<Value> {
    let request = request.to_string();
    #[cfg(unix)]
    let stream = tokio::net::UnixStream::connect(path).await;
    #[cfg(windows)]
    let stream = tokio::net::windows::named_pipe::ClientOptions::new().open(path);
    let stream = stream.map_err(|e| anyhow::anyhow!("cannot reach the Agent at {}: {e}", path.display()))?;
    // Pipe names are machine-wide: another account could create this user's pipe first. Only our own Agent may get
    // a Device Password or Server Key we send.
    #[cfg(windows)]
    if own_pipe().as_deref() == Some(path) {
        use std::os::windows::io::AsRawHandle;
        let mut pid = 0;
        let handle = windows::Win32::Foundation::HANDLE(stream.as_raw_handle());
        let server = unsafe { windows::Win32::System::Pipes::GetNamedPipeServerProcessId(handle, &mut pid) }.ok().and_then(|()| process_user_sid(Some(pid)));
        anyhow::ensure!(server.is_some() && server == process_user_sid(None), "{} is not served by this user's Agent", path.display());
    }
    let (read, mut write) = tokio::io::split(stream);
    write.write_all(format!("{request}\n").as_bytes()).await?;
    write.flush().await?;
    let mut line = String::new();
    BufReader::new(read).read_line(&mut line).await?;
    let reply: Value = serde_json::from_str(&line)?;
    match reply["error"].as_str() {
        Some(e) => Err(Refused { code: reply["code"].as_str().unwrap_or_default().into(), message: e.into() }.into()),
        None => Ok(reply),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secret_store::FileSecretStore;

    #[test]
    fn only_root_may_show_or_change_the_device() {
        let dir = std::env::temp_dir().join(format!("rcli-ipc-dispatch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = FileSecretStore::new(&dir);
        store.set(DEVICE_PASSWORD, "abcd2345ef").unwrap();
        let status = Status::new("123456789", ServerAccess { addr: "localhost:1".into(), server_key: String::new() }, Arc::new(store));

        let show = json!({ "command": "password.show" });
        let block = tokio::runtime::Runtime::new().unwrap();
        assert_eq!(block.block_on(dispatch(&status, &show, true))["password"], "abcd2345ef");
        for command in ["password.show", "id.set", "server.set", "password.set", "lock.release"] {
            let reply = block.block_on(dispatch(&status, &json!({ "command": command }), false));
            assert!(reply["error"].as_str().unwrap().contains("root"), "{command}: {reply}");
        }
        assert_eq!(block.block_on(dispatch(&status, &json!({ "command": "info" }), false))["device_id"], "123456789");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_password_change_is_at_least_eight_characters() {
        let dir = std::env::temp_dir().join(format!("rcli-ipc-short-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = FileSecretStore::new(&dir);
        let status = Status::new("123456789", ServerAccess { addr: "localhost:1".into(), server_key: String::new() }, Arc::new(store));

        let reply = tokio::runtime::Runtime::new().unwrap().block_on(dispatch(&status, &json!({ "command": "password.set", "password": "short" }), true));
        assert!(reply["error"].as_str().unwrap().contains("8 characters"), "{reply}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn prompts_are_listed_answered_or_rejected_on_timeout() {
        let prompts = Arc::new(Prompts::default());
        let answerer = {
            let prompts = prompts.clone();
            std::thread::spawn(move || {
                for _ in 0..1000 {
                    let listed = prompts.list();
                    if let Some(p) = listed.as_array().and_then(|a| a.first()) {
                        prompts.answer(p["id"].as_u64().unwrap(), true).unwrap();
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
                panic!("the prompt was never listed");
            })
        };
        assert!(prompts.ask(PromptKind::Root, "alice", Duration::from_secs(5)));
        answerer.join().unwrap();
        assert!(prompts.list().as_array().unwrap().is_empty(), "{:?}", prompts.list());
        // Nobody answers: the prompt auto-rejects when its time runs out.
        assert!(!prompts.ask(PromptKind::Accept, "dev-laptop (10.0.0.5)", Duration::from_millis(50)));
        assert!(prompts.answer(999, true).is_err());
    }

    #[test]
    fn anyone_may_refuse_a_prompt_but_only_root_accepts_and_sees_secrets() {
        let dir = std::env::temp_dir().join(format!("rcli-ipc-prompts-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = FileSecretStore::new(&dir);
        let status = Status::new(
            "123456789",
            ServerAccess { addr: "localhost:1".into(), server_key: "rk_deadbeef_secret".into() },
            Arc::new(store),
        );
        let block = tokio::runtime::Runtime::new().unwrap();
        let prompts = status.prompts();

        let waiting = |kind, who| {
            let prompts = prompts.clone();
            std::thread::spawn(move || prompts.ask(kind, who, Duration::from_secs(5)))
        };
        let next_id = |prompts: &Prompts| loop {
            let listed = prompts.list();
            if let Some(p) = listed.as_array().and_then(|a| a.first()) {
                break p["id"].as_u64().unwrap();
            }
            std::thread::sleep(Duration::from_millis(5));
        };

        let asker = waiting(PromptKind::Root, "alice");
        let id = next_id(&prompts);
        let accept = block.block_on(dispatch(&status, &json!({ "command": "prompt.answer", "id": id, "accept": true }), false));
        assert!(accept["error"].as_str().unwrap().contains("root"), "{accept}");
        let reject = block.block_on(dispatch(&status, &json!({ "command": "prompt.answer", "id": id, "accept": false }), false));
        assert!(reject["message"].as_str().is_some(), "{reject}");
        assert!(!asker.join().unwrap());

        let asker = waiting(PromptKind::Accept, "dev-laptop (10.0.0.5)");
        let id = next_id(&prompts);
        let accept = block.block_on(dispatch(&status, &json!({ "command": "prompt.answer", "id": id, "accept": true }), true));
        assert!(accept["message"].as_str().is_some(), "{accept}");
        assert!(asker.join().unwrap());

        // The Device Settings and the Server Key appear in `info` only for root.
        let as_user = block.block_on(dispatch(&status, &json!({ "command": "info" }), false));
        assert!(as_user.get("settings").is_none() && as_user.get("server_key").is_none(), "{as_user}");
        let as_root = block.block_on(dispatch(&status, &json!({ "command": "info" }), true));
        assert!(as_root.get("settings").is_some(), "{as_root}");
        assert_eq!(as_root["server_key"], "rk_deadbeef_secret");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
