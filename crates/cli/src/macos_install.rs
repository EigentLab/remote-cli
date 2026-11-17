//! `rcli install|uninstall` on macOS (ticket 24): the app bundle in /Applications (TCC grants stick to it),
//! a root LaunchDaemon running the Agent, and a LaunchAgent in the LoginWindow and Aqua sessions doing its screen
//! work (ADR 0001). Every path hangs off `Layout::root`, and every external command goes through `run`, so the
//! file layout and the `launchctl` calls are tested without touching the Device.

use std::path::{Path, PathBuf};

use crate::install::{system, write};
pub use crate::install::{Options, Run};

// shortcut: ADR 0001 calls the root process "a root daemon" and this job is a launchd LaunchDaemon, so the label
// and the LaunchDaemon wording stay; prose elsewhere says "Agent service" (GLOSSARY.md: Agent — _Avoid_: daemon).
pub const DAEMON: &str = "dev.remotecli.agent";
pub const SESSION: &str = "dev.remotecli.agent.session";
const INFO_PLIST: &str = include_str!("../../../packaging/macos/Info.plist");

pub struct Layout {
    pub root: PathBuf,
}

impl Layout {
    fn at(&self, path: &str) -> PathBuf {
        crate::install::at(&self.root, path)
    }
    pub fn app(&self) -> PathBuf {
        self.at("/Applications/rcli.app")
    }
    pub fn exe(&self) -> PathBuf {
        self.app().join("Contents/MacOS/rcli")
    }
    /// `rcli` on the PATH for the terminal.
    pub fn link(&self) -> PathBuf {
        self.at("/usr/local/bin/rcli")
    }
    pub fn state_dir(&self) -> PathBuf {
        self.at("/Library/Application Support/rcli")
    }
    pub fn daemon_plist(&self) -> PathBuf {
        self.at(&format!("/Library/LaunchDaemons/{DAEMON}.plist"))
    }
    pub fn session_plist(&self) -> PathBuf {
        self.at(&format!("/Library/LaunchAgents/{SESSION}.plist"))
    }
    /// Where the Agent service listens for the session helpers, beside the IPC socket.
    pub fn session_socket(&self) -> PathBuf {
        self.at("/var/run/rcli/session.sock")
    }
    fn log(&self) -> PathBuf {
        self.at("/Library/Logs/rcli-agent.log")
    }
    /// The machine-wide Controller configuration, readable by every local user (the state directory is root-only).
    pub fn controller_config(&self) -> PathBuf {
        crate::config::machine_path(Some(&self.root))
    }
}

fn xml(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn strings(items: &[&str]) -> String {
    items.iter().map(|s| format!("    <string>{}</string>\n", xml(s))).collect()
}

fn plist(body: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
         \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n<dict>\n{body}</dict>\n</plist>\n"
    )
}

/// The root LaunchDaemon: the Agent itself. The Server Key travels in its environment, never on a command line
/// (`ps` shows every process's arguments to every user), so this file is root-only.
pub fn daemon_plist(layout: &Layout, opts: &Options) -> String {
    let exe = layout.exe().display().to_string();
    let state = layout.state_dir().display().to_string();
    let session = layout.session_socket().display().to_string();
    let log = layout.log().display().to_string();
    let args = [&exe, "agent", "run", "--server", &opts.server, "--state-dir", &state, "--name", &opts.name, "--session-socket", &session];
    plist(&format!(
        "  <key>Label</key><string>{DAEMON}</string>\n  <key>ProgramArguments</key>\n  <array>\n{}  </array>\n  \
         <key>EnvironmentVariables</key>\n  <dict>\n    <key>RCLI_SERVER_KEY</key><string>{}</string>\n  </dict>\n  \
         <key>RunAtLoad</key><true/>\n  <key>KeepAlive</key><true/>\n  <key>StandardOutPath</key><string>{log}</string>\n  \
         <key>StandardErrorPath</key><string>{log}</string>\n",
        strings(&args),
        xml(&opts.key),
    ))
}

/// The session helper: launchd starts one in the login window (as root) and one in each user's Aqua session.
pub fn session_plist(layout: &Layout) -> String {
    let exe = layout.exe().display().to_string();
    let session = layout.session_socket().display().to_string();
    plist(&format!(
        "  <key>Label</key><string>{SESSION}</string>\n  <key>LimitLoadToSessionType</key>\n  <array>\n{}  </array>\n  \
         <key>ProgramArguments</key>\n  <array>\n{}  </array>\n  <key>RunAtLoad</key><true/>\n  \
         <key>KeepAlive</key><true/>\n  <key>ProcessType</key><string>Interactive</string>\n",
        strings(&["LoginWindow", "Aqua"]),
        strings(&[&exe, "agent", "session", "--socket", &session]),
    ))
}

/// The app bundle's Info.plist, shared with `packaging/macos/build-dmg.sh`.
pub fn info_plist() -> String {
    INFO_PLIST.replace("{{VERSION}}", env!("CARGO_PKG_VERSION"))
}

/// The `.app` that contains `exe`, if it runs from one (the `.dmg` or an unpacked copy).
fn bundle_of(exe: &Path) -> Option<PathBuf> {
    let app = exe.parent()?.parent()?.parent()?;
    (app.extension()? == "app").then(|| app.to_owned())
}

/// Puts the app at `app`, links its binary at `link` and records the Device Settings in `state` (0700). Shared by
/// both Install Modes.
fn install_app(app: &Path, link: &Path, state: &Path, opts: &Options, exe: &Path, run: Run) -> anyhow::Result<()> {
    let app_exe = app.join("Contents/MacOS/rcli");
    match bundle_of(exe) {
        Some(bundle) if bundle == app => {}
        Some(bundle) => {
            let _ = std::fs::remove_dir_all(app);
            run("/usr/bin/ditto", &[&bundle.to_string_lossy(), &app.to_string_lossy()])?;
        }
        // The curl one-liner downloads a bare binary: build the same bundle around it.
        None => {
            let _ = std::fs::remove_dir_all(app);
            std::fs::create_dir_all(app_exe.parent().unwrap())?;
            std::fs::copy(exe, &app_exe)?;
            write(&app.join("Contents/Info.plist"), &info_plist(), 0o644)?;
            write(&app.join("Contents/Resources/rcli.icns"), include_bytes!("../../../assets/icons/rcli.icns"), 0o644)?;
            // Ad-hoc sign the bundle so TCC files the grants under the app's identifier.
            run("/usr/bin/codesign", &["--force", "--sign", "-", &app.to_string_lossy()])?;
        }
    }
    std::fs::create_dir_all(link.parent().unwrap())?;
    let _ = std::fs::remove_file(link);
    std::os::unix::fs::symlink(&app_exe, link)?;

    std::fs::create_dir_all(state)?;
    std::fs::set_permissions(state, std::os::unix::fs::PermissionsExt::from_mode(0o700))?;
    crate::install::save_settings(state, opts)
}

/// Puts the app in /Applications and records the Device Settings the install asked for. `exe` is the running binary.
pub fn install_files(layout: &Layout, opts: &Options, exe: &Path, run: Run) -> anyhow::Result<()> {
    install_app(&layout.app(), &layout.link(), &layout.state_dir(), opts, exe, run)?;
    let state = layout.state_dir();
    // No secret here: `uninstall` needs the Server Address to remove this Device, and the Server Key is in the
    // root-only LaunchDaemon plist.
    write(&state.join("install.json"), &serde_json::json!({ "server": opts.server }).to_string(), 0o600)
}

/// Writes both launchd jobs and starts them: the Agent's LaunchDaemon now, the session helper in the console user's session now
/// (and launchd starts it in every later login window and Aqua session).
pub fn install_jobs(layout: &Layout, opts: &Options, console_uid: Option<u32>, run: Run) -> anyhow::Result<()> {
    write(&layout.daemon_plist(), &daemon_plist(layout, opts), 0o600)?;
    write(&layout.session_plist(), &session_plist(layout), 0o644)?;
    // A reinstall replaces the running jobs; on a first install there is nothing to stop.
    let _ = run("/bin/launchctl", &["bootout", &format!("system/{DAEMON}")]);
    run("/bin/launchctl", &["bootstrap", "system", &layout.daemon_plist().to_string_lossy()])?;
    if let Some(uid) = console_uid.filter(|&uid| uid != 0) {
        let _ = run("/bin/launchctl", &["bootout", &format!("gui/{uid}/{SESSION}")]);
        run("/bin/launchctl", &["bootstrap", &format!("gui/{uid}"), &layout.session_plist().to_string_lossy()])?;
    }
    Ok(())
}

/// Stops both jobs and deletes everything `install_files`/`install_jobs` wrote, including the Device's local data.
pub fn uninstall_files(layout: &Layout, console_uid: Option<u32>, run: Run) -> anyhow::Result<()> {
    let _ = run("/bin/launchctl", &["bootout", &format!("system/{DAEMON}")]);
    if let Some(uid) = console_uid.filter(|&uid| uid != 0) {
        let _ = run("/bin/launchctl", &["bootout", &format!("gui/{uid}/{SESSION}")]);
    }
    for file in [layout.daemon_plist(), layout.session_plist(), layout.link(), layout.session_socket(), layout.controller_config()] {
        let _ = std::fs::remove_file(file);
    }
    for dir in [layout.state_dir(), layout.app()] {
        if dir.exists() {
            std::fs::remove_dir_all(dir)?;
        }
    }
    Ok(())
}

/// The user logged in on the console (root at the login window).
pub fn console_uid() -> Option<u32> {
    Some(std::os::unix::fs::MetadataExt::uid(&std::fs::metadata("/dev/console").ok()?))
}

/// `sudo rcli install`: app, settings, registration and the machine-wide Controller configuration, then both launchd
/// jobs.
pub async fn install(opts: Options, out: &mut dyn std::io::Write) -> anyhow::Result<()> {
    anyhow::ensure!(crate::upgrade::is_root(), "run it as root: sudo rcli install --server … --key …");
    let platform = rcli_agent::macos::MacPlatform::new(opts.name.clone())?;
    install_at(&Layout { root: "/".into() }, opts, &platform, console_uid(), &mut system, out).await
}

/// `install` on `layout`, registering with `platform`'s inventory; the `--root` test seam calls it directly.
pub async fn install_at(
    layout: &Layout,
    opts: Options,
    platform: &dyn rcli_agent::platform::Platform,
    console_uid: Option<u32>,
    run: Run<'_>,
    out: &mut dyn std::io::Write,
) -> anyhow::Result<()> {
    // A reinstall must not destroy a working install when the new Server Key is wrong.
    crate::install::check_server_key(&opts).await?;
    let reinstall = layout.state_dir().join("credentials.json").exists();
    install_files(layout, &opts, &std::env::current_exe()?, run)?;
    // Register now, so a wrong Server Key fails here and the Device ID can be shown; the service finds the credentials.
    let registered = match crate::install::register(&opts, platform, &layout.state_dir(), &layout.controller_config()).await {
        Ok(registered) => registered,
        Err(e) => {
            if !reinstall {
                let _ = uninstall_files(layout, console_uid, run);
            }
            return Err(e.context(crate::install::REGISTER_FAILED));
        }
    };
    install_jobs(layout, &opts, console_uid, run)?;
    write!(
        out,
        "installed {} with the launchd jobs {DAEMON} and {SESSION}\n{registered}\
         Next: open rcli from Applications and allow it Screen Recording and Accessibility \
         (System Settings > Privacy & Security), so Controllers can see and use the screen.\n",
        layout.app().display(),
    )?;
    Ok(())
}

/// `sudo rcli uninstall`: removes the Device from the Server (see `install::unregister`), then stops and removes both
/// jobs, the app, the Device's local data and the machine-wide Controller configuration.
pub async fn uninstall(out: &mut dyn std::io::Write) -> anyhow::Result<()> {
    anyhow::ensure!(crate::upgrade::is_root(), "run it as root: sudo rcli uninstall");
    uninstall_at(&Layout { root: "/".into() }, console_uid(), &mut system, out).await
}

pub async fn uninstall_at(layout: &Layout, console_uid: Option<u32>, run: Run<'_>, out: &mut dyn std::io::Write) -> anyhow::Result<()> {
    let server = std::fs::read_to_string(layout.state_dir().join("install.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .and_then(|v| v["server"].as_str().map(str::to_owned));
    crate::install::unregister(server, &layout.state_dir(), out).await?;
    uninstall_files(layout, console_uid, run)?;
    writeln!(out, "removed the launchd jobs {DAEMON} and {SESSION}, {} and the Device's local data", layout.app().display())?;
    Ok(())
}

/// User Mode (ADR 0012): the install of one OS user, under their home. No root anywhere: one LaunchAgent runs the
/// Agent in the user's Aqua session, where it does its own screen work (no session helper).
pub struct UserLayout {
    pub home: PathBuf,
    /// The user's rcli config directory (`config::dir`), where their Controller configuration lives.
    pub config_dir: PathBuf,
}

impl UserLayout {
    fn real() -> anyhow::Result<Self> {
        let home = std::env::var_os("HOME").filter(|h| !h.is_empty()).ok_or_else(|| anyhow::anyhow!("HOME is not set"))?;
        Ok(Self { home: home.into(), config_dir: crate::config::dir() })
    }
    fn at(&self, path: &str) -> PathBuf {
        self.home.join(path)
    }
    /// `~/Applications`: TCC grants stick to the bundle, and the user may write there.
    pub fn app(&self) -> PathBuf {
        self.at("Applications/rcli.app")
    }
    pub fn exe(&self) -> PathBuf {
        self.app().join("Contents/MacOS/rcli")
    }
    // shortcut: `~/.local/bin` is not on macOS's default PATH; the install says where the link is. Add it to the shell
    // profile if people miss that line.
    pub fn link(&self) -> PathBuf {
        self.at(".local/bin/rcli")
    }
    /// Credentials, Device Settings, the Secret Store, the saved Server and the IPC socket; 0700.
    pub fn state_dir(&self) -> PathBuf {
        self.at("Library/Application Support/rcli")
    }
    pub fn plist(&self) -> PathBuf {
        self.at(&format!("Library/LaunchAgents/{DAEMON}.plist"))
    }
    pub fn socket(&self) -> PathBuf {
        self.at(rcli_agent::ipc::USER_SOCKET)
    }
    fn log(&self) -> PathBuf {
        self.at("Library/Logs/rcli-agent.log")
    }
    /// This user's Controller configuration (ADR 0012: per user in User Mode).
    pub fn controller_config(&self) -> PathBuf {
        self.config_dir.join(crate::config::CONTROLLER_FILE)
    }
}

/// The User Mode LaunchAgent: the Agent in the user's Aqua session, started at login. Its label is the Agent's
/// (`DAEMON`) in the user's `gui/<uid>` domain. No `--server`: the Agent reads the Server saved in its state dir, or
/// waits for one.
pub fn user_plist(layout: &UserLayout, opts: &Options) -> String {
    let exe = layout.exe().display().to_string();
    let state = layout.state_dir().display().to_string();
    let socket = layout.socket().display().to_string();
    let log = layout.log().display().to_string();
    let args = [&exe, "agent", "run", "--state-dir", &state, "--name", &opts.name, "--socket", &socket];
    plist(&format!(
        "  <key>Label</key><string>{DAEMON}</string>\n  <key>LimitLoadToSessionType</key>\n  <array>\n{}  </array>\n  \
         <key>ProgramArguments</key>\n  <array>\n{}  </array>\n  <key>RunAtLoad</key><true/>\n  \
         <key>KeepAlive</key><true/>\n  <key>ProcessType</key><string>Interactive</string>\n  \
         <key>StandardOutPath</key><string>{}</string>\n  <key>StandardErrorPath</key><string>{}</string>\n",
        strings(&["Aqua"]),
        strings(&args),
        xml(&log),
        xml(&log),
    ))
}

/// The User Mode LaunchAgent's job in the `gui/<uid>` domain.
pub fn user_job(uid: u32) -> String {
    format!("gui/{uid}/{DAEMON}")
}

/// This process's uid (`id -u`): the cli crate has no libc.
pub fn my_uid() -> Option<u32> {
    let out = std::process::Command::new("/usr/bin/id").arg("-u").output().ok()?;
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

/// Stops the LaunchAgent and deletes everything a User Mode install wrote, including the Device's local data.
pub fn uninstall_user_files(layout: &UserLayout, uid: Option<u32>, run: Run) -> anyhow::Result<()> {
    if let Some(uid) = uid {
        let _ = run("/bin/launchctl", &["bootout", &user_job(uid)]);
    }
    for file in [layout.plist(), layout.link(), layout.controller_config()] {
        let _ = std::fs::remove_file(file);
    }
    for dir in [layout.state_dir(), layout.app()] {
        if dir.exists() {
            std::fs::remove_dir_all(dir)?;
        }
    }
    Ok(())
}

/// `rcli install` in User Mode on `layout`; `uid` = None (the test seam) loads no LaunchAgent. With a Server it is
/// checked and the Device registered first (a wrong Server Key fails before anything stays); without one the Agent
/// serves local control until one is set.
pub async fn install_user_at(
    layout: &UserLayout,
    opts: Options,
    platform: &dyn rcli_agent::platform::Platform,
    uid: Option<u32>,
    run: Run<'_>,
    out: &mut dyn std::io::Write,
) -> anyhow::Result<()> {
    let state = layout.state_dir();
    let files = |opts: &Options, run: Run<'_>| {
        install_app(&layout.app(), &layout.link(), &state, opts, &std::env::current_exe()?, run)?;
        if !opts.server.is_empty() {
            let server = serde_json::json!({ "server": opts.server, "key": opts.key }).to_string();
            write(&state.join(rcli_agent::ipc::SERVER_FILE), &server, 0o600)?;
        }
        Ok(())
    };
    let remove = |run: Run<'_>| drop(uninstall_user_files(layout, None, run));
    let (opts, registered) =
        crate::install::user_install(&state, &layout.controller_config(), opts, platform, &mut *run, files, remove).await?;
    write(&layout.plist(), &user_plist(layout, &opts), 0o644)?;
    if let Some(uid) = uid {
        // A reinstall replaces the running job; on a first install there is nothing to stop.
        let _ = run("/bin/launchctl", &["bootout", &user_job(uid)]);
        run("/bin/launchctl", &["bootstrap", &format!("gui/{uid}"), &layout.plist().to_string_lossy()])?;
    }
    write!(
        out,
        "installed {} for this user (User Mode, starts at login) with the LaunchAgent {DAEMON}; `rcli` is linked at {}\n\
         {registered}Next: open rcli from Applications in your home and allow it Screen Recording and Accessibility \
         (System Settings > Privacy & Security).\n\
         User Mode cannot reach the lock and login screens or work before login; `rcli service install` switches to \
         Service Mode for that.\n",
        layout.app().display(),
        layout.link().display(),
    )?;
    Ok(())
}

/// `rcli install` as a normal user: User Mode for this user.
pub async fn install_user(opts: Options, out: &mut dyn std::io::Write) -> anyhow::Result<()> {
    let platform = rcli_agent::macos::MacPlatform::new(opts.name.clone())?;
    install_user_at(&UserLayout::real()?, opts, &platform, my_uid(), &mut system, out).await
}

/// `rcli uninstall` in User Mode: removes the Device from the Server it was given, then this user's install.
pub async fn uninstall_user_at(layout: &UserLayout, uid: Option<u32>, run: Run<'_>, out: &mut dyn std::io::Write) -> anyhow::Result<()> {
    let server = rcli_agent::ipc::saved_server(&layout.state_dir()).map(|s| s.addr);
    crate::install::unregister(server, &layout.state_dir(), out).await?;
    uninstall_user_files(layout, uid, run)?;
    writeln!(out, "removed the LaunchAgent {DAEMON}, {} and the Device's local data", layout.app().display())?;
    Ok(())
}

pub async fn uninstall_user(out: &mut dyn std::io::Write) -> anyhow::Result<()> {
    uninstall_user_at(&UserLayout::real()?, my_uid(), &mut system, out).await
}

/// `rcli service install` as root (ticket 15): moves `user`'s User Mode install to the LaunchDaemon, keeping the
/// Device ID, Device Password and Device Settings. `uid` (None in the `--root` test seam) owns the LaunchAgent.
#[allow(clippy::too_many_arguments)]
pub async fn to_service_at(
    user: &UserLayout,
    uid: Option<u32>,
    layout: &Layout,
    console_uid: Option<u32>,
    name: String,
    platform: &dyn rcli_agent::platform::Platform,
    run: Run<'_>,
    out: &mut dyn std::io::Write,
) -> anyhow::Result<()> {
    let opts = crate::install::hand_to_service(&user.state_dir(), &layout.state_dir(), name)?;
    install_at(layout, opts, platform, console_uid, &mut *run, out).await?;
    uninstall_user_files(user, uid, run)?;
    writeln!(out, "Service Mode: the User Mode install in {} is removed", user.home.display())?;
    Ok(())
}

/// `rcli service uninstall` as root: hands the LaunchDaemon's Device back to `user`'s User Mode, installs that as
/// `owner` in their login session (in the `--root` test seam, None: in-process with `platform`), then removes the
/// LaunchDaemon.
#[allow(clippy::too_many_arguments)]
pub async fn to_user_at(
    layout: &Layout,
    user: &UserLayout,
    owner: Option<&crate::install::Owner>,
    console_uid: Option<u32>,
    name: String,
    platform: &dyn rcli_agent::platform::Platform,
    run: Run<'_>,
    out: &mut dyn std::io::Write,
) -> anyhow::Result<()> {
    let (identity, opts) = crate::install::hand_to_user_opts(&layout.state_dir(), &layout.controller_config(), &user.state_dir(), name)?;
    match owner {
        Some(owner) => {
            owner.give(&user.state_dir())?;
            // `rcli install` as the user in their login session: their files, their LaunchAgent, the Server just
            // handed over.
            let exe = std::env::current_exe()?.to_string_lossy().into_owned();
            let uid = owner.uid.to_string();
            let (name, terminal_user) = (&opts.name, &opts.terminal_user);
            let args = ["asuser", &uid, "/usr/bin/sudo", "-H", "-u", &owner.name, &exe, "install", "--name", name, "--terminal-user", terminal_user];
            run("/bin/launchctl", &args)?;
        }
        None => install_user_at(user, opts, platform, None, &mut *run, &mut std::io::sink()).await?,
    }
    uninstall_files(layout, console_uid, run)?;
    write!(out, "User Mode: the LaunchDaemon is removed\n{identity}")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rcli_agent::settings::{DeviceSettings, RootAccess};

    fn temp_root(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rcli-install-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn opts() -> Options {
        Options {
            server: "rcli.example.com:7443".into(),
            key: "rk_ab12_s3cr&t".into(),
            name: "mac-mini".into(),
            terminal_user: "alice".into(),
            allow_root: true,
        }
    }

    #[test]
    fn install_puts_the_app_both_jobs_and_the_settings_in_place_and_uninstall_removes_them() {
        let root = temp_root("roundtrip");
        let layout = Layout { root: root.clone() };
        let exe = root.join("download/rcli");
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        std::fs::write(&exe, b"binary").unwrap();
        let mut calls: Vec<String> = vec![];
        let mut run = |program: &str, args: &[&str]| {
            calls.push(format!("{program} {}", args.join(" ")));
            Ok(())
        };

        install_files(&layout, &opts(), &exe, &mut run).unwrap();
        install_jobs(&layout, &opts(), Some(501), &mut run).unwrap();

        let app = root.join("Applications/rcli.app");
        assert_eq!(std::fs::read(app.join("Contents/MacOS/rcli")).unwrap(), b"binary");
        let info = std::fs::read_to_string(app.join("Contents/Info.plist")).unwrap();
        assert!(info.contains("<string>dev.remotecli.rcli</string>") && !info.contains("{{VERSION}}"));
        // Ticket 12: the bundle built around a bare binary has the icon its Info.plist names.
        assert!(info.contains("<key>CFBundleIconFile</key><string>rcli</string>") && app.join("Contents/Resources/rcli.icns").is_file());
        assert_eq!(std::fs::read_link(root.join("usr/local/bin/rcli")).unwrap(), app.join("Contents/MacOS/rcli"));
        let settings = DeviceSettings::load(&root.join("Library/Application Support/rcli"));
        let install = std::fs::read_to_string(root.join("Library/Application Support/rcli/install.json")).unwrap();
        assert_eq!(serde_json::from_str::<serde_json::Value>(&install).unwrap()["server"], "rcli.example.com:7443");
        assert_eq!((settings.terminal_user.as_str(), settings.root), ("alice", RootAccess::Always));

        use std::os::unix::fs::PermissionsExt;
        let daemon = root.join("Library/LaunchDaemons/dev.remotecli.agent.plist");
        let session = root.join("Library/LaunchAgents/dev.remotecli.agent.session.plist");
        assert_eq!(std::fs::metadata(&daemon).unwrap().permissions().mode() & 0o777, 0o600, "it holds the Server Key");
        assert_eq!(std::fs::metadata(&session).unwrap().permissions().mode() & 0o777, 0o644);

        let app = app.display();
        let daemon_s = daemon.display();
        let session_s = session.display();
        assert_eq!(
            calls,
            vec![
                format!("/usr/bin/codesign --force --sign - {app}"),
                "/bin/launchctl bootout system/dev.remotecli.agent".to_owned(),
                format!("/bin/launchctl bootstrap system {daemon_s}"),
                "/bin/launchctl bootout gui/501/dev.remotecli.agent.session".to_owned(),
                format!("/bin/launchctl bootstrap gui/501 {session_s}"),
            ]
        );

        calls.clear();
        uninstall_files(&layout, Some(501), &mut |program: &str, args: &[&str]| {
            calls.push(format!("{program} {}", args.join(" ")));
            Ok(())
        })
        .unwrap();
        assert_eq!(
            calls,
            vec!["/bin/launchctl bootout system/dev.remotecli.agent", "/bin/launchctl bootout gui/501/dev.remotecli.agent.session"]
        );
        for gone in ["Applications/rcli.app", "usr/local/bin/rcli", "Library/Application Support/rcli"] {
            assert!(!root.join(gone).exists(), "{gone} is left behind");
        }
        assert!(!daemon.exists() && !session.exists(), "a launchd job is left behind");
        let _ = std::fs::remove_dir_all(root);
    }

    /// Ticket 13: User Mode needs no root. The app and state live in the home, a LaunchAgent starts the Agent at
    /// login in the user's own session, a Server is optional, and uninstall leaves nothing behind.
    #[tokio::test]
    async fn a_user_mode_install_with_no_server_lives_in_the_home_and_uninstall_removes_it() {
        use std::os::unix::fs::PermissionsExt;
        let home = temp_root("user-mode");
        let layout = UserLayout { home: home.clone(), config_dir: home.join(".config/rcli") };
        let opts = Options { server: String::new(), key: String::new(), ..opts() };
        let platform = rcli_agent::platform::FakePlatform::new("mac-mini");
        let mut calls: Vec<String> = vec![];
        let mut out = vec![];
        let mut run = |program: &str, args: &[&str]| {
            calls.push(format!("{program} {}", args.join(" ")));
            Ok(())
        };

        install_user_at(&layout, opts, &platform, Some(501), &mut run, &mut out).await.unwrap();

        let out = String::from_utf8(out).unwrap();
        assert!(out.contains("No Server yet") && !out.contains("Device ID:"), "{out}");
        let app = home.join("Applications/rcli.app");
        assert!(app.join("Contents/MacOS/rcli").is_file() && app.join("Contents/Info.plist").is_file());
        assert_eq!(std::fs::read_link(home.join(".local/bin/rcli")).unwrap(), app.join("Contents/MacOS/rcli"));
        let state = home.join("Library/Application Support/rcli");
        assert_eq!(std::fs::metadata(&state).unwrap().permissions().mode() & 0o777, 0o700);
        assert!(!state.join("server.json").exists(), "no Server was given");
        let plist = home.join("Library/LaunchAgents/dev.remotecli.agent.plist");
        let text = std::fs::read_to_string(&plist).unwrap();
        let args = format!(
            "    <string>agent</string>\n    <string>run</string>\n    <string>--state-dir</string>\n    <string>{}</string>\n    \
             <string>--name</string>\n    <string>mac-mini</string>\n    <string>--socket</string>\n    <string>{}</string>\n",
            state.display(),
            home.join(rcli_agent::ipc::USER_SOCKET).display()
        );
        assert!(text.contains(&args) && text.contains("<string>Aqua</string>") && !text.contains("--server"), "{text}");
        let (app_s, plist_s) = (app.display(), plist.display());
        assert_eq!(
            calls,
            [
                format!("/usr/bin/codesign --force --sign - {app_s}"),
                "/bin/launchctl bootout gui/501/dev.remotecli.agent".into(),
                format!("/bin/launchctl bootstrap gui/501 {plist_s}"),
            ]
        );

        calls.clear();
        uninstall_user_at(&layout, Some(501), &mut |program: &str, args: &[&str]| {
            calls.push(format!("{program} {}", args.join(" ")));
            Ok(())
        }, &mut vec![])
        .await
        .unwrap();
        assert_eq!(calls, ["/bin/launchctl bootout gui/501/dev.remotecli.agent"]);
        for gone in ["Applications/rcli.app", ".local/bin/rcli", "Library/Application Support/rcli", "Library/LaunchAgents/dev.remotecli.agent.plist"] {
            assert!(!home.join(gone).exists(), "{gone} is left behind");
        }
        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn the_launchdaemon_runs_the_agent_as_root_with_the_key_out_of_its_arguments() {
        let layout = Layout { root: "/".into() };
        let daemon = daemon_plist(&layout, &opts());
        let expected_args = "    <string>/Applications/rcli.app/Contents/MacOS/rcli</string>\n    <string>agent</string>\n    \
             <string>run</string>\n    <string>--server</string>\n    <string>rcli.example.com:7443</string>\n    \
             <string>--state-dir</string>\n    <string>/Library/Application Support/rcli</string>\n    \
             <string>--name</string>\n    <string>mac-mini</string>\n    <string>--session-socket</string>\n    \
             <string>/var/run/rcli/session.sock</string>\n";
        assert!(daemon.contains(expected_args), "{daemon}");
        assert!(daemon.contains("<key>RCLI_SERVER_KEY</key><string>rk_ab12_s3cr&amp;t</string>"), "{daemon}");
        assert!(daemon.contains("<key>KeepAlive</key><true/>"));

        let session = session_plist(&layout);
        assert!(session.contains(
            "<key>LimitLoadToSessionType</key>\n  <array>\n    <string>LoginWindow</string>\n    <string>Aqua</string>\n  </array>"
        ));
        assert!(session.contains("<string>session</string>\n    <string>--socket</string>\n    <string>/var/run/rcli/session.sock</string>"));
        assert!(!session.contains("rk_"), "the session helper never gets the Server Key");
    }

    #[test]
    fn plists_are_valid_property_lists() {
        let layout = Layout { root: "/".into() };
        let user = UserLayout { home: "/Users/a&b".into(), config_dir: "/Users/a&b/.config/rcli".into() };
        let dir = temp_root("plutil");
        for (name, text) in [("daemon", daemon_plist(&layout, &opts())), ("session", session_plist(&layout)), ("info", info_plist()), ("user", user_plist(&user, &opts()))] {
            let path = dir.join(format!("{name}.plist"));
            std::fs::write(&path, text).unwrap();
            let lint = std::process::Command::new("/usr/bin/plutil").arg("-lint").arg(&path).output().unwrap();
            assert!(lint.status.success(), "{name}: {}", String::from_utf8_lossy(&lint.stdout));
        }
        let _ = std::fs::remove_dir_all(dir);
    }
}
