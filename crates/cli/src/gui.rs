//! The Agent GUI (decision 16): one main window, single instance (a second launch raises the first), with three
//! areas: This Device, Devices and Settings. It talks to the Agent over its local IPC. Built only with
//! `--features gui`; the default `rcli` binary links no display libraries. The Agent opens a Session Window per
//! connected Controller (`session`), never the main window.

pub mod session;

use std::path::PathBuf;

use eframe::egui;
use rcli_agent::{
    ipc::{self, Request},
    settings::{AcceptMode, Change, DeviceSettings, RootAccess},
};
use serde_json::{json, Value};

mod devices;

/// Opens the main window, or raises the one already open. `socket` overrides the Agent's IPC socket
/// (`RCLI_AGENT_SOCKET`).
pub fn run(socket: Option<PathBuf>) -> anyhow::Result<()> {
    let rt = tokio::runtime::Runtime::new()?;
    let listener = match rt.block_on(instance::claim()) {
        Ok(Some(listener)) => Some(listener),
        Ok(None) => return Ok(()),
        Err(e) => {
            tracing::warn!("cannot check for another rcli window: {e:#}");
            None
        }
    };
    let options = eframe::NativeOptions {
        viewport: look::viewport().with_inner_size([600.0, 760.0]).with_min_inner_size([460.0, 360.0]),
        ..Default::default()
    };
    eframe::run_native(
        "rcli",
        options,
        Box::new(move |cc| {
            look::install(&cc.egui_ctx);
            if let Some(listener) = listener {
                rt.spawn(instance::serve(listener, cc.egui_ctx.clone()));
            }
            Ok(Box::new(App::new(socket, rt)))
        }),
    )
        .map_err(|e| anyhow::anyhow!("cannot open the GUI: {e}"))?;
    Ok(())
}

/// The three areas of the main window.
#[derive(Clone, Copy, PartialEq)]
enum Area {
    ThisDevice,
    Devices,
    Settings,
}

struct App {
    /// `RCLI_AGENT_SOCKET`; else the default socket, looked up again on each call since it moves with the Install Mode.
    socket: Option<PathBuf>,
    rt: tokio::runtime::Runtime,
    area: Area,
    /// The Devices area (ticket 20).
    devices: devices::Devices,
    /// The Agent's `info` reply: `device_id`, `connected`, `server`, `controllers`, `install_mode`; `settings` and
    /// `server_key` for the Device Owner.
    info: Value,
    /// The prompts waiting for the person at the Device (tickets 07/08).
    prompts: Value,
    /// The new Device Password being typed.
    password: String,
    /// The Device Password after a reveal, for the Copy button.
    shown: Option<String>,
    /// The Server Key, after an elevated reveal (Service Mode).
    secrets: Option<Value>,
    /// The Device Settings as the Agent last reported them, once read. `None` until then, so the editors never start
    /// from made-up defaults.
    loaded: Option<DeviceSettings>,
    /// The Security editor: seeded from `loaded`, never sent as a whole (only changed fields are).
    edited: DeviceSettings,
    /// The Network editor: the new Server Address and Server Key.
    server: String,
    key: String,
    /// Whether the User Mode Agent starts at login, as the OS last said.
    start_at_login: Option<bool>,
    /// The last action's message.
    note: String,
    /// The install form (macOS and Windows, shown while no Agent answers): Server Address and Server Key.
    #[cfg(any(target_os = "macos", windows))]
    install_server: String,
    #[cfg(any(target_os = "macos", windows))]
    install_key: String,
}

impl App {
    fn new(socket: Option<PathBuf>, rt: tokio::runtime::Runtime) -> Self {
        let info = fetch(&rt, &socket.clone().unwrap_or_else(ipc::default_socket));
        Self {
            socket,
            rt,
            area: Area::ThisDevice,
            devices: Default::default(),
            info,
            prompts: json!([]),
            password: String::new(),
            shown: None,
            secrets: None,
            loaded: None,
            edited: DeviceSettings::default(),
            server: String::new(),
            key: String::new(),
            start_at_login: login::get(),
            note: String::new(),
            #[cfg(any(target_os = "macos", windows))]
            install_server: String::new(),
            #[cfg(any(target_os = "macos", windows))]
            install_key: String::new(),
        }
    }

    fn socket(&self) -> PathBuf {
        self.socket.clone().unwrap_or_else(ipc::default_socket)
    }

    /// One IPC round-trip; an `error` reply comes back as `{"error": ..., "code": ...}`. An owner change shows the OS
    /// authentication dialog to the Device Owner (ADR 0013); other requests ignore it.
    fn ipc(&self, request: &Request) -> Value {
        self.rt.block_on(ipc::send_authenticated(&self.socket(), request, &ipc::OsAuth::Dialog)).unwrap_or_else(error_reply)
    }

    /// One IPC request, run elevated when the Agent refuses only for lack of root (Service Mode).
    fn ipc_elevated(&self, request: &Request, args: &[&str]) -> Value {
        let reply = self.ipc(request);
        if reply.get("error").is_none() || !refused_for_root(&reply) {
            return reply;
        }
        match elevated(args, None) {
            // Most commands print one human line; `info --json` prints the reply as JSON.
            Ok(out) => serde_json::from_str(&out).unwrap_or_else(|_| json!({ "message": out })),
            Err(e) => json!({ "error": e.to_string() }),
        }
    }

    /// One owner change: shows its result and says whether it was applied.
    fn switch(&mut self, request: &Request, args: &[&str]) -> bool {
        let reply = self.ipc_elevated(request, args);
        self.note = note(&reply);
        reply.get("error").is_none()
    }

    fn reveal(&mut self) {
        let reply = self.ipc(&Request::PasswordShow);
        self.shown = match reply["password"].as_str() {
            Some(p) => Some(p.to_owned()),
            // Refused only for lack of root: ask the OS to run the read elevated (polkit, macOS authorization).
            None if refused_for_root(&reply) => match elevated(&["agent", "password", "show"], None) {
                Ok(p) => Some(p),
                Err(e) => {
                    self.note = format!("cannot show the Device Password: {e}");
                    None
                }
            },
            None => {
                self.note = format!("cannot show the Device Password: {}", reply["error"].as_str().unwrap_or_default());
                None
            }
        };
    }

    /// Sets `password` as the Device Password; `true` once the Agent and the Server took it.
    fn change(&mut self, password: &str) -> bool {
        let reply = self.ipc(&Request::PasswordSet { password: password.to_owned() });
        let (note, changed) = if let Some(message) = reply["message"].as_str() {
            (message.to_owned(), true)
        } else if refused_for_root(&reply) {
            match elevated(&["agent", "password", "set"], Some(password)) {
                Ok(_) => ("the Device Password was changed".to_owned(), true),
                Err(e) => (format!("cannot change the Device Password: {e}"), false),
            }
        } else {
            (format!("cannot change the Device Password: {}", reply["error"].as_str().unwrap_or_default()), false)
        };
        self.note = note;
        changed
    }

    /// Applies only the Security fields the person changed. Each field goes on its own and every failure is
    /// reported: one refused change must not hide another one.
    fn set_settings(&mut self) {
        let Some(loaded) = self.loaded.clone() else {
            self.note = "load the Device Settings first".into();
            return;
        };
        let change = Change::between(&loaded, &self.edited);
        if change.is_empty() {
            self.note = "no Device Settings changed".into();
            return;
        }
        let mut lines = Vec::new();
        let mut applied = Change::default();
        if let Some(user) = change.terminal_user.clone() {
            let request = Request::TerminalUserSet { user: user.clone() };
            match self.send_setting("Terminal User", &request, &["agent", "terminal-user", "set", &user]) {
                Ok(message) => {
                    applied.terminal_user = Some(user);
                    lines.push(message);
                }
                Err(e) => lines.push(e),
            }
        }
        if let Some(root) = change.root {
            let request = Request::RootSet { root };
            match self.send_setting("root access", &request, &["agent", "root", root.name()]) {
                Ok(message) => {
                    applied.root = Some(root);
                    lines.push(message);
                }
                Err(e) => lines.push(e),
            }
        }
        if let Some(accept) = change.accept {
            let request = Request::AcceptSet { accept };
            match self.send_setting("accept mode", &request, &["agent", "accept", accept.name()]) {
                Ok(message) => {
                    applied.accept = Some(accept);
                    lines.push(message);
                }
                Err(e) => lines.push(e),
            }
        }
        if let Some(loaded) = self.loaded.as_mut() {
            applied.apply(loaded);
        }
        self.note = lines.join("\n");
    }

    /// One changed setting, run elevated when the Agent refuses only for lack of root; `Err` names the setting.
    fn send_setting(&self, field: &str, request: &Request, args: &[&str]) -> Result<String, String> {
        let reply = self.ipc_elevated(request, args);
        match reply["message"].as_str() {
            Some(message) => Ok(message.to_owned()),
            None => Err(format!("{field}: {}", reply["error"].as_str().unwrap_or("the Agent did not report a result"))),
        }
    }

    /// A Controller capability switch in the Device Settings (`mouse-keyboard`, `terminal` or `power`).
    fn set_capability(&mut self, name: &str, on: bool) {
        let pick = |n: &str| (n == name).then_some(on);
        let request =
            Request::CapabilitiesSet { who: String::new(), mouse_keyboard: pick("mouse-keyboard"), terminal: pick("terminal"), power: pick("power") };
        if self.switch(&request, &["agent", "capability", name, on_off(on)]) {
            if let Some(c) = self.loaded.as_mut().map(|s| &mut s.capabilities) {
                *capability(c, name) = on;
            }
        }
    }

    /// A General switch kept by the Agent (`auto-update` or `session-window`).
    fn set_general(&mut self, name: &str, on: bool) {
        let pick = |n: &str| (n == name).then_some(on);
        let request = Request::GeneralSet { auto_update: pick("auto-update"), session_window: pick("session-window") };
        if self.switch(&request, &["agent", "general", name, on_off(on)]) {
            if let Some(s) = self.loaded.as_mut() {
                match name {
                    "auto-update" => s.auto_update = on,
                    _ => s.session_window = on,
                }
            }
        }
    }

    /// The `info` reply as root sees it: the ordinary IPC when this process is the Device Owner or root, else one OS
    /// authorization prompt (`rcli agent info --json`, like Reveal).
    fn root_info(&self) -> Result<Value, String> {
        let reply = self.ipc(&Request::Info);
        if reply.get("settings").is_some() || reply.get("server_key").is_some() {
            return Ok(reply);
        }
        let out = elevated(&["agent", "info", "--json"], None).map_err(|e| e.to_string())?;
        serde_json::from_str(&out).map_err(|e| format!("the elevated `rcli agent info` did not return JSON: {e}"))
    }

    /// Reads the Device Settings and fills the editors from that reply. Until this succeeds the editors are
    /// disabled: a click on Apply must never send made-up values.
    fn load_settings(&mut self) {
        match self.root_info() {
            Ok(reply) => match DeviceSettings::from_info(&reply) {
                Ok(settings) => {
                    self.edited = settings.clone();
                    self.loaded = Some(settings);
                    self.note = "the Device Settings were loaded".into();
                }
                Err(e) => self.note = format!("cannot edit the Device Settings: {e}"),
            },
            Err(e) => self.note = format!("cannot read the Device Settings: {e}"),
        }
    }

    /// The Server Key: in `info` for the Device Owner, else read elevated.
    fn reveal_secrets(&mut self) {
        match self.root_info() {
            Ok(reply) if reply.get("server_key").is_some() => self.secrets = Some(reply),
            Ok(reply) => {
                self.note = format!("cannot read the Server Key: {}", reply["error"].as_str().unwrap_or("the Agent did not include it"))
            }
            Err(e) => self.note = format!("cannot read the Server Key: {e}"),
        }
    }

    /// Settings > Network: another Server Address and Server Key; the Agent reconnects at once.
    fn set_server(&mut self) {
        let (server, key) = (self.server.trim().to_owned(), self.key.trim().to_owned());
        let request = Request::ServerSet { server: server.clone(), key: key.clone() };
        let mut reply = self.ipc(&request);
        if refused_for_root(&reply) {
            // Service Mode: the Server Key goes to the elevated `rcli` on stdin, never on its command line.
            reply = match elevated(&["agent", "server", "set", &server], Some(&key)) {
                Ok(out) => json!({ "message": out }),
                Err(e) => json!({ "error": e.to_string() }),
            };
        }
        self.note = note(&reply);
        if reply.get("error").is_none() {
            self.key.clear();
            self.secrets = None;
        }
    }

    /// Switches the Install Mode with `rcli service install|uninstall` behind the OS admin prompt (pkexec, the macOS
    /// dialog, UAC). The Device ID, Device Password and Device Settings stay.
    fn switch_install_mode(&mut self, to_service: bool) {
        let verb = if to_service { "install" } else { "uninstall" };
        self.note = match elevated(&["service", verb], None) {
            Ok(out) => out,
            #[cfg(windows)]
            Err(e) => format!("cannot switch the Install Mode: {e}\nrun `rcli service {verb}` from an elevated shell instead"),
            #[cfg(not(windows))]
            Err(e) => format!("cannot switch the Install Mode: {e}\nrun `sudo rcli service {verb}` in a terminal instead"),
        };
        self.loaded = None;
        self.secrets = None;
        self.start_at_login = login::get();
    }

    /// Installs the Agent service from this app after the OS elevation prompt (a macOS authorization dialog, a
    /// Windows UAC prompt). The person running the app becomes the Terminal User. On macOS permission grants belong
    /// to the installed app, so it is reopened; on Windows the downloaded exe may be deleted once the service runs.
    #[cfg(any(target_os = "macos", windows))]
    fn install(&mut self) {
        let user = std::env::var("USER").or_else(|_| std::env::var("USERNAME")).unwrap_or_default();
        // The Server Key goes to the elevated `rcli` on stdin, never on its command line.
        let mut args = vec!["install", "--server", self.install_server.trim(), "--key", "-"];
        if !user.is_empty() {
            args.extend(["--terminal-user", &user]);
        }
        match elevated(&args, Some(self.install_key.trim())) {
            Ok(out) => {
                self.note = out;
                #[cfg(target_os = "macos")]
                {
                    let installed = std::path::Path::new("/Applications/rcli.app");
                    if !std::env::current_exe().is_ok_and(|exe| exe.starts_with(installed)) {
                        let _ = std::process::Command::new("open").arg("-n").arg(installed).spawn();
                        std::process::exit(0);
                    }
                }
            }
            Err(e) => self.note = format!("install failed: {e}"),
        }
    }

    /// Grants root to one Controller for this session only (root).
    fn grant_root(&mut self, who: &str) {
        let reply = self.ipc_elevated(&Request::RootGrant { who: who.to_owned() }, &["agent", "root", "grant", who]);
        self.note = note(&reply);
    }

    /// The person at the Device's answer to a waiting prompt. Accepting needs root (the OS prompt);
    /// refusing is open to anyone and goes straight over the IPC.
    fn answer_prompt(&mut self, id: u64, accept: bool) {
        let request = Request::PromptAnswer { id, accept };
        let reply = if accept {
            let id = id.to_string();
            self.ipc_elevated(&request, &["agent", "prompt", "answer", &id, "accept"])
        } else {
            self.ipc(&request)
        };
        self.note = note(&reply);
    }

    /// This Device: Device ID, Device Password, Server status, Install Mode, OS permissions, who is controlling.
    fn this_device(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        // The Device ID is what a Controller types to connect: set it like a platform number.
        look::muted(ui, "Device ID");
        ui.label(egui::RichText::new(self.info["device_id"].as_str().unwrap_or("-")).font(look::bold(30.0)));

        look::section(ui, "Device Password");
        if let Some(shown) = &self.shown {
            ui.label(egui::RichText::new(shown).font(look::bold(18.0)));
        }
        ui.horizontal(|ui| {
            if ui.button("Show").clicked() {
                self.reveal();
            }
            if ui.add_enabled(self.shown.is_some(), egui::Button::new("Copy")).clicked() {
                if let Some(p) = self.shown.clone() {
                    ctx.copy_text(p);
                    self.note = "the Device Password was copied".into();
                }
            }
            if ui.button("Regenerate").clicked() {
                let fresh = rcli_protocol::password::generate();
                self.shown = self.change(&fresh).then_some(fresh);
            }
        });
        ui.horizontal(|ui| {
            look::muted(ui, "New Device Password");
            ui.text_edit_singleline(&mut self.password);
            if look::primary(ui, "Change").clicked() {
                let password = std::mem::take(&mut self.password);
                self.change(&password);
                self.shown = None;
            }
        });

        look::section(ui, "Server");
        let server = self.info["server"].as_str().unwrap_or_default();
        ui.horizontal(|ui| {
            let connected = self.info["connected"] == true;
            look::signal(ui, connected, look::INK);
            ui.label(match (server.is_empty(), connected) {
                (true, _) => "no Server set: only local control (set one in Settings > Network)".to_owned(),
                (false, true) => format!("connected to {server}"),
                (false, false) => format!("not connected to {server}"),
            });
        });

        look::section(ui, "Install Mode");
        match self.info["install_mode"].as_str() {
            Some("user") => {
                ui.label("User Mode: runs as you while you are logged in; no lock or login screens.");
                if ui.button("Switch to Service Mode").clicked() {
                    self.switch_install_mode(true);
                }
            }
            Some(_) => {
                ui.label("Service Mode: starts with the OS and reaches the lock and login screens.");
                if ui.button("Switch to User Mode").clicked() {
                    self.switch_install_mode(false);
                }
            }
            None => look::muted(ui, "unknown: the Agent is starting"),
        }
        look::small(ui, "Switching asks for an administrator; the Device ID and Device Password stay.");

        #[cfg(target_os = "macos")]
        {
            look::section(ui, "macOS permissions");
            look::muted(ui, "Live View and remote input need them.");
            // The capturing process's grants count, not this window's (macOS grants per process).
            let permissions = &self.info["capture_permissions"];
            let (screen, ax) = (permissions["screen_recording"] == true, permissions["accessibility"] == true);
            if permissions.is_null() {
                look::muted(ui, "The capturing process is not answering, so its permissions are unknown.");
            }
            let mark = |granted: bool| if granted { "allowed" } else { "not allowed yet" };
            egui::Grid::new("grants").num_columns(2).spacing([16.0, 6.0]).show(ui, |ui| {
                for (name, granted) in [("Screen Recording", screen), ("Accessibility", ax)] {
                    look::muted(ui, name);
                    ui.horizontal(|ui| {
                        look::signal(ui, granted, look::INK);
                        ui.label(mark(granted));
                    });
                    ui.end_row();
                }
            });
            if !(screen && ax) {
                ui.label("Click Allow, switch rcli on in each list System Settings opens, then click Restart capture.");
                ui.horizontal(|ui| {
                    if look::primary(ui, "Allow").clicked() {
                        rcli_agent::macos::request_grants();
                    }
                    for (label, pane) in [("Open Screen Recording", "Privacy_ScreenCapture"), ("Open Accessibility", "Privacy_Accessibility")] {
                        if ui.button(label).clicked() {
                            let url = format!("x-apple.systempreferences:com.apple.preference.security?{pane}");
                            let _ = std::process::Command::new("open").arg(url).spawn();
                        }
                    }
                    // A new grant reaches a process only when it starts again.
                    if ui.button("Restart capture").clicked() {
                        self.note = restart_session_helper();
                    }
                });
            }
        }

        look::section(ui, "Waiting for you");
        let waiting: Vec<Value> = self.prompts.as_array().cloned().unwrap_or_default();
        if waiting.is_empty() {
            look::muted(ui, "nothing is waiting");
        }
        for p in waiting {
            let id = p["id"].as_u64().unwrap_or_default();
            let who = p["who"].as_str().unwrap_or_default();
            let what = match p["kind"].as_str() {
                Some("root") => format!("{who} asks for root access"),
                _ => format!("{who} wants to connect"),
            };
            // A request waiting on the person is the one amber plate in the window.
            egui::Frame::new().fill(look::AMBER).inner_margin(egui::Margin::symmetric(12, 10)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(egui::RichText::new(what).font(look::bold(15.0)).color(look::INK));
                ui.horizontal(|ui| {
                    if look::primary(ui, "Accept").clicked() {
                        self.answer_prompt(id, true);
                    }
                    if ui.button("Reject").clicked() {
                        self.answer_prompt(id, false);
                    }
                });
            });
        }

        look::section(ui, "Being controlled by");
        let controllers = self.info["controllers"].as_array().cloned().unwrap_or_default();
        if controllers.is_empty() {
            look::muted(ui, "no Controller is connected");
        }
        for (i, controller) in controllers.iter().enumerate() {
            let who = controller.as_str().unwrap_or_default().to_owned();
            if i > 0 {
                look::rule(ui, look::RULE, 1.0);
            }
            ui.label(egui::RichText::new(&who).font(look::bold(15.0)));
            ui.horizontal(|ui| {
                if ui.button("Disconnect").clicked() {
                    let reply = self.ipc(&Request::Disconnect { who: who.clone() });
                    self.note = note(&reply);
                }
                if ui.button("Grant root to this session").clicked() {
                    self.grant_root(&who);
                }
            });
        }
        // Whoever holds the Control Lock (it may be a Controller not connected right now) loses it.
        if ui.button("Force-release the Control Lock").clicked() {
            let reply = self.ipc_elevated(&Request::LockRelease, &["agent", "release-lock"]);
            self.note = note(&reply);
        }
    }

    /// Settings: Security, Network and General.
    fn settings(&mut self, ui: &mut egui::Ui) {
        let Some(settings) = self.loaded.clone() else {
            // Service Mode: the Agent hides the Device Settings from an unelevated `info` reply, so they are read
            // through the same OS prompt as Show. The editors stay unavailable until that reply fills them.
            look::muted(ui, "Service Mode: reading the Device Settings asks for an administrator.");
            if ui.button("Load Device Settings").clicked() {
                self.load_settings();
            }
            self.general_login(ui);
            return;
        };

        look::section(ui, "Security");
        egui::Grid::new("security").num_columns(2).spacing([16.0, 8.0]).show(ui, |ui| {
            look::muted(ui, "Accept");
            ui.vertical(|ui| {
                look::choices(ui, |ui| {
                    for mode in [AcceptMode::Password, AcceptMode::PasswordAccept] {
                        look::choice(ui, &mut self.edited.accept, mode, mode.name());
                    }
                });
                look::small(ui, &format!("now: {}", settings.accept.name()));
            });
            ui.end_row();
            look::muted(ui, "Root");
            ui.vertical(|ui| {
                look::choices(ui, |ui| {
                    for level in [RootAccess::Never, RootAccess::Ask, RootAccess::Always] {
                        look::choice(ui, &mut self.edited.root, level, level.name());
                    }
                });
                look::small(ui, &format!("now: {}", settings.root.name()));
            });
            ui.end_row();
            look::muted(ui, "Terminal User");
            ui.vertical(|ui| {
                ui.text_edit_singleline(&mut self.edited.terminal_user);
                look::small(ui, &format!("now: {}", settings.terminal_user));
            });
            ui.end_row();
        });
        ui.horizontal(|ui| {
            // Only the fields the person changed are sent (see `Change::between`).
            if look::primary(ui, "Apply").clicked() {
                self.set_settings();
            }
            if ui.button("Reload").clicked() {
                self.load_settings();
            }
        });
        look::muted(ui, "What Controllers may do");
        for (name, label) in [("mouse-keyboard", "Mouse and keyboard"), ("terminal", "Terminal"), ("power", "Power")] {
            let mut on = *capability(&mut settings.capabilities.clone(), name);
            if ui.checkbox(&mut on, label).changed() {
                self.set_capability(name, on);
            }
        }

        look::section(ui, "Network");
        let key = self.secrets.as_ref().unwrap_or(&self.info)["server_key"].as_str().map(str::to_owned);
        egui::Grid::new("network").num_columns(2).spacing([16.0, 8.0]).show(ui, |ui| {
            look::muted(ui, "Server Address");
            ui.vertical(|ui| {
                ui.text_edit_singleline(&mut self.server);
                look::small(ui, &format!("now: {}", self.info["server"].as_str().filter(|s| !s.is_empty()).unwrap_or("none")));
            });
            ui.end_row();
            look::muted(ui, "Server Key");
            ui.vertical(|ui| {
                ui.add(egui::TextEdit::singleline(&mut self.key).password(true));
                match &key {
                    Some(key) => look::small(ui, &format!("now: {key}")),
                    None => {
                        if ui.button("Show the Server Key").clicked() {
                            self.reveal_secrets();
                        }
                    }
                }
            });
            ui.end_row();
        });
        if look::primary(ui, "Connect to this Server").clicked() {
            self.set_server();
        }

        look::section(ui, "General");
        self.general_login(ui);
        let mut auto_update = settings.auto_update;
        if ui.checkbox(&mut auto_update, "Install Agent updates from the Server").changed() {
            self.set_general("auto-update", auto_update);
        }
        let mut session_window = settings.session_window;
        if ui.checkbox(&mut session_window, "Show a window while a Controller is connected").changed() {
            self.set_general("session-window", session_window);
        }
    }

    /// Start at login: the User Mode Agent's login item. Service Mode starts with the OS instead.
    fn general_login(&mut self, ui: &mut egui::Ui) {
        if self.info["install_mode"] != "user" {
            look::muted(ui, "Starts with the OS (Service Mode).");
            return;
        }
        let Some(mut on) = self.start_at_login else {
            look::muted(ui, "Start at login: unknown (the OS did not say)");
            return;
        };
        if ui.checkbox(&mut on, "Start at login").changed() {
            self.note = match login::set(on) {
                Ok(()) => format!("rcli {} at login", if on { "starts" } else { "no longer starts" }),
                Err(e) => format!("cannot change start at login: {e:#}"),
            };
            self.start_at_login = login::get();
        }
    }
}

/// The message to show for an IPC or elevated reply.
fn note(reply: &Value) -> String {
    match reply["message"].as_str() {
        Some(message) => message.to_owned(),
        None => format!("error: {}", reply["error"].as_str().unwrap_or_default()),
    }
}

fn on_off(on: bool) -> &'static str {
    if on {
        "on"
    } else {
        "off"
    }
}

/// One capability switch by its CLI name.
fn capability<'a>(c: &'a mut rcli_agent::settings::Capabilities, name: &str) -> &'a mut bool {
    match name {
        "terminal" => &mut c.terminal,
        "power" => &mut c.power,
        _ => &mut c.mouse_keyboard,
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.info = fetch(&self.rt, &self.socket());
        self.prompts = self.ipc(&Request::Prompts);
        // The Device Owner (User Mode) gets the Device Settings in every `info`; Service Mode loads them elevated.
        if self.loaded.is_none() {
            if let Ok(settings) = DeviceSettings::from_info(&self.info) {
                self.edited = settings.clone();
                self.loaded = Some(settings);
            }
        }
        let unreachable = self.info["error"].is_string();
        let connected = self.info["connected"] == true;

        // The ink plate: the wordmark, and the connection as a square signal.
        egui::TopBottomPanel::top("head")
            .frame(egui::Frame::new().fill(look::INK).inner_margin(egui::Margin::symmetric(20, 14)))
            .show_separator_line(false)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("rcli").font(look::bold(20.0)).color(look::PAPER_3));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let state = if unreachable {
                            "Agent unreachable"
                        } else if connected {
                            "connected"
                        } else {
                            "disconnected"
                        };
                        ui.label(egui::RichText::new(state).color(look::PAPER_3));
                        look::signal(ui, connected && !unreachable, look::PAPER_2);
                    });
                });
            });

        egui::CentralPanel::default().frame(egui::Frame::new().fill(look::PAPER).inner_margin(egui::Margin::same(20))).show(
            ctx,
            |ui| {
                look::choices(ui, |ui| {
                    for (area, name) in [(Area::ThisDevice, "This Device"), (Area::Devices, "Devices"), (Area::Settings, "Settings")] {
                        look::choice(ui, &mut self.area, area, name);
                    }
                });
                ui.add_space(8.0);
                egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                    if !self.note.is_empty() {
                        egui::Frame::new().fill(look::PAPER_2).inner_margin(egui::Margin::symmetric(12, 8)).show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            for line in self.note.lines() {
                                ui.label(line);
                            }
                        });
                    }
                    if let Some(error) = self.info["error"].as_str().filter(|_| self.area != Area::Devices) {
                        look::section(ui, "Agent unreachable");
                        ui.label(egui::RichText::new(error).color(look::ERROR));
                        #[cfg(any(target_os = "macos", windows))]
                        {
                            look::section(ui, "Install the Agent");
                            look::muted(ui, "Install the Agent on this Device (the Dashboard's Connect page shows both values)");
                            egui::Grid::new("install").num_columns(2).spacing([16.0, 8.0]).show(ui, |ui| {
                                look::muted(ui, "Server Address");
                                ui.text_edit_singleline(&mut self.install_server);
                                ui.end_row();
                                look::muted(ui, "Server Key");
                                ui.text_edit_singleline(&mut self.install_key);
                                ui.end_row();
                            });
                            if look::primary(ui, "Install").clicked() {
                                self.install();
                            }
                        }
                        return;
                    }
                    match self.area {
                        Area::ThisDevice => self.this_device(ui, ctx),
                        Area::Devices => self.devices.show(ui, &self.rt, &mut self.note),
                        Area::Settings => self.settings(ui),
                    }
                });
            },
        );
        ctx.request_repaint_after(std::time::Duration::from_secs(2));
    }
}

/// Start at login for a User Mode Agent (Settings > General). It is the person's own login item, the way every
/// desktop app's "open at login" is, and opens nothing to anyone, so it asks for no OS password.
mod login {
    use std::process::Command;

    #[cfg(target_os = "linux")]
    pub fn get() -> Option<bool> {
        let out = Command::new("systemctl").args(["--user", "is-enabled", crate::linux_install::UNIT]).output().ok()?;
        Some(out.status.success())
    }

    #[cfg(target_os = "linux")]
    pub fn set(on: bool) -> anyhow::Result<()> {
        let verb = if on { "enable" } else { "disable" };
        let out = Command::new("systemctl").args(["--user", verb, crate::linux_install::UNIT]).output()?;
        anyhow::ensure!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr).trim());
        Ok(())
    }

    /// `launchctl disable` keeps the LaunchAgent from loading at the next login; the running Agent stays.
    #[cfg(target_os = "macos")]
    pub fn get() -> Option<bool> {
        let uid = crate::macos_install::my_uid()?;
        let out = Command::new("/bin/launchctl").args(["print-disabled", &format!("gui/{uid}")]).output().ok()?;
        let label = format!("\"{}\" =>", crate::macos_install::DAEMON);
        // `=> disabled` (or `=> true` on older macOS) marks it off; no line means on.
        let off = String::from_utf8_lossy(&out.stdout)
            .lines()
            .any(|l| l.trim().starts_with(&label) && (l.contains("disabled") || l.trim_end().ends_with("true")));
        Some(!off)
    }

    #[cfg(target_os = "macos")]
    pub fn set(on: bool) -> anyhow::Result<()> {
        let uid = crate::macos_install::my_uid().ok_or_else(|| anyhow::anyhow!("cannot read this user's uid"))?;
        let verb = if on { "enable" } else { "disable" };
        let out = Command::new("/bin/launchctl").args([verb, &crate::macos_install::user_job(uid)]).output()?;
        anyhow::ensure!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr).trim());
        Ok(())
    }

    /// Explorer's own switch for a Run key entry (what Task Manager's Startup tab writes): an odd first byte is off.
    #[cfg(windows)]
    const APPROVED: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";

    #[cfg(windows)]
    fn reg(args: &[&str]) -> std::io::Result<std::process::Output> {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        Command::new("reg.exe").args(args).creation_flags(CREATE_NO_WINDOW).output()
    }

    #[cfg(windows)]
    pub fn get() -> Option<bool> {
        let out = reg(&["query", APPROVED, "/v", crate::windows_install::RUN_VALUE]).ok()?;
        let text = String::from_utf8_lossy(&out.stdout);
        let data = text.lines().find(|l| l.contains("REG_BINARY")).and_then(|l| l.split_whitespace().last().map(str::to_owned));
        Some(!data.and_then(|d| u8::from_str_radix(d.get(..2)?, 16).ok()).is_some_and(|b| b & 1 == 1))
    }

    #[cfg(windows)]
    pub fn set(on: bool) -> anyhow::Result<()> {
        let data = format!("{}0000000000000000000000", if on { "02" } else { "03" });
        let out = reg(&["add", APPROVED, "/v", crate::windows_install::RUN_VALUE, "/t", "REG_BINARY", "/d", &data, "/f"])?;
        anyhow::ensure!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr).trim());
        Ok(())
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    pub fn get() -> Option<bool> {
        None
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    pub fn set(_on: bool) -> anyhow::Result<()> {
        anyhow::bail!("not available on this OS")
    }
}

/// Single instance (decision 16): the first window listens on a per-user socket or pipe; a later launch connects,
/// which raises the first, and exits.
mod instance {
    use eframe::egui;

    /// Brings the window forward.
    fn raise(ctx: &egui::Context) {
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        ctx.request_repaint();
    }

    #[cfg(unix)]
    fn path() -> std::path::PathBuf {
        rcli_agent::ipc::user_socket().unwrap_or_else(|| std::env::temp_dir().join("rcli/agent.sock")).with_file_name("gui.sock")
    }

    /// `Some(listener)` for the first window; `None` once the open one was raised.
    #[cfg(unix)]
    pub async fn claim() -> anyhow::Result<Option<tokio::net::UnixListener>> {
        let path = path();
        if tokio::net::UnixStream::connect(&path).await.is_ok() {
            return Ok(None);
        }
        // Nobody answers: a window that crashed left its socket behind.
        let _ = std::fs::remove_file(&path);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        Ok(Some(tokio::net::UnixListener::bind(&path)?))
    }

    /// Raises the window for each later launch, holding its connection open until this window closes.
    #[cfg(unix)]
    pub async fn serve(listener: tokio::net::UnixListener, ctx: egui::Context) {
        let mut held = Vec::new();
        while let Ok((stream, _)) = listener.accept().await {
            held.push(stream);
            raise(&ctx);
        }
    }

    #[cfg(windows)]
    fn name() -> String {
        format!(r"\\.\pipe\rcli-gui-{}", std::env::var("USERNAME").unwrap_or_default())
    }

    #[cfg(windows)]
    pub async fn claim() -> anyhow::Result<Option<tokio::net::windows::named_pipe::NamedPipeServer>> {
        use tokio::net::windows::named_pipe;
        if named_pipe::ClientOptions::new().open(name()).is_ok() {
            return Ok(None);
        }
        Ok(Some(named_pipe::ServerOptions::new().first_pipe_instance(true).create(name())?))
    }

    #[cfg(windows)]
    pub async fn serve(mut server: tokio::net::windows::named_pipe::NamedPipeServer, ctx: egui::Context) {
        let mut held = Vec::new();
        while server.connect().await.is_ok() {
            let Ok(next) = tokio::net::windows::named_pipe::ServerOptions::new().create(name()) else { return };
            held.push(std::mem::replace(&mut server, next));
            raise(&ctx);
        }
    }
}


/// Restarts this user's session helper (`launchctl kickstart -k`) so it starts with the grants just given; returns
/// the note to show.
#[cfg(target_os = "macos")]
fn restart_session_helper() -> String {
    let uid = crate::macos_install::my_uid().unwrap_or_default();
    let home = std::env::var_os("HOME").unwrap_or_default();
    // User Mode has no session helper: the Agent's own LaunchAgent captures, so that is the job to restart.
    let job = match (crate::macos_install::UserLayout { home: home.into(), config_dir: Default::default() }).plist().exists() {
        true => crate::macos_install::user_job(uid),
        false => format!("gui/{uid}/{}", crate::macos_install::SESSION),
    };
    match std::process::Command::new("/bin/launchctl").args(["kickstart", "-k", &job]).output() {
        Ok(out) if out.status.success() => "the session helper restarted; its permissions show in a moment".into(),
        Ok(out) => format!("restarting the session helper failed: {}", String::from_utf8_lossy(&out.stderr).trim()),
        Err(e) => format!("restarting the session helper failed: {e}"),
    }
}

fn fetch(rt: &tokio::runtime::Runtime, socket: &std::path::Path) -> Value {
    rt.block_on(ipc::send(socket, &Request::Info)).unwrap_or_else(|e| json!({ "error": e.to_string() }))
}

/// A failed IPC request as a reply, keeping the Agent's error code.
fn error_reply(e: anyhow::Error) -> Value {
    json!({ "error": e.to_string(), "code": e.downcast_ref::<ipc::Refused>().map(|r| r.code.as_str()) })
}

/// Whether the Agent refused only because the caller is not root (then an elevation prompt can fix it).
fn refused_for_root(reply: &Value) -> bool {
    reply["code"] == ipc::ROOT_REQUIRED
}

/// The elevated command line before its arguments: this `rcli` with SUDO_USER set to this user, which the elevation
/// prompts clear, so `rcli service` finds the account whose install it switches, as under sudo.
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn elevated_rcli() -> anyhow::Result<[String; 3]> {
    let user = format!("SUDO_USER={}", std::env::var("USER").unwrap_or_default());
    Ok(["/usr/bin/env".into(), user, std::env::current_exe()?.to_string_lossy().into_owned()])
}

/// Runs `rcli agent …` as root after the OS elevation prompt (polkit on Linux, an authorization dialog on
/// macOS) and returns its stdout. `stdin` travels over a pipe (Linux) or a 0600 temp file (macOS), never over
/// a command line, so the password is not exposed in `ps`.
#[cfg(target_os = "linux")]
fn elevated(args: &[&str], stdin: Option<&str>) -> anyhow::Result<String> {
    use std::io::Write;
    let mut cmd = std::process::Command::new("pkexec");
    cmd.args(elevated_rcli()?).args(args);
    cmd.stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
    if stdin.is_some() {
        cmd.stdin(std::process::Stdio::piped());
    }
    let mut child = cmd.spawn()?;
    if let Some(input) = stdin {
        child.stdin.take().unwrap().write_all(input.as_bytes())?;
    }
    let out = child.wait_with_output()?;
    anyhow::ensure!(out.status.success(), "{}", refusal(&out.stderr));
    Ok(String::from_utf8_lossy(&out.stdout).trim_end_matches('\n').to_owned())
}

#[cfg(target_os = "macos")]
fn elevated(args: &[&str], stdin: Option<&str>) -> anyhow::Result<String> {
    let rcli = elevated_rcli()?;
    let quoted: Vec<String> = rcli.iter().map(String::as_str).chain(args.iter().copied()).map(shell_quote).collect();
    let mut cmd = quoted.join(" ");
    let mut temp = None;
    if let Some(input) = stdin {
        use std::{io::Write as _, os::unix::fs::OpenOptionsExt as _};
        let file = std::env::temp_dir().join(format!("rcli-gui-elevated-{}", std::process::id()));
        let _ = std::fs::remove_file(&file);
        // 0600 from the start, and never another's file at that path (`create_new`): this user and root only.
        std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&file)?.write_all(input.as_bytes())?;
        let path = shell_quote(&file.to_string_lossy());
        cmd = format!("{cmd} < {path}; rm -f {path}");
        temp = Some(file);
    }
    let script = format!("do shell script \"{cmd}\" with administrator privileges");
    let out = std::process::Command::new("osascript").arg("-e").arg(script).output();
    if let Some(file) = temp {
        let _ = std::fs::remove_file(file);
    }
    let out = out?;
    anyhow::ensure!(out.status.success(), "{}", refusal(&out.stderr));
    Ok(String::from_utf8_lossy(&out.stdout).trim_end_matches('\n').to_owned())
}

/// What to show when an elevation helper failed: its stderr, or a clear line when the OS printed none (a cancelled
/// polkit or macOS authorization prompt leaves both streams empty, and an empty error reports nothing).
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn refusal(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let text = text.trim();
    if text.is_empty() {
        "the elevation prompt was refused or failed".to_owned()
    } else {
        text.to_owned()
    }
}

/// Runs `rcli agent …` elevated after the UAC prompt and returns its stdout. The elevated process cannot inherit
/// this one's output, so a hidden `cmd.exe` writes stdout/stderr to temp files; `stdin` goes through a file too, so a
/// password never appears on the elevated command line.
#[cfg(windows)]
fn elevated(args: &[&str], stdin: Option<&str>) -> anyhow::Result<String> {
    use std::os::windows::ffi::OsStrExt;
    use windows::{
        core::PCWSTR,
        Win32::{
            Foundation::CloseHandle,
            System::Threading::{GetExitCodeProcess, WaitForSingleObject, INFINITE},
            UI::{
                Shell::{ShellExecuteExW, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW},
                WindowsAndMessaging::SW_HIDE,
            },
        },
    };

    let dir = std::env::temp_dir().join(format!("rcli-gui-elevated-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
    let (out_path, err_path, status_path) = (dir.join("out"), dir.join("err"), dir.join("status"));
    let batch = dir.join("run.cmd");
    // Every argument is quoted: a Server Key may contain `&` or `^`, and paths may contain spaces. cmd does not
    // expand `%` inside... it does, so a key with `%` would break; Update key alphabets are base64url, no `%`.
    let quote = |s: &str| format!("\"{}\"", s.replace('"', "\"\""));
    let mut command = quote(&std::env::current_exe()?.to_string_lossy());
    command.push(' ');
    command.push_str(&args.iter().map(|a| quote(a)).collect::<Vec<_>>().join(" "));
    if let Some(input) = stdin {
        let input_path = dir.join("stdin");
        std::fs::write(&input_path, input)?;
        command.push_str(&format!(" < {}", quote(&input_path.to_string_lossy())));
    }
    std::fs::write(
        &batch,
        format!(
            "@echo off\r\n{command} > {} 2> {}\r\necho %ERRORLEVEL% > {}\r\n",
            quote(&out_path.to_string_lossy()),
            quote(&err_path.to_string_lossy()),
            quote(&status_path.to_string_lossy())
        ),
    )?;

    let wide = |s: &std::ffi::OsStr| s.encode_wide().chain(std::iter::once(0)).collect::<Vec<u16>>();
    let file = wide(std::ffi::OsStr::new("cmd.exe"));
    let parameters = wide(std::ffi::OsStr::new(&format!("/c \"{}\"", batch.display())));
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS,
        lpVerb: windows::core::w!("runas"),
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: PCWSTR(parameters.as_ptr()),
        nShow: SW_HIDE.0,
        ..Default::default()
    };
    let elevated = unsafe { ShellExecuteExW(&mut info) };
    if let Err(e) = elevated {
        let _ = std::fs::remove_dir_all(&dir);
        anyhow::bail!("elevation was refused or failed (UAC prompt): {e}");
    }
    anyhow::ensure!(!info.hProcess.is_invalid(), "elevation did not start a process");
    let mut process_code = 0u32;
    unsafe {
        WaitForSingleObject(info.hProcess, INFINITE);
        let _ = GetExitCodeProcess(info.hProcess, &mut process_code);
        let _ = CloseHandle(info.hProcess);
    }
    // cmd.exe's own exit code is the last command in the batch (`echo`), which always succeeds: the batch writes the
    // real one to the status file.
    let code = std::fs::read_to_string(&status_path).ok().and_then(|s| s.trim().parse::<i32>().ok()).unwrap_or(process_code as i32);
    if code != 0 {
        let stderr = std::fs::read_to_string(&err_path).unwrap_or_default();
        let stdout = std::fs::read_to_string(&out_path).unwrap_or_default();
        let _ = std::fs::remove_dir_all(&dir);
        anyhow::bail!("{}", if stderr.trim().is_empty() { stdout.trim().to_owned() } else { stderr.trim().to_owned() });
    }
    let out = std::fs::read_to_string(&out_path).unwrap_or_default();
    let _ = std::fs::remove_dir_all(&dir);
    Ok(out.trim_end_matches(['\n', '\r']).to_owned())
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn elevated(_args: &[&str], _stdin: Option<&str>) -> anyhow::Result<String> {
    anyhow::bail!("elevation is not supported here yet; run `sudo rcli agent password …`")
}

#[cfg(target_os = "macos")]
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

use std::sync::{Arc, Mutex};
use std::time::Duration;

use rcli_agent::platform::{Frame, Input, Inventory, PowerAction, Process, Pty, Resources, Account, SessionState, ShellOutput, UiAction, UiElement};

/// The Agent's platform seen from a desktop: prompts (tickets 07/08) go to the Session Window over the IPC.
pub struct WindowPlatform {
    inner: Arc<dyn rcli_agent::platform::Platform>,
    prompts: Arc<ipc::Prompts>,
}

/// Wraps the platform `rcli agent run` uses, on a desktop build, and opens a Session Window per connected Controller
/// (decision 17). Local control is no connection, so it opens none.
pub fn wrap(
    inner: Arc<dyn rcli_agent::platform::Platform>,
    prompts: Arc<ipc::Prompts>,
    settings: Arc<rcli_agent::settings::Handle>,
    controllers: Arc<rcli_agent::Controllers>,
    socket: PathBuf,
) -> Arc<dyn rcli_agent::platform::Platform> {
    let windows = SessionWindows { launch: launcher(socket, inner.clone()), open: Default::default() };
    let waiting = prompts.clone();
    // shortcut: polled each second, so a new connection's window opens up to a second late; have the Agent signal
    // new connections if that delay is noticed.
    std::thread::spawn(move || loop {
        windows.tick(&controllers.list(), &waiting.list(), settings.get().session_window);
        std::thread::sleep(Duration::from_secs(1));
    });
    Arc::new(WindowPlatform { inner, prompts })
}

/// The Session Windows the Agent opened, by Controller display name.
struct SessionWindows {
    launch: Box<dyn Fn(&str) -> Option<Window> + Send + Sync>,
    open: Mutex<std::collections::HashMap<String, Window>>,
}

impl SessionWindows {
    /// One look at who is connected: a new connection opens its window when `show` (Settings > General), a prompt
    /// that names it opens it in any case, also after the person closed it. An ended connection's window closes
    /// itself, so it is only forgotten here.
    fn tick(&self, connected: &[String], prompts: &Value, show: bool) {
        let mut open = self.open.lock().unwrap();
        open.retain(|who, _| connected.contains(who));
        let prompts = prompts.as_array().map(Vec::as_slice).unwrap_or_default();
        for who in connected {
            let asked = prompts.iter().any(|p| p["who"].as_str().is_some_and(|w| rcli_agent::names(who, w)));
            let shown = match open.get_mut(who) {
                Some(running) => !asked || running(),
                None => !asked && !show,
            };
            if shown {
                continue;
            }
            if let Some(window) = (self.launch)(who) {
                open.insert(who.clone(), window);
            }
        }
    }
}

/// Opens a Session Window in a child `rcli session-window <who>`. A root service opens it as the logged-in user,
/// never as root: as root its elevated actions would skip the elevation prompt. With nobody logged in there is
/// nobody to show it to.
fn launcher(socket: PathBuf, platform: Arc<dyn rcli_agent::platform::Platform>) -> Box<dyn Fn(&str) -> Option<Window> + Send + Sync> {
    let service_is_root = crate::upgrade::is_root();
    Box::new(move |who| {
        let exe = std::env::current_exe().unwrap_or_else(|_| "rcli".into());
        launch(&socket, &exe, &[session::COMMAND, who], service_is_root, &platform)
    })
}

/// A started GUI window process: says whether it is still running.
type Window = Box<dyn FnMut() -> bool + Send>;

/// Starts `rcli <args>` in the session that can show a window, if this process can reach one.
fn launch(
    socket: &std::path::Path,
    exe: &std::path::Path,
    args: &[&str],
    service_is_root: bool,
    platform: &Arc<dyn rcli_agent::platform::Platform>,
) -> Option<Window> {
    // A SYSTEM service in session 0 cannot show a window: it starts one as the console user (the service listens on
    // the default pipe, which the window uses too).
    #[cfg(windows)]
    if service_is_root {
        let process = rcli_agent::windows_session::spawn_as_console_user(exe, args)
            .map_err(|e| tracing::debug!("cannot open the window in the console session: {e:#}"))
            .ok()?;
        return Some(Box::new(move || process.running()));
    }
    let mut child = gui_command(socket, exe, service_is_root, platform)?.args(args).spawn().ok()?;
    Some(Box::new(move || child.try_wait().ok().flatten().is_none()))
}

/// The command that opens the GUI window in the session that can show it, if this process can reach one.
fn gui_command(
    socket: &std::path::Path,
    exe: &std::path::Path,
    service_is_root: bool,
    platform: &Arc<dyn rcli_agent::platform::Platform>,
) -> Option<std::process::Command> {
    if !service_is_root {
        let mut cmd = std::process::Command::new(exe);
        cmd.env("RCLI_AGENT_SOCKET", socket);
        // launchd kills a job's whole process group: the window outlives a User Mode Agent's "Restart capture".
        #[cfg(target_os = "macos")]
        std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
        return Some(cmd);
    }
    // The console session the screen helper serves: its user, display and X cookie or Wayland socket and session bus.
    #[cfg(target_os = "linux")]
    {
        let _ = platform;
        let target = rcli_agent::linux::session::console_target().filter(|t| !t.greeter && t.user != "root")?;
        let mut cmd = std::process::Command::new("runuser");
        cmd.args(["-u", &target.user, "--", "env"])
            .args(rcli_agent::linux::session::session_env(&target))
            .arg(format!("RCLI_AGENT_SOCKET={}", socket.display()))
            .arg(exe);
        Some(cmd)
    }
    // A launchd daemon sits outside every GUI session: `launchctl asuser` puts the window in the user's.
    #[cfg(target_os = "macos")]
    {
        let user = platform.metrics().ok().and_then(|r| r.logged_in_user).filter(|u| !u.is_empty() && u != "root")?;
        let uid = std::process::Command::new("id").args(["-u", &user]).output().ok();
        let uid = uid.map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned()).filter(|u| !u.is_empty())?;
        let mut cmd = std::process::Command::new("launchctl");
        cmd.args(["asuser", &uid, "sudo"])
            .args(["-n", "-u", &user, "--", "env"])
            .arg(format!("RCLI_AGENT_SOCKET={}", socket.display()))
            .arg(exe);
        Some(cmd)
    }
    // The SYSTEM service is handled by `launch` (`CreateProcessAsUser`, which a Command cannot express).
    #[cfg(windows)]
    {
        let _ = platform;
        None
    }
}

impl rcli_agent::platform::Platform for WindowPlatform {
    fn hostname(&self) -> String {
        self.inner.hostname()
    }
    fn displays(&self) -> anyhow::Result<Vec<rcli_agent::platform::DisplayInfo>> {
        self.inner.displays()
    }
    fn capture(&self, display: u32) -> anyhow::Result<Frame> {
        self.inner.capture(display)
    }
    fn input(&self, input: Input) -> anyhow::Result<()> {
        self.inner.input(input)
    }
    fn accessibility(&self, display: u32) -> anyhow::Result<Option<Vec<UiElement>>> {
        self.inner.accessibility(display)
    }
    fn ui_action(&self, display: u32, element: &UiElement, action: &UiAction) -> anyhow::Result<bool> {
        self.inner.ui_action(display, element, action)
    }
    fn recognize_text(&self, frame: &Frame) -> anyhow::Result<Vec<UiElement>> {
        self.inner.recognize_text(frame)
    }
    fn session_state(&self) -> anyhow::Result<SessionState> {
        self.inner.session_state()
    }
    fn inventory(&self) -> anyhow::Result<Inventory> {
        self.inner.inventory()
    }
    fn metrics(&self) -> anyhow::Result<Resources> {
        self.inner.metrics()
    }
    fn processes(&self) -> anyhow::Result<Vec<Process>> {
        self.inner.processes()
    }
    fn shell(&self, command: &str, account: Account, timeout: Duration) -> anyhow::Result<ShellOutput> {
        self.inner.shell(command, account, timeout)
    }
    fn pty(&self, account: Account, cols: u16, rows: u16) -> anyhow::Result<Pty> {
        self.inner.pty(account, cols, rows)
    }
    fn power(&self, action: PowerAction) -> anyhow::Result<()> {
        self.inner.power(action)
    }
    fn send_wake(&self, mac: [u8; 6]) -> anyhow::Result<()> {
        self.inner.send_wake(mac)
    }
    fn install_update(&self, version: &str, build: &std::path::Path) -> anyhow::Result<()> {
        self.inner.install_update(version, build)
    }
    fn set_control_indicator(&self, on: bool) -> anyhow::Result<()> {
        self.inner.set_control_indicator(on)
    }
    /// The Session Window shows the prompt and answers over the IPC; no answer in time rejects (the 30 s rule).
    fn ask_root(&self, requester: &str, timeout: Duration) -> anyhow::Result<bool> {
        Ok(self.prompts.ask(ipc::PromptKind::Root, requester, timeout))
    }
    fn ask_accept(&self, requester: &str, timeout: Duration) -> anyhow::Result<bool> {
        Ok(self.prompts.ask(ipc::PromptKind::Accept, requester, timeout))
    }
    fn user_mode(&self) -> Option<String> {
        self.inner.user_mode()
    }
    fn capture_permissions(&self) -> anyhow::Result<Option<rcli_agent::platform::CapturePermissions>> {
        self.inner.capture_permissions()
    }
    fn authenticate(&self, reason: &str, password: Option<&str>) -> anyhow::Result<bool> {
        self.inner.authenticate(reason, password)
    }
}

/// The window's look: the Dashboard's design (its tokens, flat ink panels, square status signals) mapped onto egui.
mod look {
    use eframe::egui::{
        self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Response, RichText, Stroke, TextStyle,
        Theme, Ui,
    };
    use std::sync::Arc;

    // The Dashboard's OKLCH tokens (dashboard/app/tokens.css), converted to sRGB.
    pub const PAPER: Color32 = Color32::from_rgb(237, 240, 244);
    pub const PAPER_2: Color32 = Color32::from_rgb(221, 226, 231);
    pub const PAPER_3: Color32 = Color32::from_rgb(251, 254, 255);
    pub const INK: Color32 = Color32::from_rgb(10, 13, 19);
    pub const INK_2: Color32 = Color32::from_rgb(40, 43, 50);
    pub const INK_3: Color32 = Color32::from_rgb(86, 91, 100);
    pub const RULE: Color32 = Color32::from_rgb(185, 190, 197);
    pub const SIGNAL: Color32 = Color32::from_rgb(32, 85, 225);
    pub const SIGNAL_INK: Color32 = Color32::from_rgb(252, 254, 255);
    pub const AMBER: Color32 = Color32::from_rgb(244, 181, 34);
    pub const ERROR: Color32 = Color32::from_rgb(190, 35, 35);

    const BOLD: &str = "bold";
    /// Border widths: 2px on surfaces and controls, 1px between rows.
    const BAR: f32 = 2.0;
    const HAIR: f32 = 1.0;

    /// Every rcli window: the app id matches `rcli.desktop` (Wayland and X11 find the app-menu entry and its icon by
    /// it) and the rcli icon for the title bar and taskbar. macOS takes the Dock icon from the bundle's rcli.icns (the
    /// rounded plate): an empty icon keeps eframe from replacing it with egui's.
    pub fn viewport() -> egui::ViewportBuilder {
        let icon = if cfg!(target_os = "macos") {
            egui::IconData::default()
        } else {
            eframe::icon_data::from_png_bytes(include_bytes!("../../../assets/icons/rcli-256.png")).expect("the icon is a PNG")
        };
        egui::ViewportBuilder::default().with_app_id("rcli").with_icon(icon)
    }

    pub fn bold(size: f32) -> FontId {
        FontId::new(size, FontFamily::Name(BOLD.into()))
    }

    /// Chivo Mono (OFL, `assets/fonts`), light theme only, square corners, 2px ink borders.
    pub fn install(ctx: &egui::Context) {
        let mut fonts = FontDefinitions::default();
        fonts.font_data.insert(
            "chivo".into(),
            Arc::new(FontData::from_static(include_bytes!("../assets/fonts/ChivoMono-Regular.ttf"))),
        );
        fonts.font_data.insert(
            "chivo-bold".into(),
            Arc::new(FontData::from_static(include_bytes!("../assets/fonts/ChivoMono-ExtraBold.ttf"))),
        );
        // egui's own fonts stay behind Chivo for glyphs it lacks.
        for family in [FontFamily::Proportional, FontFamily::Monospace] {
            fonts.families.entry(family).or_default().insert(0, "chivo".into());
        }
        let fallback = fonts.families[&FontFamily::Proportional].clone();
        fonts.families.insert(FontFamily::Name(BOLD.into()), [vec!["chivo-bold".to_owned()], fallback].concat());
        ctx.set_fonts(fonts);

        ctx.set_theme(Theme::Light);
        let mut style = (*ctx.style_of(Theme::Light)).clone();
        style.text_styles = [
            (TextStyle::Heading, bold(20.0)),
            (TextStyle::Body, FontId::proportional(15.0)),
            (TextStyle::Button, bold(14.0)),
            (TextStyle::Small, FontId::proportional(12.0)),
            (TextStyle::Monospace, FontId::monospace(14.0)),
        ]
        .into();
        style.spacing.button_padding = egui::vec2(12.0, 6.0);
        style.spacing.item_spacing = egui::vec2(8.0, 8.0);
        style.spacing.interact_size.y = 32.0;

        let v = &mut style.visuals;
        v.panel_fill = PAPER;
        v.window_fill = PAPER_3;
        v.window_stroke = Stroke::new(BAR, INK);
        v.window_corner_radius = CornerRadius::ZERO;
        v.menu_corner_radius = CornerRadius::ZERO;
        v.extreme_bg_color = PAPER_3;
        v.faint_bg_color = PAPER_2;
        v.code_bg_color = PAPER_2;
        v.hyperlink_color = SIGNAL;
        v.warn_fg_color = INK;
        v.error_fg_color = ERROR;
        // Text selection is amber under ink text; a focused field gets a 2px cobalt edge.
        v.selection.bg_fill = AMBER;
        v.selection.stroke = Stroke::new(BAR, SIGNAL);
        let ink = Stroke::new(BAR, INK);
        let w = &mut v.widgets;
        w.noninteractive.bg_fill = PAPER;
        w.noninteractive.weak_bg_fill = PAPER;
        w.noninteractive.bg_stroke = Stroke::new(HAIR, RULE);
        w.noninteractive.fg_stroke = Stroke::new(HAIR, INK);
        // A plain button is the outlined one; hover and press invert it to ink.
        w.inactive.weak_bg_fill = PAPER;
        w.inactive.bg_fill = PAPER_2;
        w.inactive.bg_stroke = ink;
        w.inactive.fg_stroke = Stroke::new(HAIR, INK);
        for state in [&mut w.hovered, &mut w.active, &mut w.open] {
            state.weak_bg_fill = INK;
            state.bg_fill = INK_2;
            state.bg_stroke = ink;
            state.fg_stroke = Stroke::new(HAIR, PAPER_3);
            state.expansion = 0.0;
        }
        for state in [&mut w.noninteractive, &mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
            state.corner_radius = CornerRadius::ZERO;
        }
        ctx.set_style_of(Theme::Light, style);
    }

    /// The one filled button of its group: cobalt.
    pub fn primary(ui: &mut Ui, text: &str) -> Response {
        ui.add(egui::Button::new(RichText::new(text).color(SIGNAL_INK)).fill(SIGNAL).stroke(Stroke::new(BAR, SIGNAL)))
    }

    /// One cell of a choice set; the chosen cell fills ink with paper text.
    pub fn choice<T: PartialEq>(ui: &mut Ui, current: &mut T, value: T, text: &str) {
        let chosen = *current == value;
        let button = if chosen {
            egui::Button::new(RichText::new(text).color(PAPER_3)).fill(INK)
        } else {
            egui::Button::new(text)
        };
        if ui.add(button).clicked() {
            *current = value;
        }
    }

    /// Choice cells joined edge to edge.
    pub fn choices(ui: &mut Ui, cells: impl FnOnce(&mut Ui)) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            cells(ui);
        });
    }

    /// A section head over a 2px ink rule.
    pub fn section(ui: &mut Ui, title: &str) {
        ui.add_space(20.0);
        ui.label(RichText::new(title).font(bold(17.0)));
        rule(ui, INK, BAR);
        ui.add_space(4.0);
    }

    pub fn rule(ui: &mut Ui, color: Color32, width: f32) {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), width), egui::Sense::hover());
        ui.painter().rect_filled(rect, 0.0, color);
    }

    pub fn muted(ui: &mut Ui, text: &str) {
        ui.label(RichText::new(text).color(INK_3));
    }

    pub fn small(ui: &mut Ui, text: &str) {
        ui.label(RichText::new(text).small().color(INK_3));
    }

    /// A status square: filled cobalt when on, hollow in `rim` when off.
    pub fn signal(ui: &mut Ui, on: bool, rim: Color32) {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
        if on {
            ui.painter().rect_filled(rect, 0.0, SIGNAL);
        } else {
            ui.painter().rect_stroke(rect, 0.0, Stroke::new(BAR, rim), egui::StrokeKind::Inside);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rcli_agent::platform::{FakePlatform, Platform as _};

    #[test]
    fn prompts_go_to_the_gui_queue_not_the_platform_dialog() {
        let fake = Arc::new(FakePlatform::new("pc"));
        let prompts = Arc::new(ipc::Prompts::default());
        let platform = WindowPlatform { inner: fake.clone(), prompts: prompts.clone() };
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
        assert!(platform.ask_root("alice", Duration::from_secs(5)).unwrap());
        answerer.join().unwrap();
        assert!(fake.root_prompts().is_empty(), "the platform's own prompt fired");
        // With nobody answering, the prompt auto-rejects when its time runs out.
        assert!(!platform.ask_accept("dev-laptop (10.0.0.5)", Duration::from_millis(50)).unwrap());
    }

    /// Ticket 21: one Session Window per connection, once; a prompt reopens a closed one even with them turned off.
    #[test]
    fn a_connection_opens_its_session_window_once_and_a_prompt_reopens_it() {
        let launched: Arc<Mutex<Vec<String>>> = Default::default();
        let running = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let (log, alive) = (launched.clone(), running.clone());
        let windows = SessionWindows {
            launch: Box::new(move |who| {
                log.lock().unwrap().push(who.to_owned());
                let alive = alive.clone();
                Some(Box::new(move || alive.load(std::sync::atomic::Ordering::SeqCst)))
            }),
            open: Default::default(),
        };
        let (a, b) = ("dev-laptop (10.0.0.5)".to_owned(), "alice (Dashboard)".to_owned());
        let none = json!([]);

        windows.tick(&[a.clone(), b.clone()], &none, false);
        assert!(launched.lock().unwrap().is_empty(), "Session Windows are turned off");
        windows.tick(&[a.clone(), b.clone()], &json!([{ "who": "alice", "kind": "root" }]), false);
        assert_eq!(*launched.lock().unwrap(), [b.clone()], "a prompt opens its connection's window");
        windows.tick(&[a.clone(), b.clone()], &none, true);
        windows.tick(&[a.clone(), b.clone()], &none, true);
        assert_eq!(*launched.lock().unwrap(), [b.clone(), a.clone()], "each connection opens once");

        // The person closed them: they stay closed until a prompt names that connection.
        running.store(false, std::sync::atomic::Ordering::SeqCst);
        windows.tick(&[a.clone(), b.clone()], &json!([{ "who": a, "kind": "accept" }]), true);
        assert_eq!(*launched.lock().unwrap(), [b.clone(), a.clone(), a.clone()]);
        // An ended connection is forgotten: connecting again opens a new window.
        windows.tick(&[], &none, true);
        windows.tick(&[b.clone()], &none, true);
        assert_eq!(launched.lock().unwrap().last(), Some(&b));
    }
}
