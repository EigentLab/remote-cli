//! `rcli install|uninstall` on Linux. As a normal user (User Mode, ADR 0012): everything under the user's home and
//! a systemd user unit that starts the Agent at login (`UserLayout`). As root (ticket 16): the binary in
//! /usr/local/lib/rcli, a root systemd service that comes back after a reboot (`Layout`). The Server Key travels in a
//! private file, never on the command line (`ps` shows every process's arguments to every user).
//! Every path hangs off the layout's root and every external command goes through `run`, so the layout, the unit
//! file and the systemctl pipeline are tested without touching the Device.
// Off Linux only the tests use it.
#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use std::path::{Path, PathBuf};

use crate::install::{system, write};
pub use crate::install::{Options, Run};

pub const UNIT: &str = "rcli-agent.service";
const KEY_ENV: &str = "RCLI_SERVER_KEY";
/// The app-menu entry `rcli install` places; running it opens the GUI window (ticket 14).
const DESKTOP: &str = include_str!("../../../packaging/linux/rcli.desktop");
/// The icon the desktop entry names (`Icon=rcli`), per hicolor size: 48 is the theme spec's minimum, 256 for large views.
const ICONS: [(&str, &[u8]); 2] = [
    ("48x48", include_bytes!("../../../assets/icons/rcli-48.png")),
    ("256x256", include_bytes!("../../../assets/icons/rcli-256.png")),
];

/// The icon files under the hicolor theme directory `theme`.
fn icon_files(theme: &Path) -> impl Iterator<Item = (PathBuf, &'static [u8])> + '_ {
    ICONS.into_iter().map(move |(size, png)| (theme.join(size).join("apps/rcli.png"), png))
}

pub struct Layout {
    pub root: PathBuf,
}

impl Layout {
    fn at(&self, path: &str) -> PathBuf {
        crate::install::at(&self.root, path)
    }
    /// The stable path the service runs; `rcli update` replaces this file in place.
    pub fn exe(&self) -> PathBuf {
        self.at("/usr/local/lib/rcli/rcli")
    }
    /// `rcli` on the PATH for the terminal.
    pub fn link(&self) -> PathBuf {
        self.at("/usr/local/bin/rcli")
    }
    /// Credentials, Device Settings and the Secret Store. `StateDirectory=` recreates it as root 0700.
    pub fn state_dir(&self) -> PathBuf {
        self.at("/var/lib/rcli")
    }
    pub fn unit(&self) -> PathBuf {
        self.at(&format!("/etc/systemd/system/{UNIT}"))
    }
    /// Holds the Server Key; root-only.
    pub fn env_file(&self) -> PathBuf {
        self.at("/etc/rcli/agent.env")
    }
    /// Where the service records the Server Address for `uninstall` (no secret: the Key is in `env_file`).
    pub fn install_file(&self) -> PathBuf {
        self.state_dir().join("install.json")
    }
    /// Where the root service listens for the console session helper.
    pub fn session_socket(&self) -> PathBuf {
        self.at("/run/rcli/session.sock")
    }
    /// The IPC socket `rcli agent …`, the GUI and scripts use (the `agent run` default).
    pub fn ipc_socket(&self) -> PathBuf {
        self.at("/run/rcli/agent.sock")
    }
    /// The app-menu entry: running `rcli` with no arguments opens the window.
    pub fn desktop_file(&self) -> PathBuf {
        self.at("/usr/share/applications/rcli.desktop")
    }
    /// The icon theme the app-menu entry's icon goes in.
    pub fn icon_theme(&self) -> PathBuf {
        self.at("/usr/share/icons/hicolor")
    }
    /// The machine-wide Controller configuration, readable by every local user (the state directory is root-only).
    pub fn controller_config(&self) -> PathBuf {
        crate::config::machine_path(Some(&self.root))
    }
}

/// Quotes one argument for a systemd unit file: bare when simple, else double-quoted with systemd's escapes.
fn systemd_arg(s: &str) -> String {
    if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "._:/@+-".contains(c)) {
        return s.to_owned();
    }
    let mut escaped = String::with_capacity(s.len() + 2);
    for c in s.chars() {
        match c {
            '\\' | '"' => {
                escaped.push('\\');
                escaped.push(c);
            }
            // `%` starts a systemd specifier in Exec lines; `%%` is a literal one.
            '%' => escaped.push_str("%%"),
            _ => escaped.push(c),
        }
    }
    format!("\"{escaped}\"")
}

/// The root service: the Agent itself. `--session-socket` makes the Agent service spawn and supervise the console
/// session helper (see `rcli_agent::linux::session`). The Server Key comes from `EnvironmentFile`, and the unit
/// names that file, so the key is never an argument.
pub fn unit(layout: &Layout, opts: &Options) -> String {
    let exe = systemd_arg(&layout.exe().display().to_string());
    let state = systemd_arg(&layout.state_dir().display().to_string());
    let name = systemd_arg(&opts.name);
    let server = systemd_arg(&opts.server);
    let session = systemd_arg(&layout.session_socket().display().to_string());
    let env = systemd_arg(&layout.env_file().display().to_string());
    let ipc = systemd_arg(&layout.ipc_socket().display().to_string());
    format!(
        "[Unit]\n\
         Description=rcli Agent\n\
         After=network-online.target\n\
         Wants=network-online.target\n\
         \n\
         [Service]\n\
         Type=simple\n\
         User=root\n\
         EnvironmentFile={env}\n\
         Environment=RCLI_AGENT_SOCKET={ipc}\n\
         ExecStart={exe} agent run --server {server} --state-dir {state} --name {name} --session-socket {session}\n\
         Restart=always\n\
         RestartSec=2\n\
         # The IPC and session sockets live here, 0755 so local users can reach `rcli agent info` and disconnect.\n\
         RuntimeDirectory=rcli\n\
         RuntimeDirectoryMode=0755\n\
         StateDirectory=rcli\n\
         StateDirectoryMode=0700\n\
         \n\
         [Install]\n\
         WantedBy=multi-user.target\n"
    )
}

/// The root-only `EnvironmentFile`: one `RCLI_SERVER_KEY=…` line. systemd parses it shell-like; the Server Key's
/// alphabet (`rk_`, hex, `_`) needs no quoting.
pub fn environment_file(opts: &Options) -> String {
    format!("{KEY_ENV}={}\n", opts.key)
}

/// Copies the running binary to the stable path, links it, records the Device Settings and the Server Address, and
/// installs the app-menu entry.
/// `exe` is the running binary (the download, the deb payload or the RPM payload).
pub fn install_files(layout: &Layout, opts: &Options, exe: &Path, run: Run) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let target = layout.exe();
    if exe != target {
        if let Some(dir) = target.parent() {
            std::fs::create_dir_all(dir)?;
        }
        // Copy beside the target and rename, so a running service never sees a half-written binary.
        let temp = target.with_extension("new");
        std::fs::copy(exe, &temp)?;
        std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o755))?;
        std::fs::rename(&temp, &target)?;
    }
    let link = layout.link();
    std::fs::create_dir_all(link.parent().unwrap())?;
    let _ = std::fs::remove_file(&link);
    std::os::unix::fs::symlink(&target, &link)?;

    let state = layout.state_dir();
    std::fs::create_dir_all(&state)?;
    std::fs::set_permissions(&state, std::fs::Permissions::from_mode(0o700))?;
    crate::install::save_settings(&state, opts)?;
    // The app menu launches the GUI window: `rcli` with no arguments (ticket 14).
    write(&layout.desktop_file(), DESKTOP, 0o644)?;
    for (file, png) in icon_files(&layout.icon_theme()) {
        write(&file, png, 0o644)?;
    }
    // GTK ignores new icons while the theme's icon-theme.cache is older than the theme directory, which adding a file
    // in a size subdirectory does not touch. Best effort: no GTK, no cache to refresh.
    let _ = run("gtk-update-icon-cache", &["-q", &layout.icon_theme().to_string_lossy()]);
    write(&layout.install_file(), &serde_json::json!({ "server": opts.server }).to_string(), 0o600)
}

/// Writes the unit and the root-only key file, then enables and (re)starts the service. `restart` also covers an
/// upgrade: it starts a first install and replaces the running binary's process on a reinstall.
pub fn install_service(layout: &Layout, opts: &Options, run: Run) -> anyhow::Result<()> {
    write(&layout.unit(), &unit(layout, opts), 0o644)?;
    write(&layout.env_file(), &environment_file(opts), 0o600)?;
    run("systemctl", &["daemon-reload"])?;
    run("systemctl", &["enable", UNIT])?;
    run("systemctl", &["restart", UNIT])
}

/// Stops and removes the service, the binary, the key file and the Device's local data (credentials, settings and
/// secrets). The caller has already removed the Device from the Server.
pub fn uninstall_files(layout: &Layout, run: Run) -> anyhow::Result<()> {
    // A missing unit is not an error: a first install that failed after copying files must still be removable.
    let _ = run("systemctl", &["disable", "--now", UNIT]);
    for file in [layout.unit(), layout.env_file(), layout.session_socket(), layout.link(), layout.desktop_file(), layout.controller_config()] {
        let _ = std::fs::remove_file(file);
    }
    for (file, _) in icon_files(&layout.icon_theme()) {
        let _ = std::fs::remove_file(file);
    }
    // `disable --now` stops it; the reload forgets the removed unit.
    let _ = run("systemctl", &["daemon-reload"]);
    if layout.state_dir().exists() {
        std::fs::remove_dir_all(layout.state_dir())?;
    }
    if layout.exe().exists() {
        std::fs::remove_file(layout.exe())?;
    }
    for dir in [layout.exe().parent().map(Path::to_path_buf), layout.env_file().parent().map(Path::to_path_buf)] {
        if let Some(dir) = dir.filter(|d| d.exists()) {
            // Only if empty: /usr/local/lib may hold other programs.
            let _ = std::fs::remove_dir(&dir);
        }
    }
    Ok(())
}

/// Reads the Server Address `install_files` recorded, for `uninstall` (no secret; the Device's certificate comes
/// from `credentials.json`).
fn recorded_server(layout: &Layout) -> Option<String> {
    let text = std::fs::read_to_string(layout.install_file()).ok()?;
    serde_json::from_str::<serde_json::Value>(&text).ok()?["server"].as_str().map(str::to_owned)
}

/// User Mode (ADR 0012): the install of one OS user, under their home. No root anywhere.
pub struct UserLayout {
    pub home: PathBuf,
    /// The user's rcli config directory (`config::dir`), where their Controller configuration lives.
    pub config_dir: PathBuf,
}

impl UserLayout {
    // shortcut: the XDG defaults under $HOME; a custom XDG_DATA_HOME/XDG_STATE_HOME is not followed. Read them when a
    // user with custom XDG dirs reports a missing install.
    fn real() -> anyhow::Result<Self> {
        let home = std::env::var_os("HOME").filter(|h| !h.is_empty()).ok_or_else(|| anyhow::anyhow!("HOME is not set"))?;
        Ok(Self { home: home.into(), config_dir: crate::config::dir() })
    }
    fn at(&self, path: &str) -> PathBuf {
        self.home.join(path)
    }
    /// The stable path the unit runs; `rcli update` replaces this file in place.
    pub fn exe(&self) -> PathBuf {
        self.at(".local/lib/rcli/rcli")
    }
    /// `rcli` on the PATH (`~/.local/bin` is on it on most desktops).
    pub fn link(&self) -> PathBuf {
        self.at(".local/bin/rcli")
    }
    /// Credentials, Device Settings, the Secret Store and the saved Server; 0700.
    pub fn state_dir(&self) -> PathBuf {
        self.at(".local/state/rcli")
    }
    pub fn unit(&self) -> PathBuf {
        self.at(&format!(".config/systemd/user/{UNIT}"))
    }
    pub fn desktop_file(&self) -> PathBuf {
        self.at(".local/share/applications/rcli.desktop")
    }
    pub fn icon_theme(&self) -> PathBuf {
        self.at(".local/share/icons/hicolor")
    }
    /// This user's Controller configuration (ADR 0012: per user in User Mode).
    pub fn controller_config(&self) -> PathBuf {
        self.config_dir.join(crate::config::CONTROLLER_FILE)
    }
}

/// The user unit: the Agent in the login session, started when the user logs in. No `--server`: the Agent reads the
/// Server saved in its state dir, or waits for one. Its socket is in the user's runtime dir (`%t`), theirs only.
pub fn user_unit(layout: &UserLayout, opts: &Options) -> String {
    let exe = systemd_arg(&layout.exe().display().to_string());
    let state = systemd_arg(&layout.state_dir().display().to_string());
    let name = systemd_arg(&opts.name);
    format!(
        "[Unit]\n\
         Description=rcli Agent (User Mode)\n\
         \n\
         [Service]\n\
         Type=simple\n\
         ExecStart={exe} agent run --state-dir {state} --name {name} --socket %t/rcli/agent.sock\n\
         Restart=always\n\
         RestartSec=2\n\
         \n\
         [Install]\n\
         WantedBy=default.target\n"
    )
}

/// Copies the binary into the home, links it, records the Device Settings, the Server (if given, 0600: it holds the
/// Server Key) and the app-menu entry.
pub fn install_user_files(layout: &UserLayout, opts: &Options, exe: &Path) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let target = layout.exe();
    if exe != target {
        std::fs::create_dir_all(target.parent().unwrap())?;
        // Copy beside the target and rename, so a running Agent never sees a half-written binary.
        let temp = target.with_extension("new");
        std::fs::copy(exe, &temp)?;
        std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o755))?;
        std::fs::rename(&temp, &target)?;
    }
    let link = layout.link();
    std::fs::create_dir_all(link.parent().unwrap())?;
    let _ = std::fs::remove_file(&link);
    std::os::unix::fs::symlink(&target, &link)?;
    let state = layout.state_dir();
    std::fs::create_dir_all(&state)?;
    std::fs::set_permissions(&state, std::fs::Permissions::from_mode(0o700))?;
    crate::install::save_settings(&state, opts)?;
    if !opts.server.is_empty() {
        let server = serde_json::json!({ "server": opts.server, "key": opts.key }).to_string();
        write(&state.join(rcli_agent::ipc::SERVER_FILE), &server, 0o600)?;
    }
    for (file, png) in icon_files(&layout.icon_theme()) {
        write(&file, png, 0o644)?;
    }
    write(&layout.desktop_file(), DESKTOP, 0o644)
}

/// Writes the user unit, then enables it for every login and (re)starts it now.
pub fn install_user_service(layout: &UserLayout, opts: &Options, run: Run) -> anyhow::Result<()> {
    write(&layout.unit(), &user_unit(layout, opts), 0o644)?;
    run("systemctl", &["--user", "daemon-reload"])?;
    run("systemctl", &["--user", "enable", UNIT])?;
    run("systemctl", &["--user", "restart", UNIT])
}

/// Stops and removes the user unit, the binary and the Device's local data. The caller has already removed the
/// Device from the Server.
pub fn uninstall_user_files(layout: &UserLayout, run: Run) -> anyhow::Result<()> {
    let _ = run("systemctl", &["--user", "disable", "--now", UNIT]);
    for file in [layout.unit(), layout.link(), layout.desktop_file(), layout.controller_config(), layout.exe()] {
        let _ = std::fs::remove_file(file);
    }
    for (file, _) in icon_files(&layout.icon_theme()) {
        let _ = std::fs::remove_file(file);
    }
    let _ = run("systemctl", &["--user", "daemon-reload"]);
    if layout.state_dir().exists() {
        std::fs::remove_dir_all(layout.state_dir())?;
    }
    if let Some(dir) = layout.exe().parent() {
        let _ = std::fs::remove_dir(dir);
    }
    Ok(())
}

/// `rcli install` in User Mode on `layout`. With a Server it is checked and the Device registered first (a wrong
/// Server Key fails before anything is written); without one the Agent serves local control until one is set.
pub async fn install_user_at(
    layout: &UserLayout,
    opts: Options,
    platform: &dyn rcli_agent::platform::Platform,
    run: Run<'_>,
    out: &mut dyn std::io::Write,
) -> anyhow::Result<()> {
    let (opts, registered) = crate::install::user_install(
        &layout.state_dir(),
        &layout.controller_config(),
        opts,
        platform,
        &mut *run,
        |opts, _| install_user_files(layout, opts, &std::env::current_exe()?),
        |run| drop(uninstall_user_files(layout, run)),
    )
    .await?;
    install_user_service(layout, &opts, run)?;
    write!(
        out,
        "installed {UNIT} for this user (User Mode, starts at login) and {}\n{registered}\
         User Mode cannot reach the lock and login screens, run the terminal as another user or root, or send input \
         on Wayland; `rcli service install` switches to Service Mode for that.\n",
        layout.exe().display(),
    )?;
    Ok(())
}

/// `rcli install` as a normal user: User Mode for this user.
#[cfg(target_os = "linux")]
pub async fn install_user(opts: Options, out: &mut dyn std::io::Write) -> anyhow::Result<()> {
    let platform = rcli_agent::linux::LinuxPlatform::new(opts.name.clone())?;
    install_user_at(&UserLayout::real()?, opts, &platform, &mut system, out).await
}

/// `rcli uninstall` in User Mode: removes the Device from the Server it was given, then this user's install.
pub async fn uninstall_user_at(layout: &UserLayout, run: Run<'_>, out: &mut dyn std::io::Write) -> anyhow::Result<()> {
    let server = rcli_agent::ipc::saved_server(&layout.state_dir()).map(|s| s.addr);
    crate::install::unregister(server, &layout.state_dir(), out).await?;
    uninstall_user_files(layout, run)?;
    writeln!(out, "removed {UNIT}, {} and the Device's local data", layout.exe().display())?;
    Ok(())
}

#[cfg(target_os = "linux")]
pub async fn uninstall_user(out: &mut dyn std::io::Write) -> anyhow::Result<()> {
    uninstall_user_at(&UserLayout::real()?, &mut system, out).await
}

/// `sudo rcli install`: files, Device Settings, registration and the machine-wide Controller configuration, then the
/// systemd service. Registering first makes a wrong Server Key fail at the install and lets the Device ID and Device
/// Password be printed.
#[cfg(target_os = "linux")]
pub async fn install(opts: Options, out: &mut dyn std::io::Write) -> anyhow::Result<()> {
    anyhow::ensure!(crate::upgrade::is_root(), "run it as root: sudo rcli install --server … --key …");
    let platform = rcli_agent::linux::LinuxPlatform::new(opts.name.clone())?;
    install_at(&Layout { root: "/".into() }, opts, &platform, &mut system, out).await
}

/// `install` on `layout`, registering with `platform`'s inventory; the `--root` test seam calls it directly.
pub async fn install_at(
    layout: &Layout,
    opts: Options,
    platform: &dyn rcli_agent::platform::Platform,
    run: Run<'_>,
    out: &mut dyn std::io::Write,
) -> anyhow::Result<()> {
    // A reinstall must not destroy a working install when the new Server Key is wrong.
    crate::install::check_server_key(&opts).await?;
    let reinstall = layout.state_dir().join("credentials.json").exists();
    install_files(layout, &opts, &std::env::current_exe()?, run)?;
    let registered = match crate::install::register(&opts, platform, &layout.state_dir(), &layout.controller_config()).await {
        Ok(registered) => registered,
        Err(e) => {
            if !reinstall {
                let _ = uninstall_files(layout, run);
            }
            return Err(e.context(crate::install::REGISTER_FAILED));
        }
    };
    install_service(layout, &opts, run)?;
    write!(
        out,
        "installed {UNIT} and {}\n{registered}\
         The root service captures the lock screen and the display-manager greeter over X11; a Wayland \
         greeter or session is not visible until it runs an X11 session. On a headless server, metrics, \
         exec, shell and power work without a display.\n",
        layout.exe().display(),
    )?;
    if !rcli_agent::secret_store::systemd_creds_available() {
        writeln!(
            out,
            "warning: systemd-creds is unavailable, so the Device Password stays in the root-only encrypted file {}",
            layout.state_dir().display()
        )?;
    }
    Ok(())
}

/// `sudo rcli uninstall`: removes the Device from the Server (see `install::unregister`), then stops and removes the
/// service and deletes the Device's local data and the machine-wide Controller configuration.
pub async fn uninstall(out: &mut dyn std::io::Write) -> anyhow::Result<()> {
    anyhow::ensure!(crate::upgrade::is_root(), "run it as root: sudo rcli uninstall");
    uninstall_at(&Layout { root: "/".into() }, &mut system, out).await
}

pub async fn uninstall_at(layout: &Layout, run: Run<'_>, out: &mut dyn std::io::Write) -> anyhow::Result<()> {
    crate::install::unregister(recorded_server(layout), &layout.state_dir(), out).await?;
    uninstall_files(layout, run)?;
    writeln!(out, "removed {UNIT}, {} and the Device's local data", layout.exe().display())?;
    Ok(())
}

/// `rcli service install` as root (ticket 15): moves `user`'s User Mode install to the root service, keeping the
/// Device ID, Device Password and Device Settings. `owner` (None in the `--root` test seam) is the account whose
/// systemd instance runs the User Mode Agent.
pub async fn to_service_at(
    user: &UserLayout,
    owner: Option<&str>,
    layout: &Layout,
    name: String,
    platform: &dyn rcli_agent::platform::Platform,
    run: Run<'_>,
    out: &mut dyn std::io::Write,
) -> anyhow::Result<()> {
    let opts = crate::install::hand_to_service(&user.state_dir(), &layout.state_dir(), name)?;
    install_at(layout, opts, platform, &mut *run, out).await?;
    // Root reaches the user's systemd instance with `--machine <user>@`.
    let machine = owner.map(|o| format!("{o}@"));
    let mut as_owner = |program: &str, args: &[&str]| match (&machine, args.split_first()) {
        (Some(m), Some((&"--user", rest))) => run(program, &[&["--user", "--machine", m.as_str()], rest].concat()),
        _ => run(program, args),
    };
    uninstall_user_files(user, &mut as_owner)?;
    writeln!(out, "Service Mode: the User Mode install in {} is removed", user.home.display())?;
    Ok(())
}

/// `rcli service uninstall` as root: hands the root service's Device back to `user`'s User Mode, starts that as
/// `owner` (in the `--root` test seam, None: in-process with `platform`), then removes the root service.
pub async fn to_user_at(
    layout: &Layout,
    user: &UserLayout,
    owner: Option<&crate::install::Owner>,
    name: String,
    platform: &dyn rcli_agent::platform::Platform,
    run: Run<'_>,
    out: &mut dyn std::io::Write,
) -> anyhow::Result<()> {
    let (identity, opts) = crate::install::hand_to_user_opts(&layout.state_dir(), &layout.controller_config(), &user.state_dir(), name)?;
    match owner {
        Some(owner) => {
            owner.give(&user.state_dir())?;
            // `rcli install` as the user: their files, their systemd instance, the Server just handed over.
            let runtime = format!("XDG_RUNTIME_DIR=/run/user/{}", owner.uid);
            let exe = std::env::current_exe()?.to_string_lossy().into_owned();
            let args = ["-H", "-u", &owner.name, "env", &runtime, &exe, "install", "--name", &opts.name, "--terminal-user", &opts.terminal_user];
            run("sudo", &args)?;
        }
        None => install_user_at(user, opts, platform, &mut *run, &mut std::io::sink()).await?,
    }
    uninstall_files(layout, run)?;
    write!(out, "User Mode: the root service is removed\n{identity}")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rcli_agent::settings::{DeviceSettings, RootAccess};

    fn temp_root(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rcli-linux-install-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn opts() -> Options {
        Options {
            server: "rcli.example.com:7443".into(),
            key: "rk_ab12_s3cr&t".into(),
            name: "linux-box".into(),
            terminal_user: "alice".into(),
            allow_root: true,
        }
    }

    fn recorder(calls: &mut Vec<String>) -> impl FnMut(&str, &[&str]) -> anyhow::Result<()> + '_ {
        move |program: &str, args: &[&str]| {
            calls.push(format!("{program} {}", args.join(" ")));
            Ok(())
        }
    }

    /// Ticket 12: User Mode needs no root. Everything lands in the home, the user unit starts at login, a Server is
    /// optional, and uninstall leaves nothing behind.
    #[tokio::test]
    async fn a_user_mode_install_with_no_server_lives_in_the_home_and_uninstall_removes_it() {
        use std::os::unix::fs::PermissionsExt;
        let home = temp_root("user-mode");
        let layout = UserLayout { home: home.clone(), config_dir: home.join(".config/rcli") };
        let opts = Options { server: String::new(), key: String::new(), ..opts() };
        let platform = rcli_agent::platform::FakePlatform::new("linux-box");
        let (mut calls, mut out) = (vec![], vec![]);

        install_user_at(&layout, opts, &platform, &mut recorder(&mut calls), &mut out).await.unwrap();

        let out = String::from_utf8(out).unwrap();
        assert!(out.contains("No Server yet") && !out.contains("Device ID:"), "{out}");
        assert!(home.join(".local/lib/rcli/rcli").is_file());
        assert_eq!(std::fs::read_link(home.join(".local/bin/rcli")).unwrap(), layout.exe());
        assert_eq!(std::fs::metadata(layout.state_dir()).unwrap().permissions().mode() & 0o777, 0o700);
        assert!(!layout.state_dir().join("server.json").exists(), "no Server was given");
        let unit = std::fs::read_to_string(home.join(".config/systemd/user/rcli-agent.service")).unwrap();
        let start = format!(
            "ExecStart={} agent run --state-dir {} --name linux-box --socket %t/rcli/agent.sock\n",
            layout.exe().display(),
            layout.state_dir().display()
        );
        assert!(unit.contains(&start) && unit.contains("WantedBy=default.target") && !unit.contains("User="), "{unit}");
        assert!(home.join(".local/share/applications/rcli.desktop").is_file());
        assert!(home.join(".local/share/icons/hicolor/48x48/apps/rcli.png").is_file());
        assert_eq!(calls, ["systemctl --user daemon-reload", "systemctl --user enable rcli-agent.service", "systemctl --user restart rcli-agent.service"]);

        calls.clear();
        uninstall_user_at(&layout, &mut recorder(&mut calls), &mut vec![]).await.unwrap();
        assert_eq!(calls, ["systemctl --user disable --now rcli-agent.service", "systemctl --user daemon-reload"]);
        for gone in [".local/lib/rcli", ".local/bin/rcli", ".local/state/rcli", ".config/systemd/user/rcli-agent.service", ".local/share/applications/rcli.desktop", ".local/share/icons/hicolor/48x48/apps/rcli.png"] {
            assert!(!home.join(gone).exists(), "{gone} is left behind");
        }
        let _ = std::fs::remove_dir_all(home);
    }

    /// A Server given at install is saved for the Agent, private to the user: it holds the Server Key.
    #[test]
    fn a_user_mode_install_keeps_the_server_key_off_the_unit_and_private() {
        use std::os::unix::fs::PermissionsExt;
        let home = temp_root("user-mode-server");
        let layout = UserLayout { home: home.clone(), config_dir: home.join(".config/rcli") };
        let exe = home.join("download/rcli");
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        std::fs::write(&exe, b"binary").unwrap();

        install_user_files(&layout, &opts(), &exe).unwrap();

        let saved = rcli_agent::ipc::saved_server(&layout.state_dir()).unwrap();
        assert_eq!((saved.addr.as_str(), saved.server_key.as_str()), ("rcli.example.com:7443", "rk_ab12_s3cr&t"));
        let mode = std::fs::metadata(layout.state_dir().join("server.json")).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        assert!(!user_unit(&layout, &opts()).contains("rk_"));
        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn install_puts_the_binary_unit_key_file_and_settings_in_place_and_uninstall_removes_them() {
        use std::os::unix::fs::PermissionsExt;
        let root = temp_root("roundtrip");
        let layout = Layout { root: root.clone() };
        let exe = root.join("download/rcli");
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        std::fs::write(&exe, b"binary").unwrap();
        let mut calls = vec![];

        install_files(&layout, &opts(), &exe, &mut recorder(&mut calls)).unwrap();
        install_service(&layout, &opts(), &mut recorder(&mut calls)).unwrap();

        assert_eq!(std::fs::read(root.join("usr/local/lib/rcli/rcli")).unwrap(), b"binary");
        assert_eq!(std::fs::read_link(root.join("usr/local/bin/rcli")).unwrap(), layout.exe());
        let settings = DeviceSettings::load(&layout.state_dir());
        assert_eq!((settings.terminal_user.as_str(), settings.root), ("alice", RootAccess::Always));
        assert_eq!(std::fs::metadata(layout.state_dir()).unwrap().permissions().mode() & 0o777, 0o700);
        assert_eq!(std::fs::metadata(layout.env_file()).unwrap().permissions().mode() & 0o777, 0o600, "it holds the Server Key");
        assert_eq!(std::fs::metadata(layout.install_file()).unwrap().permissions().mode() & 0o777, 0o600);
        // The app menu launches the window with no arguments; `rcli install` puts the entry there.
        let desktop = std::fs::read_to_string(layout.desktop_file()).unwrap();
        assert!(desktop.contains("Exec=/usr/local/bin/rcli\n"), "{desktop}");
        assert!(desktop.contains("[Desktop Entry]") && desktop.contains("Terminal=false"), "{desktop}");
        // Ticket 12: the entry names the rcli icon, which `rcli install` puts in the hicolor theme.
        assert!(desktop.contains("Icon=rcli\n"), "{desktop}");
        assert!(root.join("usr/share/icons/hicolor/256x256/apps/rcli.png").is_file());
        assert_eq!(std::fs::metadata(layout.desktop_file()).unwrap().permissions().mode() & 0o777, 0o644);
        let env = std::fs::read_to_string(layout.env_file()).unwrap();
        assert_eq!(env, "RCLI_SERVER_KEY=rk_ab12_s3cr&t\n");
        let unit = std::fs::read_to_string(layout.unit()).unwrap();
        assert!(unit.contains(&format!("EnvironmentFile={}", layout.env_file().display())), "{unit}");
        assert!(unit.contains(&format!("ExecStart={} agent run", layout.exe().display())), "{unit}");
        assert!(unit.contains(&format!("--session-socket {}", layout.session_socket().display())), "{unit}");
        assert!(unit.contains("Restart=always") && unit.contains("WantedBy=multi-user.target"), "{unit}");
        assert_eq!(recorded_server(&layout).as_deref(), Some("rcli.example.com:7443"));
        assert_eq!(
            calls,
            vec![
                format!("gtk-update-icon-cache -q {}", root.join("usr/share/icons/hicolor").display()),
                "systemctl daemon-reload".to_owned(),
                "systemctl enable rcli-agent.service".to_owned(),
                "systemctl restart rcli-agent.service".to_owned()
            ]
        );

        calls.clear();
        uninstall_files(&layout, &mut recorder(&mut calls)).unwrap();
        assert_eq!(
            calls,
            vec!["systemctl disable --now rcli-agent.service".to_owned(), "systemctl daemon-reload".to_owned()]
        );
        for gone in [
            "usr/local/lib/rcli/rcli",
            "usr/local/bin/rcli",
            "var/lib/rcli",
            "etc/systemd/system/rcli-agent.service",
            "etc/rcli/agent.env",
            "usr/share/applications/rcli.desktop",
            "usr/share/icons/hicolor/48x48/apps/rcli.png",
            "usr/share/icons/hicolor/256x256/apps/rcli.png",
        ] {
            assert!(!root.join(gone).exists(), "{gone} is left behind");
        }
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn the_unit_runs_the_agent_as_root_with_the_key_out_of_its_arguments() {
        let layout = Layout { root: "/".into() };
        let unit = unit(&layout, &opts());
        let expected = "ExecStart=/usr/local/lib/rcli/rcli agent run --server rcli.example.com:7443 \
                        --state-dir /var/lib/rcli --name linux-box --session-socket /run/rcli/session.sock\n";
        assert!(unit.contains(expected), "{unit}");
        assert!(!unit.contains("rk_"), "the Server Key is never on a command line: {unit}");
        assert!(unit.contains("StateDirectoryMode=0700"), "{unit}");
    }

    #[test]
    fn a_name_with_spaces_and_specifiers_is_quoted_and_escaped() {
        let layout = Layout { root: "/".into() };
        let mut o = opts();
        o.name = "bench %1 \"box\"".into();
        let unit = unit(&layout, &o);
        assert!(unit.contains("--name \"bench %%1 \\\"box\\\"\" --session-socket"), "{unit}");
    }
}
