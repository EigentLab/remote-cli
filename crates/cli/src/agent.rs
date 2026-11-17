//! `rcli agent run`: the Agent side of the binary. The Agent itself stays a library crate.

use std::{path::PathBuf, sync::Arc};

use clap::Args;
use rcli_agent::{
    ipc,
    secret_store::SecretStore,
    settings::{AcceptMode, RootAccess},
    Credentials, ServerAccess,
};

/// Keeps a registered Device connected: the Agent entry point `rcli agent run` and the test system share.
pub async fn serve(
    server: ServerAccess,
    creds: Credentials,
    platform: Arc<dyn rcli_agent::platform::Platform>,
    status: Option<Arc<ipc::Status>>,
) {
    rcli_agent::run(server, creds, platform, status).await;
}

/// Everything an Agent needs from an install: `rcli agent run` builds it from its arguments, the Windows service
/// host (`windows_service.rs`) from the protected install config.
pub struct DeviceConfig {
    /// Empty: no Server yet (User Mode, ADR 0012); the one saved in `state_dir`, else none until it is set.
    pub server: String,
    pub key: String,
    pub state_dir: PathBuf,
    /// The Controller configuration the Agent keeps current: machine-wide for root, the user's own in User Mode.
    pub controller_config: PathBuf,
    pub name: String,
    /// Local IPC endpoint the GUI and `rcli agent …` use.
    pub socket: PathBuf,
    /// macOS/Linux: the socket the GUI session helpers connect to.
    pub session_socket: Option<PathBuf>,
    /// Windows: the pipe the SYSTEM service accepts session helpers on.
    pub session_pipe: Option<String>,
}

/// Starts the Agent: registers on first start, serves the local IPC and the Server connection.
pub async fn serve_device(config: DeviceConfig) -> anyhow::Result<()> {
    #[cfg(target_os = "linux")]
    let platform = Arc::new(rcli_agent::linux::LinuxPlatform::with_state_dir(config.name.clone(), &config.state_dir)?);
    #[cfg(target_os = "macos")]
    let platform = Arc::new(rcli_agent::macos::MacPlatform::new(config.name.clone())?);
    #[cfg(windows)]
    let platform = Arc::new(rcli_agent::windows_platform::WindowsPlatform::new(config.name.clone())?);
    // shortcut: no adapter for other OSes, so the binary runs on the FakePlatform there.
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    let platform = Arc::new(rcli_agent::platform::FakePlatform::new(config.name.clone()));
    // The installed root daemon cannot reach the window server: session helpers do the screen work (tickets 16,
    // 24). Linux runs one helper for seat0 and reconciles it on login/logout.
    #[cfg(target_os = "linux")]
    let platform: Arc<dyn rcli_agent::platform::Platform> = match &config.session_socket {
        Some(socket) => rcli_agent::linux::session::console(platform, socket, Some(&config.state_dir))?,
        None => platform,
    };
    #[cfg(target_os = "macos")]
    let platform: Arc<dyn rcli_agent::platform::Platform> = match &config.session_socket {
        Some(socket) => rcli_agent::macos_session::SessionPlatform::listen(platform, socket)?,
        None => platform,
    };
    // The SYSTEM service is in session 0: a helper in the console session does the screen work (ticket 22).
    #[cfg(windows)]
    let platform: Arc<dyn rcli_agent::platform::Platform> = match &config.session_pipe {
        Some(pipe) => rcli_agent::windows_session::SessionPlatform::listen_pipe(platform, pipe)?,
        None => platform,
    };
    serve_device_on(config, platform).await
}

/// `serve_device` on `platform` (the test system passes a FakePlatform). The local IPC is up from the start; with no
/// Server the Agent waits, unregistered, until the Device Owner sets one, then registers and connects.
pub async fn serve_device_on(config: DeviceConfig, platform: Arc<dyn rcli_agent::platform::Platform>) -> anyhow::Result<()> {
    let state = config.state_dir;
    let saved = ipc::saved_server(&state);
    let server = match saved {
        Some(saved) if config.server.is_empty() => saved,
        _ => ServerAccess { addr: config.server, server_key: config.key },
    };
    let path = state.join("credentials.json");
    let secrets = secret_store(&state);
    let known: Option<Credentials> = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice(&b).ok());
    let status = Arc::new(ipc::Status::new(known.as_ref().map_or("", |c| &c.device_id), server, secrets.clone()));
    // Device Settings live beside the credentials, so `rcli agent …` changes survive a restart.
    status.set_settings_dir(state.clone());
    // A new Device ID set at the Device must survive a restart.
    status.set_credentials_path(path);
    // A Server set at the Device must survive a restart too.
    status.set_server_file(state.join(ipc::SERVER_FILE));
    // Controllers on this machine call the API Address the Server sends on every connection.
    status.set_controller_config(config.controller_config.clone());
    // Desktop build: prompts and control events open the GUI window (ticket 14).
    #[cfg(feature = "gui")]
    let platform = crate::gui::wrap(platform, status.prompts(), status.settings(), status.controllers(), config.socket.clone());
    // Owner changes need the OS authentication (`server set`, ADR 0013) and local control works before (and
    // without) a Server.
    status.set_platform(platform.clone());
    let serve_status = status.clone();
    let socket = config.socket;
    tokio::spawn(async move {
        if let Err(e) = ipc::serve(socket, serve_status).await {
            tracing::error!("local IPC stopped: {e:#}");
        }
    });
    let mut changed = status.subscribe();
    let creds = loop {
        let server = status.server_access();
        if !server.addr.is_empty() {
            match load_or_register(&server, platform.as_ref(), &state, secrets.as_ref()).await {
                Ok(creds) => break creds,
                Err(e) => tracing::warn!("cannot register with {}: {e:#}", server.addr),
            }
        }
        // No Server (or it refused): wait for the Device Owner to set one, retrying a refusal now and then.
        tokio::select! {
            _ = changed.changed() => {}
            _ = tokio::time::sleep(std::time::Duration::from_secs(30)), if !server.addr.is_empty() => {}
        }
    };
    status.set_device_id(&creds.device_id);
    let server = status.server_access();
    // An install made with no Server gets its Controller configuration once it has one.
    if !config.controller_config.exists() {
        let machine = crate::config::Machine { server: server.addr.clone(), server_key: server.server_key.clone(), api_address: creds.api_address.clone(), ca_pem: rcli_agent::cert_pem(&creds.ca_der) };
        let saved = crate::config::save_machine(&config.controller_config, &machine);
        // A user's own configuration holds the Server Key: theirs only (the machine-wide one is root's to write).
        #[cfg(unix)]
        let saved = saved.and_then(|()| match crate::upgrade::is_root() {
            true => Ok(()),
            false => Ok(std::fs::set_permissions(&config.controller_config, std::os::unix::fs::PermissionsExt::from_mode(0o600))?),
        });
        if let Err(e) = saved {
            tracing::warn!("cannot write {}: {e:#}", config.controller_config.display());
        }
    }
    tracing::info!("Device {} connecting to {}", creds.device_id, server.addr);
    serve(server, creds, platform, Some(status)).await;
    Ok(())
}

#[derive(Args)]
pub struct Run {
    /// Server Agent port, host:port; default: the Server saved in the state directory, else none until one is set.
    #[arg(long, env = "RCLI_AGENT_SERVER", default_value = "")]
    server: String,
    /// Server Key, rk_<CA fingerprint>_<secret>.
    #[arg(long, env = "RCLI_SERVER_KEY", hide_env_values = true, default_value = "")]
    key: String,
    /// Where the Device credentials are kept.
    #[arg(long, env = "RCLI_AGENT_STATE_DIR")]
    state_dir: PathBuf,
    /// Device name reported at registration.
    #[arg(long)]
    name: String,
    /// Local IPC socket `rcli agent ...`, the GUI and scripts use.
    #[arg(long, env = "RCLI_AGENT_SOCKET")]
    socket: Option<PathBuf>,
    /// Root daemon: where the console session helpers (`rcli agent session`) connect to do the screen work.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[arg(long)]
    session_socket: Option<PathBuf>,
}

/// The Device's credentials from `state_dir`, or a new registration saved there. The Device Password goes to
/// `secrets`, never into `credentials.json`.
pub async fn load_or_register(
    server: &ServerAccess,
    platform: &dyn rcli_agent::platform::Platform,
    state_dir: &std::path::Path,
    secrets: &dyn SecretStore,
) -> anyhow::Result<Credentials> {
    let path = state_dir.join("credentials.json");
    if let Ok(bytes) = std::fs::read(&path) {
        return Ok(serde_json::from_slice(&bytes)?);
    }
    let creds = rcli_agent::register(server, platform).await?;
    std::fs::create_dir_all(state_dir)?;
    secrets.set(ipc::DEVICE_PASSWORD, &creds.device_password)?;
    std::fs::write(&path, serde_json::to_vec(&creds)?)?;
    #[cfg(unix)]
    std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o600))?;
    Ok(creds)
}

/// The Secret Store: DPAPI on Windows (machine scope for the elevated install and the SYSTEM service, the user's own
/// scope in User Mode), `systemd-creds` for the root service on Linux (the
/// encrypted-file fallback when it is not available); otherwise the encrypted-file store in the root-only state
/// dir. Not the macOS System Keychain: its items trust only the ad-hoc build that wrote them, so every upgrade
/// would lose the Device Password.
pub fn secret_store(state_dir: &std::path::Path) -> Arc<dyn SecretStore> {
    #[cfg(target_os = "linux")]
    if crate::upgrade::is_root() && rcli_agent::secret_store::systemd_creds_available() {
        return Arc::new(rcli_agent::secret_store::SystemdCredsSecretStore::new(state_dir));
    }
    #[cfg(windows)]
    return Arc::new(rcli_agent::secret_store::DpapiSecretStore::new(state_dir.join("secrets"), crate::upgrade::is_root()));
    #[cfg(not(windows))]
    Arc::new(rcli_agent::secret_store::FileSecretStore::new(state_dir))
}

impl Run {
    pub async fn run(self) -> anyhow::Result<()> {
        let _ = tracing_subscriber::fmt().try_init();
        let config = DeviceConfig {
            server: self.server,
            key: self.key,
            state_dir: self.state_dir,
            // Root keeps the machine-wide one; a User Mode Agent its user's own (ADR 0012).
            controller_config: match crate::upgrade::is_root() {
                true => crate::config::machine_path(None),
                false => crate::config::dir().join(crate::config::CONTROLLER_FILE),
            },
            name: self.name,
            socket: self.socket.unwrap_or_else(ipc::default_socket),
            #[cfg(unix)]
            session_socket: self.session_socket,
            #[cfg(not(unix))]
            session_socket: None,
            session_pipe: None,
        };
        serve_device(config).await
    }
}

/// `rcli agent info`: the Device ID and connection status, readable by any local user.
#[derive(Args)]
pub struct Info {
    /// Local IPC socket of the running Agent.
    #[arg(long, env = "RCLI_AGENT_SOCKET")]
    socket: Option<PathBuf>,
    /// Machine-readable JSON output.
    #[arg(long)]
    json: bool,
}

impl Info {
    pub async fn run(self, out: &mut dyn std::io::Write) -> anyhow::Result<()> {
        let reply = send(&self.socket.unwrap_or_else(ipc::default_socket), &ipc::Request::Info).await?;
        if self.json {
            writeln!(out, "{}", serde_json::to_string_pretty(&reply)?)?;
        } else {
            writeln!(
                out,
                "device id {} ({})\nserver {}",
                reply["device_id"].as_str().unwrap_or_default(),
                if reply["connected"] == true { "connected" } else { "disconnected" },
                reply["server"].as_str().unwrap_or_default(),
            )?;
            let controllers = reply["controllers"].as_array().map(Vec::as_slice).unwrap_or_default();
            if !controllers.is_empty() {
                writeln!(out, "controlled by:")?;
                for c in controllers {
                    writeln!(out, "  {}", c.as_str().unwrap_or_default())?;
                }
            }
        }
        Ok(())
    }
}

/// One request to the running Agent. An owner change made as the Device Owner (not root) asks for the OS account
/// password on the tty and is refused without one, so a script cannot loosen the Device (ADR 0013).
async fn send(socket: &std::path::Path, request: &ipc::Request) -> anyhow::Result<serde_json::Value> {
    use std::io::IsTerminal;
    match ipc::send(socket, request).await {
        Err(e) if e.to_string() == ipc::OS_PASSWORD_REQUIRED => {
            anyhow::ensure!(std::io::stdin().is_terminal(), "this asks for your OS account password and there is no terminal to ask on");
            eprint!("Your OS account password: ");
            let password = rpassword::read_password().map_err(|e| anyhow::anyhow!("cannot read the password: {e}"))?;
            ipc::send_authenticated(socket, request, &ipc::OsAuth::Password(password)).await
        }
        reply => reply,
    }
}

/// `rcli agent disconnect [<who>]`: end a Controller's Live View and shells and release its Control Lock.
/// Allowed for any local user: the person at the Device can always stop a session on their Device.
#[derive(Args)]
pub struct Disconnect {
    /// The Controller as shown at the Device, e.g. `alice` or `dev-laptop (10.0.0.5)`. Omit to disconnect all.
    who: Option<String>,
    /// Local IPC socket of the running Agent.
    #[arg(long, env = "RCLI_AGENT_SOCKET")]
    socket: Option<PathBuf>,
}

impl Disconnect {
    pub async fn run(self, out: &mut dyn std::io::Write) -> anyhow::Result<()> {
        let socket = self.socket.unwrap_or_else(ipc::default_socket);
        let reply = send(&socket, &ipc::Request::Disconnect { who: self.who.unwrap_or_default() }).await?;
        writeln!(out, "{}", reply["message"].as_str().unwrap_or_default())?;
        Ok(())
    }
}

/// `rcli agent release-lock`: end whoever's Control Lock it is (the Device Owner only); the holder is told on its
/// next input Action.
#[derive(Args)]
pub struct ReleaseLock {
    /// Local IPC socket of the running Agent.
    #[arg(long, env = "RCLI_AGENT_SOCKET")]
    socket: Option<PathBuf>,
}

impl ReleaseLock {
    pub async fn run(self, out: &mut dyn std::io::Write) -> anyhow::Result<()> {
        let socket = self.socket.unwrap_or_else(ipc::default_socket);
        let reply = send(&socket, &ipc::Request::LockRelease).await?;
        writeln!(out, "{}", reply["message"].as_str().unwrap_or_default())?;
        Ok(())
    }
}

/// `rcli agent password ...`: show or change the Device Password (root only).
#[derive(Args)]
pub struct Password {
    #[command(subcommand)]
    command: PasswordCommand,
    /// Local IPC socket of the running Agent.
    #[arg(long, env = "RCLI_AGENT_SOCKET", global = true)]
    socket: Option<PathBuf>,
}

#[derive(clap::Subcommand)]
pub enum PasswordCommand {
    /// Show the Device Password (requires root/Administrator).
    Show {
        /// Machine-readable JSON output.
        #[arg(long)]
        json: bool,
    },
    /// Replace the Device Password (requires root/Administrator); open sessions keep working.
    Set {
        /// At least 8 characters. Reads one line from stdin when omitted, so an elevated helper
        /// never puts a password on a command line.
        #[arg(long, env = "RCLI_DEVICE_PASSWORD", hide_env_values = true)]
        password: Option<String>,
    },
}

/// `rcli agent password show|set`: the running Agent refuses non-root callers itself (peer credentials).
pub async fn run_password(command: PasswordCommand, socket: Option<PathBuf>, out: &mut dyn std::io::Write) -> anyhow::Result<()> {
    let socket = socket.unwrap_or_else(ipc::default_socket);
    match command {
        PasswordCommand::Show { json: true } => {
            let reply = send(&socket, &ipc::Request::PasswordShow).await?;
            writeln!(out, "{}", serde_json::to_string_pretty(&reply)?)?;
        }
        PasswordCommand::Show { json: false } => {
            let reply = send(&socket, &ipc::Request::PasswordShow).await?;
            writeln!(out, "{}", reply["password"].as_str().unwrap_or_default())?;
        }
        PasswordCommand::Set { password } => {
            let password = match password {
                Some(p) => p,
                None => {
                    let mut line = String::new();
                    std::io::stdin().read_line(&mut line)?;
                    line.trim_end_matches(['\n', '\r']).to_owned()
                }
            };
            let reply = send(&socket, &ipc::Request::PasswordSet { password }).await?;
            writeln!(out, "{}", reply["message"].as_str().unwrap_or_default())?;
        }
    }
    Ok(())
}

/// `rcli agent id ...`: the Device ID (root only).
#[derive(Args)]
pub struct Id {
    #[command(subcommand)]
    command: IdCommand,
    /// Local IPC socket of the running Agent.
    #[arg(long, env = "RCLI_AGENT_SOCKET", global = true)]
    socket: Option<PathBuf>,
}

#[derive(clap::Subcommand)]
pub enum IdCommand {
    /// Replace the Device ID with a free 9-digit one (requires root/Administrator).
    Set { device_id: String },
}

pub async fn run_id(command: IdCommand, socket: Option<PathBuf>, out: &mut dyn std::io::Write) -> anyhow::Result<()> {
    let reply = match command {
        IdCommand::Set { device_id } => send(&socket.unwrap_or_else(ipc::default_socket), &ipc::Request::IdSet { device_id }).await?,
    };
    writeln!(out, "{}", reply["message"].as_str().unwrap_or_default())?;
    Ok(())
}

/// `rcli agent terminal-user ...`: the account terminal commands and shells run as (root only).
#[derive(Args)]
pub struct TerminalUser {
    #[command(subcommand)]
    command: TerminalUserCommand,
    /// Local IPC socket of the running Agent.
    #[arg(long, env = "RCLI_AGENT_SOCKET", global = true)]
    socket: Option<PathBuf>,
}

#[derive(clap::Subcommand)]
pub enum TerminalUserCommand {
    /// Run terminal commands and shells as this account (requires root/Administrator).
    Set { user: String },
}

pub async fn run_terminal_user(command: TerminalUserCommand, socket: Option<PathBuf>, out: &mut dyn std::io::Write) -> anyhow::Result<()> {
    let reply = match command {
        TerminalUserCommand::Set { user } => {
            send(&socket.unwrap_or_else(ipc::default_socket), &ipc::Request::TerminalUserSet { user }).await?
        }
    };
    writeln!(out, "{}", reply["message"].as_str().unwrap_or_default())?;
    Ok(())
}

/// `rcli agent root never|ask|always`: whether `--as root` is allowed (root only); `root grant` is for the person
/// at the Device (the GUI), allowing root to one Controller for the current session.
#[derive(Args)]
pub struct Root {
    #[command(subcommand)]
    command: RootCommand,
    /// Local IPC socket of the running Agent.
    #[arg(long, env = "RCLI_AGENT_SOCKET", global = true)]
    socket: Option<PathBuf>,
}

#[derive(clap::Subcommand)]
pub enum RootCommand {
    /// Root is never allowed, whoever asks.
    Never,
    /// A person at the Device decides each time (the default).
    Ask,
    /// Root is always allowed for `--as root`.
    Always,
    /// Allow root to `who` for the current session only, without changing the setting.
    Grant {
        /// The Controller as shown at the Device, e.g. `alice (Dashboard)` (or just `alice`) or, for a password
        /// connection, its full name `dev-laptop (10.0.0.5)`.
        who: String,
    },
}

impl Root {
    pub async fn run(self, out: &mut dyn std::io::Write) -> anyhow::Result<()> {
        let socket = self.socket.unwrap_or_else(ipc::default_socket);
        let reply = match self.command {
            RootCommand::Grant { who } => send(&socket, &ipc::Request::RootGrant { who }).await?,
            RootCommand::Never => send(&socket, &ipc::Request::RootSet { root: RootAccess::Never }).await?,
            RootCommand::Ask => send(&socket, &ipc::Request::RootSet { root: RootAccess::Ask }).await?,
            RootCommand::Always => send(&socket, &ipc::Request::RootSet { root: RootAccess::Always }).await?,
        };
        writeln!(out, "{}", reply["message"].as_str().unwrap_or_default())?;
        Ok(())
    }
}

/// `rcli agent accept ...`: whether a person at the Device must accept each new connection (root only).
#[derive(Args)]
pub struct Accept {
    #[command(subcommand)]
    command: AcceptCommand,
    /// Local IPC socket of the running Agent.
    #[arg(long, env = "RCLI_AGENT_SOCKET", global = true)]
    socket: Option<PathBuf>,
}

#[derive(clap::Subcommand)]
pub enum AcceptCommand {
    /// A Controller with the Device Password connects at once.
    Password,
    /// A person at the Device accepts each new connection (rejected after 30 s).
    #[command(name = "password+accept")]
    PasswordAccept,
}

impl Accept {
    pub async fn run(self, out: &mut dyn std::io::Write) -> anyhow::Result<()> {
        let socket = self.socket.unwrap_or_else(ipc::default_socket);
        let accept = match self.command {
            AcceptCommand::Password => AcceptMode::Password,
            AcceptCommand::PasswordAccept => AcceptMode::PasswordAccept,
        };
        let reply = send(&socket, &ipc::Request::AcceptSet { accept }).await?;
        writeln!(out, "{}", reply["message"].as_str().unwrap_or_default())?;
        Ok(())
    }
}

/// `rcli agent capability <mouse-keyboard|terminal|power> on|off [--controller <who>]`: a Device Settings switch
/// (root only), or one connection's switch within it (anyone may switch off, root switches back on).
#[derive(Args)]
pub struct Capability {
    #[arg(value_parser = ["mouse-keyboard", "terminal", "power"])]
    capability: String,
    #[arg(value_parser = ["on", "off"])]
    state: String,
    /// Only this connection, as shown at the Device, e.g. `dev-laptop (10.0.0.5)`.
    #[arg(long)]
    controller: Option<String>,
    /// Local IPC socket of the running Agent.
    #[arg(long, env = "RCLI_AGENT_SOCKET")]
    socket: Option<PathBuf>,
}

impl Capability {
    pub async fn run(self, out: &mut dyn std::io::Write) -> anyhow::Result<()> {
        let socket = self.socket.unwrap_or_else(ipc::default_socket);
        let on = Some(self.state == "on");
        let pick = |name: &str| if self.capability == name { on } else { None };
        let request = ipc::Request::CapabilitiesSet {
            who: self.controller.clone().unwrap_or_default(),
            mouse_keyboard: pick("mouse-keyboard"),
            terminal: pick("terminal"),
            power: pick("power"),
        };
        let reply = send(&socket, &request).await?;
        writeln!(out, "{}", reply["message"].as_str().unwrap_or_default())?;
        Ok(())
    }
}

/// `rcli agent general <auto-update|session-window> on|off`: Settings > General (the Device Owner).
#[derive(Args)]
pub struct General {
    #[arg(value_parser = ["auto-update", "session-window"])]
    setting: String,
    #[arg(value_parser = ["on", "off"])]
    state: String,
    /// Local IPC socket of the running Agent.
    #[arg(long, env = "RCLI_AGENT_SOCKET")]
    socket: Option<PathBuf>,
}

impl General {
    pub async fn run(self, out: &mut dyn std::io::Write) -> anyhow::Result<()> {
        let socket = self.socket.unwrap_or_else(ipc::default_socket);
        let on = Some(self.state == "on");
        let pick = |name: &str| if self.setting == name { on } else { None };
        let request = ipc::Request::GeneralSet { auto_update: pick("auto-update"), session_window: pick("session-window") };
        let reply = send(&socket, &request).await?;
        writeln!(out, "{}", reply["message"].as_str().unwrap_or_default())?;
        Ok(())
    }
}

/// `rcli agent server ...`: which Server the Agent connects to (root only).
#[derive(Args)]
pub struct Server {
    #[command(subcommand)]
    command: ServerCommand,
    /// Local IPC socket of the running Agent.
    #[arg(long, env = "RCLI_AGENT_SOCKET", global = true)]
    socket: Option<PathBuf>,
}

#[derive(clap::Subcommand)]
pub enum ServerCommand {
    /// Point the Agent at another Server; it reconnects at once (requires root/Administrator).
    Set {
        /// host:port of the Server's Agent port.
        address: String,
        /// rk_<CA fingerprint>_<secret>. Reads one line from stdin when omitted, so an elevated helper never puts
        /// the key on a command line.
        #[arg(long, env = "RCLI_SERVER_KEY", hide_env_values = true)]
        key: Option<String>,
    },
}

pub async fn run_server(command: ServerCommand, socket: Option<PathBuf>, out: &mut dyn std::io::Write) -> anyhow::Result<()> {
    let reply = match command {
        ServerCommand::Set { address, key } => {
            let key = match key {
                Some(key) => key,
                None => {
                    let mut line = String::new();
                    std::io::stdin().read_line(&mut line)?;
                    line.trim().to_owned()
                }
            };
            send(&socket.unwrap_or_else(ipc::default_socket), &ipc::Request::ServerSet { server: address, key }).await?
        }
    };
    writeln!(out, "{}", reply["message"].as_str().unwrap_or_default())?;
    Ok(())
}

/// `rcli agent prompt answer`: the person at the Device's answer to a waiting prompt (tickets 07/08).
/// Refusing is open to any local user; accepting needs root, so the GUI runs this command elevated.
#[derive(Args)]
pub struct Prompt {
    #[command(subcommand)]
    pub command: PromptCommand,
    /// Local IPC socket of the running Agent.
    #[arg(long, env = "RCLI_AGENT_SOCKET", global = true)]
    pub socket: Option<PathBuf>,
}

#[derive(clap::Subcommand)]
pub enum PromptCommand {
    /// Accept or refuse the prompt with this id (`rcli agent prompts` lists them).
    Answer {
        id: u64,
        #[arg(value_parser = ["accept", "reject"])]
        decision: String,
    },
}

/// `rcli agent prompts`: the prompts waiting for the person at the Device (any local user).
#[derive(Args)]
pub struct Prompts {
    /// Local IPC socket of the running Agent.
    #[arg(long, env = "RCLI_AGENT_SOCKET")]
    socket: Option<PathBuf>,
    /// Machine-readable JSON output.
    #[arg(long)]
    json: bool,
}

impl Prompts {
    pub async fn run(self, out: &mut dyn std::io::Write) -> anyhow::Result<()> {
        let reply = send(&self.socket.unwrap_or_else(ipc::default_socket), &ipc::Request::Prompts).await?;
        if self.json {
            writeln!(out, "{}", serde_json::to_string_pretty(&reply)?)?;
        } else {
            for p in reply.as_array().map(Vec::as_slice).unwrap_or_default() {
                writeln!(
                    out,
                    "{} {} ({}, {}s left)",
                    p["id"].as_u64().unwrap_or_default(),
                    p["who"].as_str().unwrap_or_default(),
                    p["kind"].as_str().unwrap_or_default(),
                    p["seconds_left"].as_u64().unwrap_or_default(),
                )?;
            }
        }
        Ok(())
    }
}

pub async fn run_prompt(command: PromptCommand, socket: Option<PathBuf>, out: &mut dyn std::io::Write) -> anyhow::Result<()> {
    let PromptCommand::Answer { id, decision } = command;
    let request = ipc::Request::PromptAnswer { id, accept: decision == "accept" };
    let reply = send(&socket.unwrap_or_else(ipc::default_socket), &request).await?;
    writeln!(out, "{}", reply["message"].as_str().unwrap_or_default())?;
    Ok(())
}

/// Runs no external program: the `--root` test seam installs files only, with no service.
fn no_service(_program: &str, _args: &[&str]) -> anyhow::Result<()> {
    Ok(())
}

/// `sudo rcli install`: install the Agent as a background service, register the Device and write the machine-wide
/// Controller configuration (ADR 0006).
#[derive(Args)]
pub struct Install {
    /// Server Agent port, host:port. Optional in User Mode: without it the Device serves only local control until a
    /// Server is set (`rcli agent server set`).
    #[arg(long, default_value = "")]
    server: String,
    /// Server Key, rk_<CA fingerprint>_<secret>. `-` reads one line from stdin, so an elevated helper never puts the
    /// key on a command line.
    #[arg(long, env = "RCLI_SERVER_KEY", hide_env_values = true, default_value = "")]
    key: String,
    /// Device name; default: this Device's host name.
    #[arg(long)]
    name: Option<String>,
    /// The account terminal commands run as; default: the installing account (`SUDO_USER`), or the console user on
    /// Windows.
    #[arg(long)]
    terminal_user: Option<String>,
    /// Always allow `--as root` (Device Setting root = always) instead of asking at the Device.
    #[arg(long)]
    allow_root: bool,
}

/// `root` is the `--root` test seam: install there instead of `/`, without root rights, a service or the real screen.
impl Install {
    /// Resolves `--key -` from stdin.
    pub fn with_key_from_stdin(mut self) -> anyhow::Result<Self> {
        if self.key == "-" {
            let mut line = String::new();
            std::io::stdin().read_line(&mut line)?;
            self.key = line.trim().to_owned();
        }
        Ok(self)
    }

    #[cfg(target_os = "macos")]
    pub async fn run(self, root: Option<&std::path::Path>, out: &mut dyn std::io::Write) -> anyhow::Result<()> {
        let name = match self.name {
            Some(name) => name,
            None => String::from_utf8(std::process::Command::new("/bin/hostname").arg("-s").output()?.stdout)?.trim().to_owned(),
        };
        anyhow::ensure!(self.server.is_empty() == self.key.is_empty(), "give both --server and --key, or neither");
        let opts = crate::macos_install::Options {
            server: self.server,
            key: self.key,
            name,
            terminal_user: crate::install::terminal_user(self.terminal_user),
            allow_root: self.allow_root,
        };
        // A normal user gets User Mode (ADR 0012); root keeps the LaunchDaemon (Service Mode).
        let Some(root) = root else {
            return match crate::upgrade::is_root() {
                true => {
                    anyhow::ensure!(!opts.server.is_empty(), "give --server and --key");
                    crate::macos_install::install(opts, out).await
                }
                false => crate::macos_install::install_user(opts, out).await,
            };
        };
        let platform = rcli_agent::platform::FakePlatform::new(opts.name.clone());
        let layout = crate::macos_install::UserLayout { home: root.into(), config_dir: root.into() };
        crate::macos_install::install_user_at(&layout, opts, &platform, None, &mut no_service, out).await
    }

    #[cfg(target_os = "linux")]
    pub async fn run(self, root: Option<&std::path::Path>, out: &mut dyn std::io::Write) -> anyhow::Result<()> {
        let name = match self.name {
            Some(name) => name,
            None => match std::fs::read_to_string("/proc/sys/kernel/hostname") {
                Ok(name) if !name.trim().is_empty() => name.trim().to_owned(),
                _ => String::from_utf8(std::process::Command::new("hostname").output()?.stdout)?.trim().to_owned(),
            },
        };
        anyhow::ensure!(self.server.is_empty() == self.key.is_empty(), "give both --server and --key, or neither");
        let opts = crate::linux_install::Options {
            server: self.server,
            key: self.key,
            name,
            terminal_user: crate::install::terminal_user(self.terminal_user),
            allow_root: self.allow_root,
        };
        // A normal user gets User Mode (ADR 0012); root keeps the root service the packages and install script use.
        let Some(root) = root else {
            return match crate::upgrade::is_root() {
                true => crate::linux_install::install(opts, out).await,
                false => crate::linux_install::install_user(opts, out).await,
            };
        };
        let platform = rcli_agent::platform::FakePlatform::new(opts.name.clone());
        let layout = crate::linux_install::UserLayout { home: root.into(), config_dir: root.into() };
        crate::linux_install::install_user_at(&layout, opts, &platform, &mut no_service, out).await
    }

    /// Windows, not elevated: User Mode for this user (ADR 0012). Elevated: copy the binary to `%ProgramFiles%\rcli`,
    /// register the Device and install the SYSTEM service.
    #[cfg(windows)]
    pub async fn run(self, root: Option<&std::path::Path>, out: &mut dyn std::io::Write) -> anyhow::Result<()> {
        if root.is_none() && !crate::upgrade::is_root() {
            anyhow::ensure!(self.server.is_empty() == self.key.is_empty(), "give both --server and --key, or neither");
            let opts = crate::windows_install::Options {
                server: self.server,
                key: self.key,
                name: self.name.unwrap_or_else(crate::windows_install::device_name),
                // User Mode runs commands only as this account.
                terminal_user: self.terminal_user.or_else(|| std::env::var("USERNAME").ok()).unwrap_or_else(terminal_user),
                allow_root: self.allow_root,
            };
            return crate::windows_install::install_user(opts, out).await;
        }
        anyhow::ensure!(!self.server.is_empty() && !self.key.is_empty(), "give --server and --key");
        let terminal_user = self.terminal_user.unwrap_or_else(terminal_user);
        let opts = crate::windows_install::Options {
            server: self.server,
            key: self.key,
            name: self.name.unwrap_or_else(crate::windows_install::device_name),
            terminal_user,
            allow_root: self.allow_root,
        };
        let Some(root) = root else { return crate::windows_install::install(opts, out).await };
        let platform = rcli_agent::platform::FakePlatform::new(opts.name.clone());
        crate::windows_install::install_at(&crate::windows_install::Layout::under(root), opts, &platform, &mut no_service, out).await
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    pub async fn run(self, _root: Option<&std::path::Path>, _out: &mut dyn std::io::Write) -> anyhow::Result<()> {
        anyhow::bail!("`rcli install` is not available on this OS yet")
    }
}

/// The logged-in interactive user: the person at the Device, not the elevated administrator account that may have
/// been used for "Run as different user". Falls back to this process's account.
#[cfg(windows)]
fn terminal_user() -> String {
    rcli_agent::windows_platform::active_user()
        .or_else(|| std::env::var("USERNAME").ok())
        .filter(|u| !u.trim().is_empty())
        .unwrap_or_else(|| "SYSTEM".into())
}

/// `sudo rcli uninstall`: remove the service, the Device's local data and the machine-wide Controller configuration.
/// Each user's remembered Device Passwords stay.
#[derive(Args)]
pub struct Uninstall {}

/// `root` is the `--root` test seam, as for `Install`.
impl Uninstall {
    #[cfg(target_os = "macos")]
    pub async fn run(self, root: Option<&std::path::Path>, out: &mut dyn std::io::Write) -> anyhow::Result<()> {
        match root {
            None if crate::upgrade::is_root() => crate::macos_install::uninstall(out).await,
            None => crate::macos_install::uninstall_user(out).await,
            Some(root) => {
                let layout = crate::macos_install::UserLayout { home: root.into(), config_dir: root.into() };
                crate::macos_install::uninstall_user_at(&layout, None, &mut no_service, out).await
            }
        }
    }

    #[cfg(target_os = "linux")]
    pub async fn run(self, root: Option<&std::path::Path>, out: &mut dyn std::io::Write) -> anyhow::Result<()> {
        match root {
            None if crate::upgrade::is_root() => crate::linux_install::uninstall(out).await,
            None => crate::linux_install::uninstall_user(out).await,
            Some(root) => {
                let layout = crate::linux_install::UserLayout { home: root.into(), config_dir: root.into() };
                crate::linux_install::uninstall_user_at(&layout, &mut no_service, out).await
            }
        }
    }

    #[cfg(windows)]
    pub async fn run(self, root: Option<&std::path::Path>, out: &mut dyn std::io::Write) -> anyhow::Result<()> {
        match root {
            None if crate::upgrade::is_root() => crate::windows_install::uninstall(out).await,
            None => crate::windows_install::uninstall_user(out).await,
            Some(root) => crate::windows_install::uninstall_at(&crate::windows_install::Layout::under(root), &mut no_service, out).await,
        }
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    pub async fn run(self, _root: Option<&std::path::Path>, _out: &mut dyn std::io::Write) -> anyhow::Result<()> {
        anyhow::bail!("`rcli uninstall` is not available on this OS yet")
    }
}

/// `rcli service install|uninstall` (ticket 15, ADR 0012): switch this Device between User Mode and Service Mode.
/// Root is asked only here, at the Device: no Controller request or Action reaches this.
#[derive(clap::Subcommand)]
pub enum ServiceCommand {
    /// Move this user's User Mode install to the root/SYSTEM service (Service Mode: lock and login screens, before
    /// login, other Terminal Users). Keeps the Device ID, Device Password and Device Settings. Asks for root.
    Install,
    /// Move the Service Mode install back to User Mode for the user running it. Keeps the Device ID, Device Password
    /// and Device Settings. Asks for root.
    Uninstall,
}

impl ServiceCommand {
    /// `root` is the `--root` test seam: the User Mode install is there (as for `Install`), the Service Mode one
    /// under `<root>/system`, and nothing runs or asks for root.
    pub async fn run(self, root: Option<&std::path::Path>, out: &mut dyn std::io::Write) -> anyhow::Result<()> {
        let to_service = matches!(self, Self::Install);
        if root.is_none() && !crate::upgrade::is_root() {
            use std::io::IsTerminal;
            return escalate(if to_service { "install" } else { "uninstall" }, std::io::stdin().is_terminal());
        }
        switch(to_service, root, out).await
    }
}

/// Without root: on a terminal, runs the same command through `sudo`, which asks for the password; otherwise says
/// what to run and changes nothing.
fn escalate(verb: &str, tty: bool) -> anyhow::Result<()> {
    #[cfg(unix)]
    if tty {
        let status = std::process::Command::new("sudo").arg(std::env::current_exe()?).args(["service", verb]).status()?;
        anyhow::ensure!(status.success(), "`sudo rcli service {verb}` failed");
        return Ok(());
    }
    let _ = tty;
    #[cfg(windows)]
    anyhow::bail!("switching the Install Mode needs an Administrator: run `rcli service {verb}` from an elevated shell (Run as administrator)");
    #[cfg(not(windows))]
    anyhow::bail!("switching the Install Mode needs root: run `sudo rcli service {verb}` in a terminal")
}

#[cfg(target_os = "linux")]
async fn switch(to_service: bool, root: Option<&std::path::Path>, out: &mut dyn std::io::Write) -> anyhow::Result<()> {
    use crate::linux_install::{to_service_at, to_user_at, Layout, UserLayout};
    let name = crate::config::hostname_or("linux");
    let Some(root) = root else {
        let owner = crate::install::Owner::from_sudo()?;
        // shortcut: the user's XDG default config dir; a custom XDG_CONFIG_HOME is not followed. Read it from the
        // user's environment when someone with a custom one reports a missing Address Book.
        let user = UserLayout { home: owner.home.clone(), config_dir: owner.home.join(".config/rcli") };
        let layout = Layout { root: "/".into() };
        let platform = rcli_agent::linux::LinuxPlatform::new(name.clone())?;
        let run = &mut crate::install::system;
        return match to_service {
            true => to_service_at(&user, Some(&owner.name), &layout, name, &platform, run, out).await,
            false => to_user_at(&layout, &user, Some(&owner), name, &platform, run, out).await,
        };
    };
    let user = UserLayout { home: root.into(), config_dir: root.into() };
    let layout = Layout { root: root.join("system") };
    let platform = rcli_agent::platform::FakePlatform::new(name.clone());
    match to_service {
        true => to_service_at(&user, None, &layout, name, &platform, &mut no_service, out).await,
        false => to_user_at(&layout, &user, None, name, &platform, &mut no_service, out).await,
    }
}

#[cfg(target_os = "macos")]
async fn switch(to_service: bool, root: Option<&std::path::Path>, out: &mut dyn std::io::Write) -> anyhow::Result<()> {
    use crate::macos_install::{to_service_at, to_user_at, Layout, UserLayout};
    let name = String::from_utf8(std::process::Command::new("/bin/hostname").arg("-s").output()?.stdout)?.trim().to_owned();
    let Some(root) = root else {
        let owner = crate::install::Owner::from_sudo()?;
        let user = UserLayout { home: owner.home.clone(), config_dir: owner.home.join("Library/Application Support").join(crate::config::MACOS_DIR) };
        let layout = Layout { root: "/".into() };
        let platform = rcli_agent::macos::MacPlatform::new(name.clone())?;
        let (run, console) = (&mut crate::install::system, crate::macos_install::console_uid());
        return match to_service {
            true => to_service_at(&user, Some(owner.uid), &layout, console, name, &platform, run, out).await,
            false => to_user_at(&layout, &user, Some(&owner), console, name, &platform, run, out).await,
        };
    };
    let user = UserLayout { home: root.into(), config_dir: root.into() };
    let layout = Layout { root: root.join("system") };
    let platform = rcli_agent::platform::FakePlatform::new(name.clone());
    match to_service {
        true => to_service_at(&user, None, &layout, None, name, &platform, &mut no_service, out).await,
        false => to_user_at(&layout, &user, None, None, name, &platform, &mut no_service, out).await,
    }
}

#[cfg(windows)]
async fn switch(to_service: bool, root: Option<&std::path::Path>, out: &mut dyn std::io::Write) -> anyhow::Result<()> {
    use crate::windows_install::{to_service_at, to_user_at, Layout, UserLayout};
    let name = crate::windows_install::device_name();
    let (user, layout) = match root {
        None => (UserLayout::real()?, Layout::real()),
        Some(root) => {
            let user = UserLayout { local_app_data: root.into(), config_dir: root.into(), user: "seam".into() };
            (user, Layout::under(&root.join("system")))
        }
    };
    let platform: Box<dyn rcli_agent::platform::Platform> = match root {
        None => Box::new(rcli_agent::windows_platform::WindowsPlatform::new(name.clone())?),
        Some(_) => Box::new(rcli_agent::platform::FakePlatform::new(name.clone())),
    };
    let run: crate::install::Run = match root {
        None => &mut crate::install::system,
        Some(_) => &mut no_service,
    };
    match to_service {
        true => to_service_at(&user, &layout, name, &*platform, run, out).await,
        false => to_user_at(&layout, &user, name, &*platform, run, out).await,
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
async fn switch(_to_service: bool, _root: Option<&std::path::Path>, _out: &mut dyn std::io::Write) -> anyhow::Result<()> {
    anyhow::bail!("`rcli service` is not available on this OS yet")
}

/// `rcli agent session` (started by the root service): does the daemon's screen work in this console session.
#[derive(Args)]
pub struct Session {
    /// macOS/Linux: the daemon's session socket.
    #[arg(long)]
    socket: Option<PathBuf>,
    /// Windows: the service's session pipe.
    #[arg(long)]
    pipe: Option<String>,
}

impl Session {
    #[cfg(target_os = "macos")]
    pub async fn run(self) -> anyhow::Result<()> {
        let _ = tracing_subscriber::fmt().try_init();
        let socket = self.socket.ok_or_else(|| anyhow::anyhow!("--socket is required on macOS"))?;
        let platform = rcli_agent::macos::MacPlatform::new("session")?;
        rcli_agent::macos::stay_out_of_the_dock();
        tokio::task::spawn_blocking(move || rcli_agent::macos_session::run_helper(&socket, &platform, &rcli_agent::macos::on_console))
            .await?
    }

    #[cfg(windows)]
    pub async fn run(self) -> anyhow::Result<()> {
        let _ = tracing_subscriber::fmt().try_init();
        let pipe = self.pipe.ok_or_else(|| anyhow::anyhow!("--pipe is required on Windows"))?;
        let platform = rcli_agent::windows_platform::WindowsPlatform::new("session")?;
        tokio::task::spawn_blocking(move || {
            rcli_agent::windows_session::run_helper(&pipe, &platform, &rcli_agent::windows_session::on_console)
        })
        .await?
    }

    #[cfg(target_os = "linux")]
    pub async fn run(self) -> anyhow::Result<()> {
        let _ = tracing_subscriber::fmt().try_init();
        let socket = self.socket.ok_or_else(|| anyhow::anyhow!("--socket is required on Linux"))?;
        tokio::task::spawn_blocking(move || rcli_agent::linux::session::run_helper(&socket)).await?
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    pub async fn run(self) -> anyhow::Result<()> {
        anyhow::bail!("`rcli agent session` is not available on this OS yet")
    }
}

/// `rcli agent service` (Windows, started by the SCM, hidden): hosts the Agent as a service.
#[derive(Args)]
pub struct Service {
    /// The protected file `rcli install` wrote (Server Address, Server Key, Device name).
    #[arg(long)]
    config: PathBuf,
}

impl Service {
    #[cfg(windows)]
    pub fn run(self) -> anyhow::Result<()> {
        crate::windows_service::run(&self.config)
    }

    #[cfg(not(windows))]
    pub fn run(self) -> anyhow::Result<()> {
        anyhow::bail!("`rcli agent service` is Windows only")
    }
}

#[cfg(test)]
mod tests {
    /// Ticket 15: without a terminal and without root, the switch says what to run and changes nothing.
    #[test]
    fn without_a_tty_the_switch_prints_the_command_to_run() {
        let err = super::escalate("install", false).unwrap_err().to_string();
        #[cfg(not(windows))]
        assert!(err.contains("`sudo rcli service install`"), "{err}");
        #[cfg(windows)]
        assert!(err.contains("`rcli service install` from an elevated shell"), "{err}");
    }
}
