//! `rcli`: a thin client over the Controller API. `run` is the whole CLI, so tests call it in-process.
//! Add a command: a `Command` variant plus a `match` arm; print human text, or the API JSON with `--json`.

use std::{
    ffi::OsString,
    io::Write,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};

pub mod agent;
pub mod config;
pub mod install;
#[cfg(feature = "gui")]
pub mod gui;
// Like `windows_install`: the layouts and flows are portable through the `Run` seam, so their tests run on any Unix.
#[cfg(any(target_os = "linux", all(unix, test)))]
mod linux_install;
#[cfg(target_os = "macos")]
mod macos_install;
pub mod media;
// The layout, service definition and install/uninstall flow are portable through the `Run` seam, so their tests
// also run off Windows; the real install is Windows-only.
#[cfg(any(windows, test))]
mod windows_install;
#[cfg(windows)]
mod windows_service;
pub mod upgrade;

use base64::Engine;
use clap::{Parser, Subcommand};
use serde_json::{json, Value};
use tokio::sync::mpsc;

pub mod mcp;

/// The Device target for the Local Device, through its running Agent (decision 15).
const LOCAL: &str = "local";

#[derive(Parser)]
#[command(name = "rcli", version, about = "rcli: remote control for AI agents and scripts")]
struct Cli {
    /// Controller API base URL; default: the API Address this machine's install keeps.
    #[arg(long, global = true)]
    server: Option<String>,
    /// User session token (tests and scripting); default: the one `rcli login` saved.
    #[arg(long, global = true, hide_env_values = true)]
    token: Option<String>,
    /// Controller config directory; default: the platform's per-user location.
    #[arg(long, global = true, hide = true)]
    config_dir: Option<String>,
    /// Test seam: machine-wide paths hang off this directory instead of `/`; `install` and `uninstall` there need no
    /// root rights and touch no service or real screen.
    #[arg(long, global = true, hide = true)]
    root: Option<PathBuf>,
    /// The running Agent's IPC socket, for the Local Device (`local`).
    #[arg(long, global = true, hide = true)]
    agent_socket: Option<PathBuf>,
    /// Machine-readable JSON output.
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Make the Local Device both a Device and a Controller: installs the Agent, registers the Device, prints its Device ID and
    /// Device Password, and sets up `rcli connect` and `rcli mcp`. A normal user gets User Mode (no root or
    /// Administrator, for this user, starts at login).
    Install(agent::Install),
    /// Undo `rcli install` (as the same user); each user's remembered Device Passwords stay.
    Uninstall(agent::Uninstall),
    /// Switch this Device between User Mode and Service Mode, keeping its Device ID and Device Password (asks for
    /// root, at the Device only).
    Service {
        #[command(subcommand)]
        command: agent::ServiceCommand,
    },
    /// Sign in with a Dashboard account and save the session (Server administration commands need it).
    Login {
        /// Dashboard user name.
        name: String,
        /// The Dashboard password; asked for when omitted.
        #[arg(long, hide_env_values = true)]
        password: Option<String>,
        /// The 6-digit TOTP code, if the User turned TOTP on; asked for when needed and omitted.
        #[arg(long)]
        totp: Option<String>,
    },
    /// Revoke and forget the saved session.
    Logout,
    /// Update this `rcli` binary to the Server's Latest Build for this OS/arch, after checking its sha256; never
    /// downgrades. As root on an installed Device it also restarts the Agent service.
    #[command(alias = "upgrade")]
    Update {
        /// Binary to replace; default: the one running now. Hidden: a seam for tests.
        #[arg(long, hide = true)]
        target: Option<PathBuf>,
    },
    /// Add a Device to your Address Book by its Device ID and Device Password, so later commands take just its ID.
    Connect {
        device: String,
        /// The Device Password; asked for when omitted.
        #[arg(long, hide_env_values = true)]
        password: Option<String>,
        /// A name to tell the Device apart.
        #[arg(long, default_value = "")]
        name: String,
        /// A tag; repeat for several.
        #[arg(long = "tag")]
        tags: Vec<String>,
        /// Do not remember the Device Password: Device commands ask for it on the terminal.
        #[arg(long)]
        no_remember: bool,
    },
    /// Remove a Device from your Address Book, with its remembered Device Password.
    Forget { device: String },
    /// List your Address Book with each Device's Device Status.
    Devices,
    /// One Device: inventory, current resources, processes.
    #[command(subcommand)]
    Device(DeviceCommand),
    /// Read or replace the Server Key Agents and Controllers need to connect (needs a User session).
    #[command(subcommand)]
    ServerKey(ServerKeyCommand),
    /// Read or set the Server Address Agents use (needs a User session).
    #[command(subcommand)]
    ServerAddress(ServerAddressCommand),
    /// Read or correct the API Address installs get, the Dashboard's public https:// URL (needs a User session).
    #[command(subcommand)]
    ApiAddress(ApiAddressCommand),
    /// Manage users (needs a User session).
    #[command(subcommand)]
    User(UserCommand),
    /// Install packages for the Connect guide's downloads (needs a User session).
    #[command(subcommand)]
    Package(PackageCommand),
    /// List, upload and delete Agent Builds; the highest version per target is the Latest Build.
    #[command(subcommand)]
    Build(BuildCommand),
    /// Control Lock: needed to send input Actions to a Device.
    #[command(subcommand)]
    Lock(LockCommand),
    /// List a Device's displays with resolution and arrangement.
    Displays { device: String },
    /// Wake an Offline Device through a Wake Relay on its LAN.
    Wake { device: String },
    /// Restart, shut down, sleep or lock the screen of a Device (all but lock respect the Control Lock).
    Power {
        device: String,
        #[arg(value_parser = ["restart", "shutdown", "sleep", "lock"])]
        action: String,
    },
    /// Observe a Device's screen (no Control Lock needed).
    Shot {
        device: String,
        /// Display index or `all`; default: the display last acted on.
        #[arg(long)]
        display: Option<String>,
        /// Number the detected Marks on the images and print the Mark table.
        #[arg(long)]
        marks: bool,
        #[command(flatten)]
        until: Until,
        /// Directory to save images in.
        #[arg(long, default_value = ".")]
        out: PathBuf,
    },
    /// Run a one-shot command on a Device: prints its stdout/stderr and exits with its exit code.
    Exec {
        device: String,
        /// `root`: run as root/SYSTEM (the Device's root setting decides). Default: the Device's Terminal User.
        #[arg(long = "as")]
        account: Option<String>,
        /// Seconds before the command is killed (default 60).
        #[arg(long)]
        timeout: Option<u32>,
        /// The command, after `--`.
        #[arg(last = true, required = true)]
        command: Vec<String>,
    },
    /// Make `--as root` the default for exec and shell on one Device: `rcli root on <Device ID>` (or `off`).
    Root {
        #[arg(value_parser = ["on", "off"])]
        state: String,
        device: String,
    },
    /// Interactive shell on a Device, like SSH. Exits with the remote shell's exit code.
    Shell {
        device: String,
        /// `root`: run as root/SYSTEM (the Device's root setting decides). Default: the Device's Terminal User.
        #[arg(long = "as")]
        account: Option<String>,
    },
    /// Send one Action (needs the Control Lock) and get an Observation back.
    /// Coordinates are in the Observation image space of `--display`.
    Act {
        device: String,
        #[command(subcommand)]
        action: ActCommand,
        /// Display the coordinates refer to.
        #[arg(long, global = true, default_value_t = 0)]
        display: u32,
        /// Observation mode: shot, frames, none or clip (saved as `clip.mp4`).
        #[arg(long, global = true, default_value = "shot")]
        observe: String,
        /// Display to observe, index or `all`; default: the display acted on.
        #[arg(long, global = true)]
        observe_display: Option<String>,
        /// Number the detected Marks on the Observation and print the Mark table.
        #[arg(long, global = true)]
        marks: bool,
        /// Directory to save images in.
        #[arg(long, global = true, default_value = ".")]
        out: PathBuf,
    },
    /// Serve the Controller operations as MCP tools on stdio, for AI agents (Claude Code etc.).
    Mcp,
    /// Run the Agent on this Device: one binary for Controller and Agent.
    #[command(subcommand)]
    Agent(AgentCommand),
    /// Send an Action Batch from JSON (needs the Control Lock): a list of Actions as in the API
    /// (plus `{"type":"wait","ms":N}`), or a whole request body, which is sent as-is.
    Batch {
        device: String,
        /// JSON file; `-` reads stdin.
        file: PathBuf,
        /// Observation mode: shot, frames or none.
        #[arg(long, default_value = "shot")]
        observe: String,
        /// Display to observe, index or `all`; default: the display last acted on.
        #[arg(long)]
        display: Option<String>,
        /// Directory to save images in.
        #[arg(long, default_value = ".")]
        out: PathBuf,
    },
}

#[derive(Subcommand)]
enum AgentCommand {
    /// Register on first start, then keep the Agent connected to the Server.
    Run(agent::Run),
    /// The console-session helper the service starts (macOS LaunchAgent, Linux systemd service, Windows service).
    #[command(hide = true)]
    Session(agent::Session),
    /// Windows: the Agent hosted as a service, started by the Service Manager.
    #[command(hide = true)]
    Service(agent::Service),
    /// Show the Device ID and connection status (any local user).
    Info(agent::Info),
    /// The Device Password: show or change it (root only).
    Password {
        #[command(subcommand)]
        command: agent::PasswordCommand,
        /// Local IPC socket of the running Agent.
        #[arg(long, env = "RCLI_AGENT_SOCKET", global = true)]
        socket: Option<PathBuf>,
    },
    /// The Device ID: set a free 9-digit one (root only).
    Id {
        #[command(subcommand)]
        command: agent::IdCommand,
        /// Local IPC socket of the running Agent.
        #[arg(long, env = "RCLI_AGENT_SOCKET", global = true)]
        socket: Option<PathBuf>,
    },
    /// The Server the Agent connects to: point it elsewhere (root only).
    Server {
        #[command(subcommand)]
        command: agent::ServerCommand,
        /// Local IPC socket of the running Agent.
        #[arg(long, env = "RCLI_AGENT_SOCKET", global = true)]
        socket: Option<PathBuf>,
    },
    /// The Terminal User: the account terminal commands and shells run as (root only).
    TerminalUser {
        #[command(subcommand)]
        command: agent::TerminalUserCommand,
        /// Local IPC socket of the running Agent.
        #[arg(long, env = "RCLI_AGENT_SOCKET", global = true)]
        socket: Option<PathBuf>,
    },
    /// Whether `--as root` is never allowed, asked at the Device, or always allowed (root only).
    Root(agent::Root),
    /// Whether a person at the Device must accept each new connection (root only).
    Accept(agent::Accept),
    /// Switch what Controllers may do (mouse and keyboard, terminal, power), on the Device or for one connection.
    Capability(agent::Capability),
    /// Auto-update and the Session Window, on or off (the Device Owner only).
    General(agent::General),
    /// End a Controller's sessions and Control Lock (any local user).
    Disconnect(agent::Disconnect),
    /// Force-release the Control Lock, whoever holds it (the Device Owner only).
    ReleaseLock(agent::ReleaseLock),
    /// The prompts waiting for the person at the Device (any local user).
    Prompts(agent::Prompts),
    /// Answer one of those prompts: refusing is open to anyone, accepting needs root.
    Prompt(agent::Prompt),
}

#[derive(Subcommand)]
enum LockCommand {
    /// Take the Control Lock. The CLI keeps no `lock/watch` open, so the lock ends only after 10 minutes without
    /// an Action (or on release); the 60 s disconnect grace applies to watchers (`rcli mcp`, Dashboard).
    Acquire { device: String },
    Release { device: String },
    Status { device: String },
    /// End someone else's lock (needs a User session).
    ForceRelease { device: String },
}

#[derive(Subcommand)]
enum ActCommand {
    Move(At),
    Click(At),
    DoubleClick(At),
    RightClick(At),
    /// Drag with the left button from (x, y) to (to_x, to_y).
    Drag { x: u32, y: u32, to_x: u32, to_y: u32 },
    /// Scroll at (x, y); positive --dy scrolls down.
    Scroll {
        x: u32,
        y: u32,
        #[arg(long, default_value_t = 0, allow_negative_numbers = true)]
        dx: i32,
        #[arg(long, default_value_t = 0, allow_negative_numbers = true)]
        dy: i32,
    },
    /// Type text.
    Type {
        text: String,
        /// It is a password.
        #[arg(long)]
        secret: bool,
    },
    /// Set a field's value through the accessibility tree, without keystrokes. Needs a Mark
    /// target from the accessibility tree.
    SetValue {
        value: String,
        /// Mark number from an Observation taken with --marks.
        #[arg(long, requires = "observation", required_unless_present = "text")]
        mark: Option<u32>,
        /// The Observation the Mark number comes from.
        #[arg(long)]
        observation: Option<u64>,
        /// Visible text of the target Mark.
        #[arg(long, conflicts_with = "mark")]
        text: Option<String>,
    },
    /// Press a key combination, e.g. ctrl+shift+t.
    Key { keys: String },
    /// Wait this many milliseconds, then (with --until-*) until a Wait Condition holds.
    Wait {
        #[arg(long, default_value_t = 0)]
        ms: u32,
        #[command(flatten)]
        until: Until,
    },
}

/// A Wait Condition from flags: `--until-appears|--until-disappears|--until-value|--until-state T`, optionally
/// `--role`, `--timeout-ms`.
#[derive(clap::Args)]
struct Until {
    /// Wait Condition: until this text is on screen (fails with wait_timeout otherwise).
    #[arg(long, group = "until")]
    until_appears: Option<String>,
    /// Wait Condition: until this text is gone from the screen.
    #[arg(long, group = "until")]
    until_disappears: Option<String>,
    /// Wait Condition: until the value of the Mark with this text matches --equals or --contains.
    #[arg(long, group = "until")]
    until_value: Option<String>,
    /// Wait Condition: until the Mark with this text matches --has or --lacks.
    #[arg(long, group = "until")]
    until_state: Option<String>,
    /// With --until-value: the whole value.
    #[arg(long, requires = "until_value", conflicts_with = "contains")]
    equals: Option<String>,
    /// With --until-value: part of the value.
    #[arg(long, requires = "until_value")]
    contains: Option<String>,
    /// With --until-state: a Mark State the Mark must have (e.g. checked).
    #[arg(long, requires = "until_state", conflicts_with = "lacks")]
    has: Option<String>,
    /// With --until-state: a Mark State the Mark must lack (e.g. disabled).
    #[arg(long, requires = "until_state")]
    lacks: Option<String>,
    /// Wait Condition: only Marks with this role count.
    #[arg(long)]
    role: Option<String>,
    /// Wait Condition: give up after this many milliseconds (default 5000, at most 30000).
    #[arg(long)]
    timeout_ms: Option<u32>,
}

impl Until {
    /// The API `until` object, or None without --until-*.
    fn json(self) -> Option<Value> {
        let role = self.role;
        let (condition, target) = match (self.until_appears, self.until_disappears, self.until_value, self.until_state) {
            (Some(text), ..) => ("appears", json!({ "text": text, "role": role })),
            (_, Some(text), ..) => ("disappears", json!({ "text": text, "role": role })),
            (_, _, Some(text), _) => ("value", json!({ "text": text, "role": role, "equals": self.equals, "contains": self.contains })),
            (.., Some(text)) => ("state", json!({ "text": text, "role": role, "has": self.has, "lacks": self.lacks })),
            _ => return None,
        };
        Some(json!({ condition: target, "timeout_ms": self.timeout_ms }))
    }
}

/// Server administration, which a password connection can never do (ADR 0004): these need a User session.
fn needs_user(command: &Command) -> bool {
    matches!(
        command,
        Command::ServerKey(_)
            | Command::ServerAddress(_)
            | Command::ApiAddress(_)
            | Command::User(_)
            | Command::Build(_)
            | Command::Lock(LockCommand::ForceRelease { .. })
    )
}

/// Where to point: image coordinates X Y, `--mark N --observation ID`, or `--text T`.
#[derive(clap::Args)]
struct At {
    #[arg(required_unless_present_any = ["mark", "text"])]
    x: Option<u32>,
    #[arg(required_unless_present_any = ["mark", "text"])]
    y: Option<u32>,
    /// Mark number from an Observation taken with --marks.
    #[arg(long, requires = "observation", conflicts_with_all = ["x", "y", "text"])]
    mark: Option<u32>,
    /// The Observation the Mark number comes from.
    #[arg(long)]
    observation: Option<u64>,
    /// Visible text of the target Mark.
    #[arg(long, conflicts_with_all = ["x", "y"])]
    text: Option<String>,
}

impl ActCommand {
    fn json(self, display: u32) -> Value {
        let point = |kind: &str, x, y| json!({ "type": kind, "display": display, "x": x, "y": y });
        let at = |kind: &str, at: At| match at {
            At { mark: Some(mark), observation, .. } => json!({ "type": kind, "mark": mark, "observation": observation }),
            At { text: Some(text), .. } => json!({ "type": kind, "display": display, "text": text }),
            At { x, y, .. } => json!({ "type": kind, "display": display, "x": x, "y": y }),
        };
        match self {
            ActCommand::Move(a) => at("move", a),
            ActCommand::Click(a) => at("click", a),
            ActCommand::DoubleClick(a) => at("double_click", a),
            ActCommand::RightClick(a) => at("right_click", a),
            ActCommand::Drag { x, y, to_x, to_y } => {
                let mut v = point("drag", x, y);
                v["to"] = json!({ "display": display, "x": to_x, "y": to_y });
                v
            }
            ActCommand::Scroll { x, y, dx, dy } => {
                let mut v = point("scroll", x, y);
                (v["dx"], v["dy"]) = (dx.into(), dy.into());
                v
            }
            ActCommand::Type { text, secret } => json!({ "type": "type", "text": text, "secret": secret }),
            ActCommand::SetValue { value, mark, observation, text } => match mark {
                Some(mark) => json!({ "type": "set_value", "mark": mark, "observation": observation, "value": value }),
                None => json!({ "type": "set_value", "display": display, "text": text, "value": value }),
            },
            ActCommand::Key { keys } => json!({ "type": "key", "keys": keys }),
            ActCommand::Wait { ms, until } => json!({ "type": "wait", "ms": ms, "until": until.json() }),
        }
    }
}

/// `--display` value: an index or "all".
fn display_sel(s: Option<String>) -> anyhow::Result<Value> {
    Ok(match s.as_deref() {
        None => Value::Null,
        Some("all") => json!("all"),
        Some(n) => json!(n.parse::<u32>().map_err(|_| anyhow::anyhow!("--display must be an index or `all`"))?),
    })
}

/// Saves each image as `<out>/display-<N>.jpg` (frames: `display-<N>-<at_ms>ms.jpg`) and a clip as
/// `<out>/clip.mp4`; one line per file.
fn save_observation(obs: &Value, out: &Path, frames: bool) -> anyhow::Result<String> {
    let mut text = String::new();
    if let Some(data) = obs["clip"]["data"].as_str() {
        std::fs::create_dir_all(out)?;
        let path = out.join("clip.mp4");
        std::fs::write(&path, base64::engine::general_purpose::STANDARD.decode(data)?)?;
        let c = &obs["clip"];
        text += &format!("clip of display {}: {}x{} {} ms {}\n", c["display"], c["width"], c["height"], c["duration_ms"], path.display());
    }
    for i in obs["images"].as_array().into_iter().flatten() {
        let display = &i["display"];
        if i["unchanged"] == true {
            text += &format!("display {display}: unchanged\n");
            continue;
        }
        let data = base64::engine::general_purpose::STANDARD.decode(str_of(&i["data"]))?;
        std::fs::create_dir_all(out)?;
        let (name, label) = match frames {
            true => (format!("display-{display}-{}ms.jpg", i["at_ms"]), format!("display {display} +{}ms", i["at_ms"])),
            false => (format!("display-{display}.jpg"), format!("display {display}")),
        };
        let path = out.join(name);
        std::fs::write(&path, data)?;
        text += &format!("{label}: {}x{} {}\n", i["width"], i["height"], path.display());
    }
    if let Some(marks) = obs["marks"].as_array() {
        text += &format!(
            "observation {}\n{:<4}  {:<7}  {:<12}  {:<19}  {:<6}  {:<17}  {:<20}  {}\n",
            obs["observation"], "MARK", "DISPLAY", "ROLE", "BOX", "PARENT", "STATES", "VALUE", "TEXT"
        );
        for m in marks {
            let bbox = format!("{},{} {}x{}", m["x"], m["y"], m["width"], m["height"]);
            let parent = m["parent"].as_u64().map(|p| p.to_string()).unwrap_or_default();
            let states = m["states"].as_array().into_iter().flatten().map(str_of).collect::<Vec<_>>().join(",");
            let value = str_of(&m["value"]);
            // The full value is in --json.
            let value = if value.chars().count() > 20 { value.chars().take(19).collect::<String>() + "…" } else { value.to_owned() };
            text += &format!(
                "{:<4}  {:<7}  {:<12}  {bbox:<19}  {parent:<6}  {states:<17}  {value:<20}  {}\n",
                m["number"].to_string(),
                m["display"].to_string(),
                str_of(&m["role"]),
                str_of(&m["text"])
            );
        }
    }
    if text.is_empty() {
        text = "ok\n".into();
    }
    Ok(text)
}

fn lock_text(lock: &Value) -> String {
    match lock["holder"].as_str() {
        Some(h) => format!("held by {h} since {}, expires {}\n", str_of(&lock["acquired_at"]), str_of(&lock["expires_at"])),
        None => "free\n".into(),
    }
}

#[derive(Subcommand)]
enum PackageCommand {
    /// Upload a .dmg, .deb or .rpm for the Connect guide's downloads; replaces the one for that kind and arch.
    Publish {
        file: String,
        /// The CPU it is for: x86_64 or aarch64.
        #[arg(long)]
        arch: String,
    },
}

#[derive(Subcommand)]
enum BuildCommand {
    /// List Agent Builds, newest version first.
    List,
    /// Upload an Agent Build; the Server computes its sha256.
    Upload {
        file: String,
        /// Semver, e.g. 0.3.0.
        #[arg(long)]
        version: String,
        /// `<os>-<arch>`, e.g. linux-x86_64.
        #[arg(long)]
        target: String,
    },
    /// Delete the build of VERSION for TARGET, or every target of VERSION when TARGET is omitted.
    Delete { version: String, target: Option<String> },
}

#[derive(Subcommand)]
enum UserCommand {
    /// Create a user.
    Create { name: String },
    /// List users.
    List,
    /// Disable a user; all of their sessions stop working.
    Disable { name: String },
    /// Set a user's Dashboard password (their TOTP setting is kept).
    SetPassword {
        name: String,
        #[arg(long, env = "RCLI_NEW_PASSWORD", hide_env_values = true)]
        password: String,
    },
}

#[derive(Subcommand)]
enum DeviceCommand {
    /// Inventory and current resources of a Device (id or name), read from its Agent now.
    Show { device: String },
    /// The Device's process list, fetched from its Agent now.
    Processes { device: String },
}

#[derive(Subcommand)]
enum ServerKeyCommand {
    /// Print the Server Key (rk_<CA fingerprint>_<secret>).
    Show,
    /// Replace the secret. Registered Devices stay Online; new installs and Controllers need the new key.
    Rotate,
}

#[derive(Subcommand)]
enum ServerAddressCommand {
    /// Print the Server Address.
    Show,
    /// Set the Server Address (host or host:port, comma-separated). Reissues the TLS certificate.
    Set { address: String },
}

#[derive(Subcommand)]
enum ApiAddressCommand {
    /// Print the API Address (empty until a User opens the Dashboard over https://).
    Show,
    /// Correct the API Address, e.g. https://rcli.example.com. Agents get it on their next connection.
    Set { address: String },
}

pub async fn run<I, T>(args: I, out: &mut dyn Write) -> anyhow::Result<()>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    run_with_input(args, None, out, Arc::new(Mutex::new(std::io::stderr()))).await
}

/// Where warnings go: stderr, or a buffer in tests.
pub type Stderr = Arc<Mutex<dyn Write + Send>>;

/// `run` with the keystrokes for `rcli shell` taken from `input` instead of the terminal (None = stdin), and
/// warnings written to `err`.
pub async fn run_with_input<I, T>(
    args: I,
    input: Option<mpsc::UnboundedReceiver<Vec<u8>>>,
    out: &mut dyn Write,
    err: Stderr,
) -> anyhow::Result<()>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let argv: Vec<OsString> = args.into_iter().map(Into::into).collect();
    // `rcli` with no arguments on a desktop opens the Agent GUI (ticket 14).
    #[cfg(feature = "gui")]
    if argv.len() == 1 {
        // Leaves `main`'s runtime first: the GUI drives its own runtime with `block_on`, which panics inside another.
        return tokio::task::block_in_place(|| crate::gui::run(std::env::var_os("RCLI_AGENT_SOCKET").map(PathBuf::from)));
    }
    // `rcli session-window <controller>`: the Agent opens one per connected Controller (ticket 21).
    #[cfg(feature = "gui")]
    if let [_, command, who] = argv.as_slice() {
        if command == crate::gui::session::COMMAND {
            let socket = std::env::var_os("RCLI_AGENT_SOCKET").map(PathBuf::from);
            return tokio::task::block_in_place(|| crate::gui::session::run(who.to_string_lossy().into_owned(), socket));
        }
    }
    let cli = match Cli::try_parse_from(argv) {
        Ok(cli) => cli,
        Err(e) if !e.use_stderr() => return Ok(write!(out, "{e}")?), // --help / --version
        Err(e) => anyhow::bail!("{e}"),
    };
    if let Command::Agent(command) = cli.command {
        return match command {
            AgentCommand::Run(run) => run.run().await,
            AgentCommand::Session(session) => session.run().await,
            AgentCommand::Service(service) => service.run(),
            AgentCommand::Info(info) => info.run(out).await,
            AgentCommand::Password { command, socket } => agent::run_password(command, socket, out).await,
            AgentCommand::Id { command, socket } => agent::run_id(command, socket, out).await,
            AgentCommand::Server { command, socket } => agent::run_server(command, socket, out).await,
            AgentCommand::TerminalUser { command, socket } => agent::run_terminal_user(command, socket, out).await,
            AgentCommand::Root(root) => root.run(out).await,
            AgentCommand::Accept(accept) => accept.run(out).await,
            AgentCommand::Capability(capability) => capability.run(out).await,
            AgentCommand::General(general) => general.run(out).await,
            AgentCommand::Disconnect(disconnect) => disconnect.run(out).await,
            AgentCommand::ReleaseLock(release) => release.run(out).await,
            AgentCommand::Prompts(prompts) => prompts.run(out).await,
            AgentCommand::Prompt(prompt) => agent::run_prompt(prompt.command, prompt.socket, out).await,
        };
    }
    let cli = match cli.command {
        Command::Install(install) => return install.with_key_from_stdin()?.run(cli.root.as_deref(), out).await,
        Command::Uninstall(uninstall) => return uninstall.run(cli.root.as_deref(), out).await,
        Command::Service { command } => return command.run(cli.root.as_deref(), out).await,
        command => Cli { command, ..cli },
    };
    let dir = cli.config_dir.clone().map_or_else(config::dir, Into::into);
    let machine = config::controller_path(&dir, cli.root.as_deref());
    // `--server` gets the https rule: passwords, the Server Key and sessions travel to it.
    if let Some(server) = &cli.server {
        config::check_transport(server.trim().trim_end_matches('/'))?;
    }
    if let Command::Login { name, password, totp } = &cli.command {
        let base = api_base(cli.server.clone(), &machine)?;
        let password = match password {
            Some(p) => p.clone(),
            None => prompt_password("Dashboard password: ")?,
        };
        let http = config::http(&machine);
        let sign_in = |totp: String| {
            http.post(format!("{}/api/v1/sessions", base.trim_end_matches('/')))
                .json(&json!({ "name": name, "password": password, "totp": totp.trim() }))
                .send()
        };
        let resp = sign_in(totp.clone().unwrap_or_default()).await?;
        let mut status = resp.status();
        let mut body: Value = resp.json().await.unwrap_or(Value::Null);
        // The Server asks for a code only when the User turned TOTP on.
        if body["error"]["code"] == "totp_required" && totp.is_none() {
            use std::io::IsTerminal;
            anyhow::ensure!(std::io::stdin().is_terminal(), "TOTP is on for {name}: pass --totp <code>");
            let resp = sign_in(prompt_line("TOTP code: ")?).await?;
            status = resp.status();
            body = resp.json().await.unwrap_or(Value::Null);
        }
        if !status.is_success() {
            let e = &body["error"];
            anyhow::bail!("{} ({}): {}", str_of(&e["code"]), status, str_of(&e["message"]));
        }
        config::set_session(&dir, Some(str_of(&body["token"]).to_owned()))?;
        let user = &body["user"];
        let text = format!("signed in as {}\n", str_of(&user["name"]));
        return match cli.json {
            true => Ok(writeln!(out, "{}", json!({ "user": user, "expires_at": body["expires_at"] }))?),
            false => Ok(write!(out, "{text}")?),
        };
    }
    if let Command::Logout = &cli.command {
        if let Some(session) = config::load_at(&dir)?.and_then(|c| c.session) {
            let base = cli.server.clone().or_else(|| config::load_machine(&machine).ok().flatten().map(|m| m.api_address));
            // Best effort: an already expired session needs no revoking.
            if let Some(base) = base.filter(|b| !b.is_empty()) {
                let _ = config::http(&machine)
                    .delete(format!("{}/api/v1/session", base.trim_end_matches('/')))
                    .bearer_auth(session)
                    .send()
                    .await;
            }
            config::set_session(&dir, None)?;
        }
        return match cli.json {
            true => Ok(writeln!(out, "{}", json!({ "signed_out": true }))?),
            false => Ok(writeln!(out, "signed out")?),
        };
    }
    if let Command::Connect { device, password, name, tags, no_remember } = &cli.command {
        let key = config::installed(&machine)?.server_key;
        let server = api_base(cli.server.clone(), &machine)?;
        let password = match password {
            Some(p) => p.clone(),
            None => prompt_password("Device Password: ")?,
        };
        let controller = config::hostname();
        let seen = config::verify(&config::http(&machine), &server, &key, device, &password, &controller).await?;
        let wake = Some(seen["wake"].clone()).filter(Value::is_object);
        config::add_entry(&dir, config::Entry { device_id: device.clone(), name: name.clone(), tags: tags.clone(), wake })?;
        match no_remember {
            true => config::forget_password(&dir, device)?,
            false => config::remember(&dir, device, &password, &controller)?,
        }
        let text = format!("connected to {device} as {controller}\n");
        return match cli.json {
            true => Ok(writeln!(out, "{}", json!({ "device_id": device, "controller": controller }))?),
            false => Ok(write!(out, "{text}")?),
        };
    }
    if let Command::Update { target } = &cli.command {
        let installed = config::installed(&machine)?;
        let path = target.clone().unwrap_or_else(|| std::env::current_exe().unwrap_or_default());
        let mut sink = std::io::sink();
        let progress: &mut dyn Write = if cli.json { &mut sink } else { &mut *out };
        let (text, body) = match upgrade::run(&installed, &path, &dir, progress).await? {
            upgrade::Outcome::Latest(latest) => {
                (format!("already the latest version ({latest})\n"), json!({ "updated": false, "version": latest }))
            }
            upgrade::Outcome::Newer(latest) => (
                format!("running {}, newer than the Server's latest ({latest})\n", upgrade::RUNNING),
                json!({ "updated": false, "version": upgrade::RUNNING, "latest": latest }),
            ),
            upgrade::Outcome::Updated(done) => (
                format!(
                    "updated {} → {} at {} for {}{}\n",
                    done.from,
                    done.version,
                    done.path.display(),
                    done.target,
                    if done.restarted { "; restarted the rcli-agent service" } else { "" }
                ),
                json!({ "updated": true, "from": done.from, "version": done.version, "target": done.target, "path": done.path, "restarted": done.restarted }),
            ),
        };
        return match cli.json {
            true => Ok(writeln!(out, "{}", serde_json::to_string_pretty(&body)?)?),
            false => Ok(write!(out, "{text}")?),
        };
    }
    if let Command::Forget { device } = &cli.command {
        config::forget(&dir, device)?;
        return match cli.json {
            true => Ok(writeln!(out, "{}", json!({ "forgotten": device }))?),
            false => Ok(writeln!(out, "forgot {device}")?),
        };
    }
    let local = match &cli.command {
        Command::Shot { device, .. } | Command::Act { device, .. } | Command::Batch { device, .. } => device == LOCAL,
        Command::Mcp => true,
        _ => false,
    };
    let mut api = Api::target(&dir, &machine, cli.server, cli.token, local, err)?;
    api.agent_socket = cli.agent_socket;
    if needs_user(&cli.command) && api.connection.token.is_none() {
        anyhow::bail!("this command needs a signed-in User: run `rcli login <name>` (or `rcli connect <Device ID>` for Device commands)");
    }
    let (body, human) = match cli.command {
        Command::Install(_)
        | Command::Uninstall(_)
        | Command::Service { .. }
        | Command::Login { .. }
        | Command::Logout
        | Command::Update { .. }
        | Command::Connect { .. }
        | Command::Forget { .. } => {
            unreachable!("handled before connecting")
        }
        Command::Devices => {
            let devices = api.address_book().await?;
            let mut text = format!(
                "{:<9}  {:<24}  {:<7}  {:<25}  {:<10}  {}\n",
                "ID", "NAME", "STATUS", "LAST SEEN", "TAGS", "AGENT"
            );
            for d in devices.as_array().into_iter().flatten() {
                let tags: Vec<&str> = d["tags"].as_array().into_iter().flatten().map(str_of).collect();
                text += &format!(
                    "{:<9}  {:<24}  {:<7}  {:<25}  {:<10}  {}\n",
                    str_of(&d["device_id"]),
                    d["name"].as_str().filter(|n| !n.is_empty()).unwrap_or("-"),
                    match d["error"].as_str() {
                        Some(e) => format!("- ({e})"),
                        None => d["status"].as_str().unwrap_or("-").to_owned(),
                    },
                    d["last_seen_at"].as_str().unwrap_or("-"),
                    if tags.is_empty() { "-".to_owned() } else { tags.join(",") },
                    match d["needs_update"].as_bool() {
                        Some(true) => format!("{} (update needed)", str_of(&d["agent_version"])),
                        _ => d["agent_version"].as_str().unwrap_or("-").to_owned(),
                    },
                );
            }
            (devices, text)
        }
        Command::Device(DeviceCommand::Show { device }) => {
            let d = api.call(reqwest::Method::GET, &format!("/api/v1/devices/{}", path_segment(&device)), None).await?;
            let mut text = String::new();
            flatten("", &d, &mut text);
            (d, text)
        }
        Command::Device(DeviceCommand::Processes { device }) => {
            let path = format!("/api/v1/devices/{}/processes", path_segment(&device));
            let procs = api.call(reqwest::Method::GET, &path, None).await?;
            let mut text = format!("{:>7}  {:<16}  {:>6}  {:>12}  {}\n", "PID", "USER", "CPU%", "MEM BYTES", "NAME");
            for p in procs.as_array().into_iter().flatten() {
                text += &format!(
                    "{:>7}  {:<16}  {:>6.1}  {:>12}  {}\n",
                    p["pid"],
                    str_of(&p["user"]),
                    p["cpu_percent"].as_f64().unwrap_or(0.0),
                    p["memory_bytes"],
                    str_of(&p["name"]),
                );
            }
            (procs, text)
        }
        Command::ServerKey(ServerKeyCommand::Show) => {
            let key = api.call(reqwest::Method::GET, "/api/v1/server-key", None).await?;
            let text = format!("{}\n", str_of(&key["server_key"]));
            (key, text)
        }
        Command::ServerKey(ServerKeyCommand::Rotate) => {
            let key = api.call(reqwest::Method::POST, "/api/v1/server-settings/server-key/rotate", None).await?;
            let text = format!("{}\n", str_of(&key["server_key"]));
            (key, text)
        }
        Command::ServerAddress(ServerAddressCommand::Show) => {
            let s = api.call(reqwest::Method::GET, "/api/v1/server-settings", None).await?;
            let address = s["server_address"].clone();
            let text = format!("{}\n", str_of(&address));
            (serde_json::json!({ "server_address": address }), text)
        }
        Command::ServerAddress(ServerAddressCommand::Set { address }) => {
            let body = serde_json::json!({ "address": address });
            let s = api.call(reqwest::Method::PUT, "/api/v1/server-settings/server-address", Some(&body)).await?;
            let text = format!("server address {}\n", str_of(&s["server_address"]));
            (s, text)
        }
        Command::ApiAddress(ApiAddressCommand::Show) => {
            let s = api.call(reqwest::Method::GET, "/api/v1/server-settings", None).await?;
            let address = s["api_address"].clone();
            let text = format!("{}\n", str_of(&address));
            (serde_json::json!({ "api_address": address }), text)
        }
        Command::ApiAddress(ApiAddressCommand::Set { address }) => {
            let body = serde_json::json!({ "address": address });
            let s = api.call(reqwest::Method::PUT, "/api/v1/server-settings/api-address", Some(&body)).await?;
            let text = format!("api address {}\n", str_of(&s["api_address"]));
            (s, text)
        }
        Command::User(UserCommand::Create { name }) => {
            let body = serde_json::json!({ "name": name });
            let user = api.call(reqwest::Method::POST, "/api/v1/users", Some(&body)).await?;
            let text = format!("created {}\n", str_of(&user["name"]));
            (user, text)
        }
        Command::User(UserCommand::List) => {
            let users = api.call(reqwest::Method::GET, "/api/v1/users", None).await?;
            let mut text = format!("{:<24}  {}\n", "NAME", "STATE");
            for u in users.as_array().into_iter().flatten() {
                let state = if u["disabled_at"].is_null() { "enabled" } else { "disabled" };
                text += &format!("{:<24}  {}\n", str_of(&u["name"]), state);
            }
            (users, text)
        }
        Command::User(UserCommand::Disable { name }) => {
            let user = api.call(reqwest::Method::POST, &format!("/api/v1/users/{name}/disable"), None).await?;
            (user, format!("disabled {name}\n"))
        }
        Command::User(UserCommand::SetPassword { name, password }) => {
            let body = serde_json::json!({ "password": password });
            let set = api.call(reqwest::Method::POST, &format!("/api/v1/users/{name}/credentials"), Some(&body)).await?;
            (set, format!("password set for {name}\n"))
        }
        Command::Build(BuildCommand::List) => {
            let builds = api.call(reqwest::Method::GET, "/api/v1/agent-builds", None).await?;
            let mut text = format!("{:<16}  {:<16}  {:>10}  {:<12}  {}\n", "VERSION", "TARGET", "SIZE", "SHA256", "");
            for b in builds.as_array().into_iter().flatten() {
                let latest = if b["latest"] == true { "latest" } else { "" };
                let sha = str_of(&b["sha256"]).get(..12).unwrap_or_default();
                text += &format!("{:<16}  {:<16}  {:>10}  {sha:<12}  {latest}\n", str_of(&b["version"]), str_of(&b["target"]), b["size"]);
            }
            (builds, text)
        }
        Command::Build(BuildCommand::Upload { file, version, target }) => {
            let url = format!("{}/api/v1/agent-builds/{version}/{target}", api.base.trim_end_matches('/'));
            let uploaded = api.send(api.http.put(url).body(tokio::fs::File::open(&file).await?), None).await?;
            (uploaded, format!("uploaded {version} for {target}\n"))
        }
        Command::Build(BuildCommand::Delete { version, target }) => {
            let path = match &target {
                Some(t) => format!("/api/v1/agent-builds/{version}/{t}"),
                None => format!("/api/v1/agent-builds/{version}"),
            };
            api.call(reqwest::Method::DELETE, &path, None).await?;
            (Value::Null, format!("deleted {version} {}\n", target.as_deref().unwrap_or("(all targets)")))
        }
        Command::Lock(cmd) => {
            let (method, device, suffix) = match cmd {
                LockCommand::Acquire { device } => (reqwest::Method::POST, device, ""),
                LockCommand::Release { device } => (reqwest::Method::POST, device, "/release"),
                LockCommand::Status { device } => (reqwest::Method::GET, device, ""),
                LockCommand::ForceRelease { device } => (reqwest::Method::POST, device, "/force-release"),
            };
            let lock = api.call(method, &format!("/api/v1/devices/{device}/lock{suffix}"), None).await?;
            let text = lock_text(&lock);
            (lock, text)
        }
        Command::Wake { device } => {
            let w = api.wake(&device).await?;
            let text = format!("waking {device} via {} ({})\n", str_of(&w["relay"]["name"]), str_of(&w["mac"]));
            (w, text)
        }
        Command::Power { device, action } => {
            let path = format!("/api/v1/devices/{}/power", path_segment(&device));
            let p = api.call(reqwest::Method::POST, &path, Some(&json!({ "action": action }))).await?;
            let text = format!("{device}: {action} sent\n");
            (p, text)
        }
        Command::Displays { device } => {
            let displays = api.call(reqwest::Method::GET, &format!("/api/v1/devices/{device}/displays"), None).await?;
            let mut text = format!("{:<5}  {:<11}  {:<5}  {:<11}  {}\n", "INDEX", "RESOLUTION", "SCALE", "POSITION", "PRIMARY");
            for d in displays.as_array().into_iter().flatten() {
                text += &format!(
                    "{:<5}  {:<11}  {:<5}  {:<11}  {}\n",
                    d["index"],
                    format!("{}x{}", d["width"], d["height"]),
                    d["scale"],
                    format!("{},{}", d["x"], d["y"]),
                    if d["primary"] == true { "yes" } else { "" },
                );
            }
            (displays, text)
        }
        Command::Shot { device, display, marks, until, out } => {
            let body = json!({ "actions": [], "display": display_sel(display)?, "marks": marks, "until": until.json() });
            let obs = api.observe(&device, &body).await?;
            let text = if cli.json { String::new() } else { save_observation(&obs, &out, false)? };
            (obs, text)
        }
        Command::Exec { device, account, timeout, command } => {
            let account = account.or_else(|| saved_root(&api.dir, &device));
            let body = json!({ "command": command.join(" "), "as": account, "timeout_secs": timeout });
            let r = api.call(reqwest::Method::POST, &format!("/api/v1/devices/{}/exec", path_segment(&device)), Some(&body)).await?;
            if !cli.json {
                write!(out, "{}", str_of(&r["stdout"]))?;
                eprint!("{}", str_of(&r["stderr"]));
                let code = r["exit_code"].as_i64().unwrap_or(0) as i32;
                if r["timed_out"] == true || code != 0 {
                    return Err(RemoteExit { code, timed_out: r["timed_out"] == true }.into());
                }
            }
            (r, String::new())
        }
        Command::Root { state, device } => {
            let root = state == "on";
            config::set_root(&api.dir, &device, root)?;
            let text = format!("root {state} for {device}\n");
            (json!({ "device": device, "root": state }), text)
        }
        Command::Shell { device, account } => {
            let account = account.or_else(|| saved_root(&api.dir, &device));
            let code = shell(&api, &device, account, input.unwrap_or_else(stdin_input), out).await?;
            if code != 0 {
                return Err(RemoteExit { code, timed_out: false }.into());
            }
            return Ok(());
        }
        Command::Act { device, action, display, observe, observe_display, marks, out } => {
            let body = json!({
                "actions": [action.json(display)],
                "observe": observe,
                "display": display_sel(observe_display)?,
                "marks": marks,
            });
            let obs = api.observe(&device, &body).await?;
            let text = if cli.json { String::new() } else { save_observation(&obs, &out, observe == "frames")? };
            (obs, text)
        }
        Command::Batch { device, file, observe, display, out } => {
            let json = match file.to_str() {
                Some("-") => std::io::read_to_string(std::io::stdin())?,
                _ => std::fs::read_to_string(&file)?,
            };
            let body = match serde_json::from_str(&json)? {
                Value::Array(actions) => json!({ "actions": actions, "observe": observe, "display": display_sel(display)? }),
                body => body,
            };
            let frames = body["observe"] == "frames";
            let obs = api.observe(&device, &body).await?;
            let text = if cli.json { String::new() } else { save_observation(&obs, &out, frames)? };
            (obs, text)
        }
        Command::Mcp => return mcp::serve(api, tokio::io::stdin(), tokio::io::stdout()).await,
        Command::Agent(_) => unreachable!("handled before connecting"),
        Command::Package(PackageCommand::Publish { file, arch }) => {
            let kind = std::path::Path::new(&file).extension().and_then(|e| e.to_str()).unwrap_or_default().to_owned();
            let url = format!("{}/api/v1/packages/{kind}/{arch}", api.base.trim_end_matches('/'));
            let published = api.send(api.http.put(url).body(tokio::fs::File::open(&file).await?), None).await?;
            (published, format!("published the .{kind} for {arch}\n"))
        }
    };
    if cli.json {
        writeln!(out, "{}", serde_json::to_string_pretty(&body)?)?;
    } else {
        write!(out, "{human}")?;
    }
    Ok(())
}

/// Raw keystrokes from stdin, read on a plain thread (a blocked read must not hold up the runtime's exit).
fn stdin_input() -> mpsc::UnboundedReceiver<Vec<u8>> {
    let (tx, rx) = mpsc::unbounded_channel();
    std::thread::spawn(move || {
        let mut buf = [0; 4096];
        while let Ok(n @ 1..) = std::io::Read::read(&mut std::io::stdin(), &mut buf) {
            if tx.send(buf[..n].to_vec()).is_err() {
                break;
            }
        }
    });
    rx
}

/// Puts the local terminal in raw mode (keys such as Ctrl-C go to the remote shell) until dropped.
struct RawMode;

impl Drop for RawMode {
    fn drop(&mut self) {
        let _ = crossterm::terminal::disable_raw_mode();
    }
}

/// Runs an interactive shell session until the remote shell exits; returns its exit code.
/// `rcli root on <Device ID>` saved this Controller asking for root by default on that Device (`None` = off).
fn saved_root(dir: &Path, device: &str) -> Option<String> {
    config::root_default(dir, device).then(|| "root".to_owned())
}

async fn shell(
    api: &Api,
    device: &str,
    account: Option<String>,
    mut input: mpsc::UnboundedReceiver<Vec<u8>>,
    out: &mut dyn Write,
) -> anyhow::Result<i32> {
    use std::io::IsTerminal;
    let tty = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
    let size = || if tty { crossterm::terminal::size().ok() } else { None };
    let (cols, rows) = size().unwrap_or((80, 24));
    let path = format!("/api/v1/devices/{}/shell", path_segment(device));
    let url = format!("{}{path}", api.base.trim_end_matches('/'));
    let body = json!({ "as": account, "cols": cols, "rows": rows });
    let mut resp = api.check(api.auth(api.http.post(url).json(&body), Some(&path))?.send().await?).await?;
    let _raw = if tty { crossterm::terminal::enable_raw_mode().map(|()| RawMode).ok() } else { None };
    let session_url = |session: &str, what: &str| format!("{}/api/v1/shell-sessions/{session}/{what}", api.base.trim_end_matches('/'));
    let (mut session, mut buf, mut last_size, mut input_open) = (String::new(), Vec::new(), size(), true);
    // shortcut: the terminal size is polled (no SIGWINCH / console events); fine for a human resizing a window.
    let mut size_poll = tokio::time::interval(std::time::Duration::from_millis(250));
    loop {
        tokio::select! {
            chunk = resp.chunk() => {
                let Some(chunk) = chunk? else { anyhow::bail!("the Server closed the shell stream") };
                buf.extend_from_slice(&chunk);
                while let Some(i) = buf.iter().position(|b| *b == b'\n') {
                    let line: Value = serde_json::from_slice(&buf[..i])?;
                    buf.drain(..=i);
                    if let Some(data) = line["output"].as_str() {
                        out.write_all(&base64::engine::general_purpose::STANDARD.decode(data)?)?;
                        out.flush()?;
                    } else if let Some(code) = line["exit_code"].as_i64() {
                        return Ok(code as i32);
                    } else if let Some(s) = line["session"].as_str() {
                        session = s.to_owned();
                    } else if !line["error"].is_null() {
                        let e = &line["error"];
                        anyhow::bail!("{}: {}\nhint: {}", str_of(&e["code"]), str_of(&e["message"]), str_of(&e["hint"]));
                    }
                }
            }
            keys = input.recv(), if input_open && !session.is_empty() => match keys {
                Some(keys) => {
                    let path = format!("/api/v1/devices/{device}/shell");
                    api.check(api.auth(api.http.post(session_url(&session, "input")).body(keys), Some(&path))?.send().await?).await?;
                }
                None => input_open = false,
            },
            _ = size_poll.tick(), if tty && !session.is_empty() => {
                if let Some((cols, rows)) = size().filter(|s| Some(*s) != last_size) {
                    last_size = Some((cols, rows));
                    let body = json!({ "cols": cols, "rows": rows });
                    let path = format!("/api/v1/devices/{device}/shell");
                    api.check(api.auth(api.http.post(session_url(&session, "resize")).json(&body), Some(&path))?.send().await?).await?;
                }
            }
        }
    }
}

/// `rcli exec`'s remote command failed or timed out; `main` exits with `exit_code()` and prints nothing more.
#[derive(Debug)]
pub struct RemoteExit {
    pub code: i32,
    pub timed_out: bool,
}

impl RemoteExit {
    /// The remote exit code, or 124 (like `timeout(1)`) when the command timed out.
    pub fn exit_code(&self) -> i32 {
        if self.timed_out { 124 } else { self.code }
    }
}

impl std::fmt::Display for RemoteExit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.timed_out {
            true => write!(f, "command timed out"),
            false => write!(f, "command exited with code {}", self.code),
        }
    }
}

impl std::error::Error for RemoteExit {}

/// Human view of nested JSON: one `a.b[0].c: value` line per leaf, nulls and empties skipped.
fn flatten(prefix: &str, v: &Value, out: &mut String) {
    match v {
        Value::Null => {}
        Value::Object(m) => m
            .iter()
            .for_each(|(k, v)| flatten(&if prefix.is_empty() { k.clone() } else { format!("{prefix}.{k}") }, v, out)),
        Value::Array(a) => a.iter().enumerate().for_each(|(i, v)| flatten(&format!("{prefix}[{i}]"), v, out)),
        Value::String(s) if s.is_empty() => {}
        Value::String(s) => *out += &format!("{prefix}: {s}\n"),
        _ => *out += &format!("{prefix}: {v}\n"),
    }
}

fn path_segment(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn str_of(v: &Value) -> &str {
    v.as_str().unwrap_or("")
}

/// How the CLI authenticates to the Controller API: a User session when it has one, else a password connection.
#[derive(Clone, Default)]
pub struct Connection {
    /// A User session (`rcli login`, or `--token` in tests).
    pub token: Option<String>,
    /// The saved Server Key: with a remembered Device Password this makes a password connection.
    pub server_key: Option<String>,
    /// This Controller's name, shown as Control Lock holder.
    pub controller: String,
}

/// The Controller API target: `--server`/`--token`, else this machine's install (API Address, Server Key) and this
/// user's `rcli login` session.
fn api_target(dir: &Path, machine: &Path, server: Option<String>, token: Option<String>) -> anyhow::Result<(String, Connection)> {
    let controller = config::hostname();
    let session = || anyhow::Ok(config::load_at(dir)?.and_then(|c| c.session));
    let (base, token, server_key) = match (server, token) {
        (Some(base), Some(token)) => (base, Some(token), None),
        (server, token) => {
            let server_key = config::load_machine(machine)?.map(|m| m.server_key);
            (api_base(server, machine)?, token.map_or_else(session, |t| Ok(Some(t)))?, server_key)
        }
    };
    Ok((base, Connection { token, server_key, controller }))
}

/// `--server`, else this machine's installed API Address.
fn api_base(server: Option<String>, machine: &Path) -> anyhow::Result<String> {
    server.map_or_else(|| config::api_address(&config::installed(machine)?), Ok)
}

/// Asks for the Device Password without echoing it.
fn prompt_password(prompt: &str) -> anyhow::Result<String> {
    eprint!("{prompt}");
    let password = rpassword::read_password().map_err(|e| anyhow::anyhow!("cannot read the password: {e}"))?;
    Ok(password)
}

/// Asks for a line (the TOTP code).
fn prompt_line(prompt: &str) -> anyhow::Result<String> {
    eprint!("{prompt}");
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    Ok(line.trim().to_owned())
}

/// An API error response: its code and message as text, and its JSON body (a timed-out wait's carries the Observation).
#[derive(Debug)]
pub(crate) struct ApiFailure {
    message: String,
    pub(crate) body: Value,
}

impl std::fmt::Display for ApiFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ApiFailure {}

/// Controller API client (base URL + how to authenticate).
#[derive(Clone)]
pub struct Api {
    base: String,
    /// Where remembered Device Passwords are read from.
    dir: std::path::PathBuf,
    connection: Connection,
    http: reqwest::Client,
    stderr: Stderr,
    /// Shared by clones: one Api per process, so the Minimum Version warning prints at most once per process.
    warned: Arc<AtomicBool>,
    /// Device Passwords typed on the terminal for Devices whose password is not remembered.
    typed: Arc<Mutex<std::collections::HashMap<String, String>>>,
    /// Why there is no Server to call (none set up): only the Local Device can be reached.
    no_server: Option<String>,
    /// The running Agent's IPC socket, for the Local Device; default: `ipc::default_socket()`.
    agent_socket: Option<PathBuf>,
}

impl Api {
    pub fn new(base: String, token: String) -> Self {
        let connection = Connection { token: Some(token), ..Default::default() };
        Self::with(base, config::dir(), connection, Arc::new(Mutex::new(std::io::stderr())))
    }

    fn with(base: String, dir: std::path::PathBuf, connection: Connection, stderr: Stderr) -> Self {
        let (warned, typed) = Default::default();
        Self { base, dir, connection, http: rcli_agent::http_client(""), stderr, warned, typed, no_server: None, agent_socket: None }
    }

    /// `api_target`, or with `local` and no Server set up, an Api that reaches only the Local Device (decision 15).
    fn target(dir: &Path, machine: &Path, server: Option<String>, token: Option<String>, local: bool, stderr: Stderr) -> anyhow::Result<Self> {
        Ok(match api_target(dir, machine, server, token) {
            Ok((base, connection)) => Self { http: config::http(machine), ..Self::with(base, dir.to_path_buf(), connection, stderr) },
            Err(e) if local => Self {
                no_server: Some(format!("{e:#}")),
                ..Self::with(String::new(), dir.to_path_buf(), Connection { controller: config::hostname(), ..Default::default() }, stderr)
            },
            Err(e) => return Err(e),
        })
    }

    /// The Controller from this machine's install (`machine`, the machine-wide configuration), and the user's
    /// `rcli login` session and remembered Device Passwords in `dir` (what `rcli mcp` uses): no env vars and no flags.
    pub fn from_config(dir: &Path, machine: &Path) -> anyhow::Result<Self> {
        Self::target(dir, machine, None, None, true, Arc::new(Mutex::new(std::io::stderr())))
    }

    /// Reaches the Local Device through the Agent IPC socket at `socket`.
    pub fn with_agent_socket(self, socket: &Path) -> Self {
        Self { agent_socket: Some(socket.to_path_buf()), ..self }
    }

    /// `observe` on the Local Device: the running Agent runs `body` itself, no Server involved. Errors read as the
    /// Controller API's (`ApiFailure`), so a timed-out wait still carries its Observation.
    async fn observe_local(&self, body: &Value) -> anyhow::Result<Value> {
        let socket = self.agent_socket.clone().unwrap_or_else(rcli_agent::ipc::default_socket);
        let reply = rcli_agent::ipc::send(&socket, &rcli_agent::ipc::Request::Act { body: body.clone() }).await?;
        let e = &reply["error"];
        if e.is_null() {
            return Ok(reply);
        }
        let step = e["step"].as_u64().map(|s| format!(" (step {s})")).unwrap_or_default();
        let message = format!("{}: {}{step}\nhint: {}", str_of(&e["code"]), str_of(&e["message"]), str_of(&e["hint"]));
        Err(ApiFailure { message, body: reply }.into())
    }

    async fn call(&self, method: reqwest::Method, path: &str, body: Option<&Value>) -> anyhow::Result<Value> {
        let url = format!("{}{path}", self.base.trim_end_matches('/'));
        let mut req = self.http.request(method, url);
        if let Some(body) = body {
            req = req.json(body);
        }
        self.send(req, Some(path)).await
    }

    async fn send(&self, req: reqwest::RequestBuilder, path: Option<&str>) -> anyhow::Result<Value> {
        let resp = self.check(self.auth(req, path)?.send().await?).await?;
        Ok(resp.json().await.unwrap_or(Value::Null))
    }

    /// A User session when there is one, else the password connection of the Device in the request `path`.
    fn auth(&self, req: reqwest::RequestBuilder, path: Option<&str>) -> anyhow::Result<reqwest::RequestBuilder> {
        if let Some(e) = &self.no_server {
            anyhow::bail!("{e}");
        }
        if let Some(token) = &self.connection.token {
            return Ok(req.bearer_auth(token));
        }
        // The path's Device authenticates the connection when its password is saved; otherwise any remembered
        // Device does, and the Server refuses the unrelated Device itself (`forbidden_device`).
        let path_device = path
            .and_then(|p| p.strip_prefix("/api/v1/devices/"))
            .and_then(|rest| rest.split('/').next())
            .filter(|d| !d.is_empty())
            .map(str::to_owned);
        let key = self.connection.server_key.as_deref().unwrap_or_default();
        let unremembered = path_device.as_deref().filter(|d| {
            config::remembered(&self.dir, d).ok().flatten().is_none()
                && config::load_at(&self.dir).ok().flatten().is_some_and(|c| c.address_book.iter().any(|e| e.device_id == *d))
        });
        if let Some(device) = unremembered {
            let password = self.typed_password(device)?;
            return Ok(config::password_connection(req, key, device, &password, &self.connection.controller));
        }
        let device = path_device
            .filter(|d| config::remembered(&self.dir, d).ok().flatten().is_some())
            .or_else(|| config::some_remembered(&self.dir))
            .ok_or_else(|| anyhow::anyhow!("no User session saved; run `rcli login <name>` for Server administration, or `rcli connect <Device ID>`"))?;
        let Some(remembered) = config::remembered(&self.dir, &device)? else {
            anyhow::bail!("no Device Password saved for {device}; run `rcli connect {device}`");
        };
        Ok(config::password_connection(req, key, &device, &remembered.password, &remembered.controller))
    }

    /// The Device Password of an Address Book entry added with `--no-remember`: asked on the terminal once per process.
    fn typed_password(&self, device: &str) -> anyhow::Result<String> {
        use std::io::IsTerminal;
        let mut typed = self.typed.lock().unwrap();
        if let Some(password) = typed.get(device) {
            return Ok(password.clone());
        }
        anyhow::ensure!(
            std::io::stdin().is_terminal(),
            "no Device Password saved for {device} and no terminal to ask on; run `rcli connect {device}`"
        );
        let password = prompt_password(&format!("Device Password for {device}: "))?;
        typed.insert(device.to_owned(), password.clone());
        Ok(password)
    }

    /// The Address Book, each entry with what the Server says of the Device (Device Status, last seen, Agent version),
    /// asked with that Device's remembered password. A Device without one shows only its entry (asking would prompt
    /// for every Device).
    pub async fn address_book(&self) -> anyhow::Result<Value> {
        let key = self.connection.server_key.as_deref().unwrap_or_default();
        let mut rows = Vec::new();
        // shortcut: one request after another; send them together if Address Books grow to many Devices.
        for e in config::load_at(&self.dir)?.unwrap_or_default().address_book {
            let device = async {
                let Some(r) = config::remembered(&self.dir, &e.device_id)? else { return anyhow::Ok(json!({})) };
                let url = format!("{}/api/v1/devices/{}", self.base.trim_end_matches('/'), path_segment(&e.device_id));
                let req = config::password_connection(self.http.get(url), key, &e.device_id, &r.password, &r.controller);
                Ok(self.check(req.send().await?).await?.json::<Value>().await?)
            };
            let mut row = device.await.unwrap_or_else(|err| json!({ "error": format!("{err:#}") }));
            let row_map = row.as_object_mut().ok_or_else(|| anyhow::anyhow!("unexpected Device reply"))?;
            if let Some(wake) = row_map.get("wake") {
                config::set_wake(&self.dir, &e.device_id, wake)?;
            }
            for heavy in ["inventory", "resources", "wake"] {
                row_map.remove(heavy);
            }
            // The Server keeps no Device name (ADR 0011): the name is this Address Book's own.
            row_map.insert("name".into(), json!(e.name));
            row_map.insert("device_id".into(), json!(e.device_id));
            row_map.insert("tags".into(), json!(e.tags));
            row_map.entry("status").or_insert(Value::Null);
            rows.push(row);
        }
        Ok(Value::Array(rows))
    }

    /// Wakes a Device with the MAC and subnet its Address Book entry keeps (a User session's Server-side entry
    /// answers when this one has none).
    pub async fn wake(&self, device: &str) -> anyhow::Result<Value> {
        let book = config::load_at(&self.dir)?.unwrap_or_default().address_book;
        let address = book.into_iter().find(|e| e.device_id == device).and_then(|e| e.wake);
        let path = format!("/api/v1/devices/{}/wake", path_segment(device));
        self.call(reqwest::Method::POST, &path, address.as_ref()).await
    }

    /// Turns an API error response into an error naming its code and message.
    async fn check(&self, resp: reqwest::Response) -> anyhow::Result<reqwest::Response> {
        if let Some(minimum) = resp.headers().get("x-rcli-minimum-version").and_then(|v| v.to_str().ok()) {
            let below = semver::Version::parse(minimum).is_ok_and(|m| semver::Version::parse(upgrade::RUNNING).is_ok_and(|r| r < m));
            if below && !self.warned.swap(true, Ordering::Relaxed) {
                let _ = writeln!(self.stderr.lock().unwrap(), "Server recommends rcli ≥ {minimum}, run: rcli update");
            }
        }
        let status = resp.status();
        if !status.is_success() {
            let body: Value = resp.json().await.unwrap_or(Value::Null);
            let e = &body["error"];
            let step = e["step"].as_u64().map(|s| format!(" (step {s})")).unwrap_or_default();
            let hint = e["hint"].as_str().map(|h| format!("\nhint: {h}")).unwrap_or_default();
            let message = format!("{} ({}): {}{step}{hint}", str_of(&e["code"]), status, str_of(&e["message"]));
            return Err(ApiFailure { message, body }.into());
        }
        Ok(resp)
    }
}
