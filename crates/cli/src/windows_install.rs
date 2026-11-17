//! `rcli install|uninstall` on Windows (ticket 22): `rcli.exe` in `%ProgramFiles%\rcli`, the Server Key in a
//! SYSTEM/Administrators-only config file (never on the service's command line, which `sc qc` and the registry show
//! to every user), the Device's data in `%ProgramData%\rcli`, and a SYSTEM service `rcli-agent`. The service itself
//! is hosted with the `windows-service` crate (`windows_service.rs`), so it reports SCM status instead of dying
//! with error 1053.
//!
//! Every path hangs off `Layout`, and every external command (`sc.exe`, `icacls.exe`) goes through `Run`, so the
//! layout, the service definition and the exact commands are tested on any host without touching a real Device.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub use crate::install::{Options, Run};

/// The SCM name of the service.
pub const SERVICE: &str = "rcli-agent";
/// What the Services console shows.
pub const DISPLAY_NAME: &str = "rcli Agent";
/// The named pipe the SYSTEM service accepts its session helpers on (`windows_session.rs` owns the name).
pub const PIPE: &str = rcli_agent::session::PIPE;
/// The Start Menu shortcut an install creates: running it opens the GUI window with no arguments (ticket 14).
pub const SHORTCUT: &str = "rcli.lnk";

/// Where an install puts things. Two roots, so a test can point both at a temp directory.
#[derive(Clone, Debug)]
pub struct Layout {
    pub program_files: PathBuf,
    pub program_data: PathBuf,
}

impl Layout {
    /// The real Device: `%ProgramFiles%` and `%ProgramData%` (never assumed to be on `C:`).
    #[cfg(windows)]
    pub fn real() -> Self {
        let dir = |name: &str, fallback: &str| std::env::var_os(name).map(PathBuf::from).unwrap_or_else(|| PathBuf::from(fallback));
        Self { program_files: dir("ProgramFiles", r"C:\Program Files"), program_data: dir("ProgramData", r"C:\ProgramData") }
    }

    /// Both roots under `root` (the `--root` test seam).
    pub fn under(root: &Path) -> Self {
        Self { program_files: root.join("Program Files"), program_data: root.join("ProgramData") }
    }

    pub fn exe(&self) -> PathBuf {
        self.program_files.join("rcli").join("rcli.exe")
    }

    /// The machine-wide Controller configuration; `share_controller_config` makes it readable by every local user.
    pub fn controller_config(&self) -> PathBuf {
        self.state_dir().join("controller.json")
    }

    /// The Device's credentials, Device Settings and Secret Store.
    pub fn state_dir(&self) -> PathBuf {
        self.program_data.join("rcli")
    }

    /// The service config: Server Address, Server Key, Device name, state directory and helper pipe.
    pub fn config(&self) -> PathBuf {
        self.state_dir().join("service.json")
    }

    pub fn session_pipe(&self) -> &'static str {
        PIPE
    }

    /// The all-users Start Menu shortcut: visible to every account, like the service itself.
    pub fn shortcut(&self) -> PathBuf {
        self.program_data.join("Microsoft").join("Windows").join("Start Menu").join("Programs").join(SHORTCUT)
    }
}

/// The protected file the service reads at start. The Server Key lives here, not in the service's arguments.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ServiceConfig {
    pub server: String,
    pub key: String,
    pub name: String,
    pub state_dir: PathBuf,
    pub pipe: String,
}

impl ServiceConfig {
    pub fn new(layout: &Layout, opts: &Options) -> Self {
        Self {
            server: opts.server.clone(),
            key: opts.key.clone(),
            name: opts.name.clone(),
            state_dir: layout.state_dir(),
            pipe: layout.session_pipe().into(),
        }
    }

    pub fn to_json(&self) -> anyhow::Result<String> {
        Ok(serde_json::to_string_pretty(self)?)
    }

    pub fn load(path: &Path) -> anyhow::Result<Self> {
        Ok(serde_json::from_slice(&std::fs::read(path)?)?)
    }
}

/// The arguments after `rcli.exe` that start the service host; the config path is not secret, the key is in the file.
pub fn service_args(layout: &Layout) -> Vec<String> {
    vec!["agent".into(), "service".into(), "--config".into(), layout.config().display().to_string()]
}

/// The service's binary path: the installed exe, quoted if it has spaces, then its arguments.
pub fn bin_path(layout: &Layout) -> String {
    let quote = |s: String| {
        if s.contains(' ') {
            format!("\"{s}\"")
        } else {
            s
        }
    };
    let args: Vec<String> = service_args(layout).into_iter().map(quote).collect();
    format!("{} {}", quote(layout.exe().display().to_string()), args.join(" "))
}

/// The SCM definition an install creates: a SYSTEM service, started automatically at boot.
#[derive(Clone, Debug, PartialEq)]
pub struct ServiceDefinition {
    pub name: &'static str,
    pub display_name: &'static str,
    pub executable: PathBuf,
    pub arguments: Vec<String>,
    pub account: &'static str,
    pub start: &'static str,
}

pub fn service_definition(layout: &Layout) -> ServiceDefinition {
    ServiceDefinition {
        name: SERVICE,
        display_name: DISPLAY_NAME,
        executable: layout.exe(),
        arguments: service_args(layout),
        account: "LocalSystem",
        start: "auto",
    }
}

/// Copies the running binary to `%ProgramFiles%\rcli`, writes the protected config, records the Device Settings and
/// creates the Start Menu shortcut.
/// `exe` is the binary running the install (a download in `%TEMP%`, or the installed one on a reinstall).
pub fn install_files(layout: &Layout, opts: &Options, exe: &Path, run: Run) -> anyhow::Result<()> {
    copy_binary(exe, &layout.exe())?;

    let state = layout.state_dir();
    std::fs::create_dir_all(&state)?;
    std::fs::write(layout.config(), ServiceConfig::new(layout, opts).to_json()?)?;
    // The config holds the Server Key and the registry copy of the binary path is world-readable: only SYSTEM and
    // Administrators may read it.
    restrict(run, &layout.config())?;

    crate::install::save_settings(&state, opts)?;
    // Everything under %ProgramData%\rcli is the Device's private data, including the DPAPI blobs.
    restrict(run, &state)?;
    create_shortcut(layout, run);
    Ok(())
}

/// Copies `exe` (a download, or the installed one on a reinstall) to `installed`.
fn copy_binary(exe: &Path, installed: &Path) -> anyhow::Result<()> {
    let installed = installed.to_path_buf();
    if let Some(dir) = installed.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let same = std::fs::canonicalize(exe).ok().is_some_and(|e| std::fs::canonicalize(&installed).ok().is_some_and(|i| i == e));
    if !same {
        // A reinstall runs while the service still runs the installed exe: Windows refuses to overwrite a running
        // image but allows renaming it, so move it aside, write the new one, and put the old one back on failure.
        if installed.exists() {
            let old = installed.with_extension("old.exe");
            let _ = std::fs::remove_file(&old);
            std::fs::rename(&installed, &old)?;
            if let Err(e) = std::fs::copy(exe, &installed) {
                let _ = std::fs::rename(&old, &installed);
                return Err(e.into());
            }
        } else {
            std::fs::copy(exe, &installed)?;
        }
    }
    Ok(())
}

/// The PowerShell that creates the Start Menu shortcut through WScript.Shell. Paths are single-quoted for
/// `-Command` (a `'` would end the string, so it is doubled); the shortcut starts the installed exe with no
/// arguments, which opens the GUI window.
pub fn shortcut_script(layout: &Layout) -> String {
    let quote = |path: &Path| path.display().to_string().replace('\'', "''");
    let working_dir = layout.exe().parent().map(Path::to_path_buf).unwrap_or_default();
    format!(
        "$s=(New-Object -ComObject WScript.Shell).CreateShortcut('{}'); \
         $s.TargetPath='{}'; $s.WorkingDirectory='{}'; $s.Save()",
        quote(&layout.shortcut()),
        quote(&layout.exe()),
        quote(&working_dir),
    )
}

/// Creates the Start Menu shortcut, best effort: a locked-down Start Menu or a missing PowerShell must not fail an
/// otherwise good install.
pub fn create_shortcut(layout: &Layout, run: Run) {
    let script = shortcut_script(layout);
    let _ = run("powershell.exe", &["-NoProfile", "-NonInteractive", "-Command", &script]);
}

/// Restricts `path` to SYSTEM and the Administrators group, by well-known SID so a non-English Windows works too.
/// A directory passes the grant on to what is created inside it (the Device credentials and the DPAPI blobs).
pub fn restrict(run: Run, path: &Path) -> anyhow::Result<()> {
    let spec = |sid: &str| {
        if path.is_dir() {
            format!("*{sid}:(OI)(CI)F")
        } else {
            format!("*{sid}:F")
        }
    };
    let system = spec("S-1-5-18");
    let administrators = spec("S-1-5-32-544");
    run("icacls.exe", &[&path.display().to_string(), "/inheritance:r", "/grant:r", &system, &administrators])
}

/// The machine-wide Controller configuration sits in the restricted state directory: give it its own ACL, read-only
/// for the Users group. Users reach it by its full path (every account may traverse folders it cannot list), and the
/// Agent rewrites it in place, so the ACL stays.
pub fn share_controller_config(layout: &Layout, run: Run) -> anyhow::Result<()> {
    let path = layout.controller_config().display().to_string();
    run("icacls.exe", &[&path, "/inheritance:r", "/grant:r", "*S-1-5-18:F", "*S-1-5-32-544:F", "*S-1-5-32-545:R"])
}

/// Creates and starts the SYSTEM service; a reinstall replaces the old one (stopping it first is not an error if it
/// was not running).
pub fn install_service(layout: &Layout, run: Run) -> anyhow::Result<()> {
    // A reinstall: stop and delete whatever is there, then create it fresh so arguments and paths are current.
    let _ = run("sc.exe", &["stop", SERVICE]);
    let _ = run("sc.exe", &["delete", SERVICE]);
    let definition = service_definition(layout);
    run(
        "sc.exe",
        &[
            "create",
            definition.name,
            "binPath=",
            &bin_path(layout),
            "start=",
            definition.start,
            "DisplayName=",
            definition.display_name,
            "obj=",
            definition.account,
        ],
    )?;
    run("sc.exe", &["start", SERVICE])
}

/// Stops and deletes the service. Absent or already stopped is not an error: `sc.exe` says so and uninstall goes on.
pub fn remove_service(run: Run) {
    let _ = run("sc.exe", &["stop", SERVICE]);
    let _ = run("sc.exe", &["delete", SERVICE]);
}

/// Removes everything `install_files`/`install_service` wrote, including the Device's local data and secrets.
pub fn uninstall_files(layout: &Layout, run: Run) -> anyhow::Result<()> {
    remove_service(run);
    let _ = std::fs::remove_file(layout.shortcut());
    let state = layout.state_dir();
    if state.exists() {
        std::fs::remove_dir_all(&state)?;
    }
    let dir = layout.program_files.join("rcli");
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    Ok(())
}

/// The per-user Run key: what is listed there starts at this user's sign-in. Writing it needs no Administrator.
pub const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
pub const RUN_VALUE: &str = "rcli";

/// User Mode (ADR 0012): the install of one Windows account, under its profile. No Administrator anywhere.
#[derive(Clone, Debug)]
pub struct UserLayout {
    /// `%LOCALAPPDATA%`.
    pub local_app_data: PathBuf,
    /// The user's rcli config directory (`config::dir`), where their Controller configuration lives.
    pub config_dir: PathBuf,
    /// The account name: it names the Agent's pipe.
    pub user: String,
}

impl UserLayout {
    #[cfg(windows)]
    pub fn real() -> anyhow::Result<Self> {
        let local = std::env::var_os("LOCALAPPDATA").filter(|d| !d.is_empty()).ok_or_else(|| anyhow::anyhow!("LOCALAPPDATA is not set"))?;
        let user = std::env::var("USERNAME").ok().filter(|u| !u.is_empty()).ok_or_else(|| anyhow::anyhow!("USERNAME is not set"))?;
        Ok(Self { local_app_data: local.into(), config_dir: crate::config::dir(), user })
    }
    /// Where per-user programs go; `rcli update` replaces this file.
    pub fn exe(&self) -> PathBuf {
        self.local_app_data.join("Programs").join("rcli").join("rcli.exe")
    }
    /// Credentials, Device Settings, the Secret Store (DPAPI, this user's scope) and the saved Server.
    pub fn state_dir(&self) -> PathBuf {
        self.local_app_data.join("rcli")
    }
    /// This user's Controller configuration (ADR 0012: per user in User Mode).
    pub fn controller_config(&self) -> PathBuf {
        self.config_dir.join(crate::config::CONTROLLER_FILE)
    }
}

/// Double-quotes one argument for a Windows command line.
fn quoted(arg: &str) -> String {
    format!("\"{}\"", arg.replace('"', "\\\""))
}

/// What the Run key starts at sign-in. `conhost --headless` keeps the console program from opening a window. No
/// `--server`: the Agent reads the Server saved in its state dir, or waits for one.
// shortcut: `conhost --headless` needs Windows 10 1809 or later; start through a hidden-window launcher if older
// Windows must be supported.
pub fn run_line(layout: &UserLayout, opts: &Options) -> String {
    format!("conhost.exe --headless {} {}", quoted(&layout.exe().display().to_string()), agent_args(layout, opts))
}

fn agent_args(layout: &UserLayout, opts: &Options) -> String {
    let state = quoted(&layout.state_dir().display().to_string());
    let pipe = quoted(&rcli_agent::ipc::user_pipe(&layout.user).display().to_string());
    format!("agent run --state-dir {state} --name {} --socket {pipe}", quoted(&opts.name))
}

/// A string for PowerShell's single quotes.
fn ps_quoted(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// Stops this user's running Agent (a reinstall or uninstall replaces its exe), but not the process running this.
/// By directory: a reinstall has already moved the running image aside to `rcli.old.exe`.
fn stop_agent(layout: &UserLayout, run: Run) {
    let dir = layout.exe().parent().map(|d| d.display().to_string()).unwrap_or_default();
    let script = format!(
        "Get-Process | Where-Object {{ $_.Path -and (Split-Path $_.Path) -eq {} -and $_.Id -ne {} }} | Stop-Process -Force",
        ps_quoted(&dir),
        std::process::id()
    );
    let _ = run("powershell.exe", &["-NoProfile", "-NonInteractive", "-Command", &script]);
}

/// Copies the binary into the profile and records the Device Settings and the Server (if given).
pub fn install_user_files(layout: &UserLayout, opts: &Options, exe: &Path) -> anyhow::Result<()> {
    copy_binary(exe, &layout.exe())?;
    let state = layout.state_dir();
    std::fs::create_dir_all(&state)?;
    crate::install::save_settings(&state, opts)?;
    if !opts.server.is_empty() {
        // The profile is this user's only, so the Server Key in it is too.
        let server = serde_json::json!({ "server": opts.server, "key": opts.key }).to_string();
        std::fs::write(state.join(rcli_agent::ipc::SERVER_FILE), server)?;
    }
    Ok(())
}

/// Lists the Agent in the user's Run key, then (re)starts it now with no window.
pub fn install_user_service(layout: &UserLayout, opts: &Options, run: Run) -> anyhow::Result<()> {
    run("reg.exe", &["add", RUN_KEY, "/v", RUN_VALUE, "/t", "REG_SZ", "/d", &run_line(layout, opts), "/f"])?;
    stop_agent(layout, run);
    let rest = format!("--headless {} {}", quoted(&layout.exe().display().to_string()), agent_args(layout, opts));
    let start = format!("Start-Process -FilePath 'conhost.exe' -ArgumentList {}", ps_quoted(&rest));
    run("powershell.exe", &["-NoProfile", "-NonInteractive", "-Command", &start])
}

/// Removes the Run key entry, stops the Agent and deletes the binary and the Device's local data. The caller has
/// already removed the Device from the Server.
pub fn uninstall_user_files(layout: &UserLayout, run: Run) -> anyhow::Result<()> {
    let _ = run("reg.exe", &["delete", RUN_KEY, "/v", RUN_VALUE, "/f"]);
    stop_agent(layout, run);
    let _ = std::fs::remove_file(layout.controller_config());
    if layout.state_dir().exists() {
        std::fs::remove_dir_all(layout.state_dir())?;
    }
    // Best effort: an uninstall run from the installed exe cannot delete its own image.
    if let Some(dir) = layout.exe().parent() {
        let _ = std::fs::remove_dir_all(dir);
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
        "installed {} for this user (User Mode, starts at sign-in)\n{registered}\
         User Mode cannot reach the lock and login screens or UAC prompts, or run the terminal as another user or \
         SYSTEM; `rcli service install` switches to Service Mode for that.\n",
        layout.exe().display(),
    )?;
    Ok(())
}

/// `rcli uninstall` in User Mode: removes the Device from the Server it was given, then this user's install.
pub async fn uninstall_user_at(layout: &UserLayout, run: Run<'_>, out: &mut dyn std::io::Write) -> anyhow::Result<()> {
    let server = rcli_agent::ipc::saved_server(&layout.state_dir()).map(|s| s.addr);
    crate::install::unregister(server, &layout.state_dir(), out).await?;
    uninstall_user_files(layout, run)?;
    writeln!(out, "removed the Run key entry {RUN_VALUE}, {} and the Device's local data", layout.exe().display())?;
    Ok(())
}

/// `rcli install` from a normal (not elevated) shell: User Mode for this user.
#[cfg(windows)]
pub async fn install_user(opts: Options, out: &mut dyn std::io::Write) -> anyhow::Result<()> {
    let platform = rcli_agent::windows_platform::WindowsPlatform::new(opts.name.clone())?;
    install_user_at(&UserLayout::real()?, opts, &platform, &mut crate::install::system, out).await
}

#[cfg(windows)]
pub async fn uninstall_user(out: &mut dyn std::io::Write) -> anyhow::Result<()> {
    uninstall_user_at(&UserLayout::real()?, &mut crate::install::system, out).await
}

/// This Device's name, as the Device reports it: the same hostname probe as the Controller's name (S7).
#[cfg(windows)]
pub fn device_name() -> String {
    crate::config::hostname_or("windows")
}

/// `rcli install --server … --key …` from an elevated shell. Files, registration and the machine-wide Controller
/// configuration, then the service; a wrong Server Key fails here, before anything runs as SYSTEM, and the Device ID
/// and password are printed on success.
#[cfg(windows)]
pub async fn install(opts: Options, out: &mut dyn std::io::Write) -> anyhow::Result<()> {
    anyhow::ensure!(crate::upgrade::is_root(), "run it from an elevated shell (Run as administrator): rcli install --server … --key …");
    let platform = rcli_agent::windows_platform::WindowsPlatform::new(opts.name.clone())?;
    install_at(&Layout::real(), opts, &platform, &mut crate::install::system, out).await
}

/// `install` on `layout`, registering with `platform`'s inventory; the `--root` test seam calls it directly.
#[cfg(windows)]
pub async fn install_at(
    layout: &Layout,
    opts: Options,
    platform: &dyn rcli_agent::platform::Platform,
    run: Run<'_>,
    out: &mut dyn std::io::Write,
) -> anyhow::Result<()> {
    // A reinstall must not destroy a working install when the new Server Key is wrong.
    crate::install::check_server_key(&opts).await?;
    let reinstall = layout.config().exists();
    install_files(layout, &opts, &std::env::current_exe()?, run)?;
    // Register now, so the Device ID and password exist before the service starts and a wrong key is caught here.
    let registered = match crate::install::register(&opts, platform, &layout.state_dir(), &layout.controller_config()).await {
        Ok(registered) => registered,
        Err(e) => {
            // Leave no half-installed Device behind; on a reinstall keep the existing one and its credentials.
            if !reinstall {
                let _ = uninstall_files(layout, run);
            }
            return Err(e.context(crate::install::REGISTER_FAILED));
        }
    };
    if let Err(e) = share_controller_config(layout, run).and_then(|()| install_service(layout, run)) {
        if !reinstall {
            let _ = uninstall_files(layout, run);
        }
        return Err(e.context("cannot install the rcli-agent service"));
    }
    write!(
        out,
        "installed {} and started the service {SERVICE}\n{registered}\
         The service runs as SYSTEM and drives the screen in the active session (the lock and login screens and \
         UAC prompts too).\nNext: for Ctrl+Alt+Del, allow software SAS once (see https://rcli.eigentlab.com/en/usage/), then \
         `rcli act <Device ID> key ctrl+alt+del` from a Controller.\n",
        layout.exe().display(),
    )?;
    Ok(())
}

/// `rcli uninstall` from an elevated shell: removes the Device from the Server (see `install::unregister`), then
/// stops and deletes the service and deletes the local data, the machine-wide Controller configuration with it.
#[cfg(windows)]
pub async fn uninstall(out: &mut dyn std::io::Write) -> anyhow::Result<()> {
    anyhow::ensure!(crate::upgrade::is_root(), "run it from an elevated shell (Run as administrator): rcli uninstall");
    uninstall_at(&Layout::real(), &mut crate::install::system, out).await
}

#[cfg(windows)]
pub async fn uninstall_at(layout: &Layout, run: Run<'_>, out: &mut dyn std::io::Write) -> anyhow::Result<()> {
    let server = ServiceConfig::load(&layout.config()).ok().map(|config| config.server);
    crate::install::unregister(server, &layout.state_dir(), out).await?;
    uninstall_files(layout, run)?;
    writeln!(out, "stopped and deleted the service {SERVICE}; removed {} and the Device's local data", layout.exe().display())?;
    Ok(())
}

/// `rcli service install` elevated (ticket 15): moves this account's User Mode install to the SYSTEM service, keeping
/// the Device ID, Device Password and Device Settings. UAC elevates the same account, so its profile is `user`.
#[cfg(windows)]
pub async fn to_service_at(
    user: &UserLayout,
    layout: &Layout,
    name: String,
    platform: &dyn rcli_agent::platform::Platform,
    run: Run<'_>,
    out: &mut dyn std::io::Write,
) -> anyhow::Result<()> {
    let opts = crate::install::hand_to_service(&user.state_dir(), &layout.state_dir(), name)?;
    install_at(layout, opts, platform, &mut *run, out).await?;
    uninstall_user_files(user, run)?;
    writeln!(out, "Service Mode: the User Mode install of {} is removed", user.user)?;
    Ok(())
}

/// `rcli service uninstall` elevated: hands the SYSTEM service's Device back to this account's User Mode, then removes
/// the service. The User Mode Agent starts at the next sign-in: started from here it would run elevated.
#[cfg(windows)]
pub async fn to_user_at(
    layout: &Layout,
    user: &UserLayout,
    name: String,
    platform: &dyn rcli_agent::platform::Platform,
    run: Run<'_>,
    out: &mut dyn std::io::Write,
) -> anyhow::Result<()> {
    let (identity, opts) = crate::install::hand_to_user_opts(&layout.state_dir(), &layout.controller_config(), &user.state_dir(), name)?;
    let mut no_start = |program: &str, args: &[&str]| if program == "powershell.exe" { Ok(()) } else { run(program, args) };
    install_user_at(user, opts, platform, &mut no_start, &mut std::io::sink()).await?;
    uninstall_files(layout, run)?;
    write!(out, "User Mode: the service {SERVICE} is removed; rcli starts at your next sign-in\n{identity}")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rcli_agent::settings::{DeviceSettings, RootAccess};

    fn temp_layout(name: &str) -> (Layout, PathBuf) {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root =
            std::env::temp_dir().join(format!("rcli-win-install-{name}-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let layout = Layout::under(&root);
        (layout, root)
    }

    fn opts() -> Options {
        Options {
            server: "rcli.example.com:7443".into(),
            key: "rk_ab12_s3cr&t".into(),
            name: "dev-pc".into(),
            terminal_user: "alice".into(),
            allow_root: true,
        }
    }

    #[test]
    fn install_copies_the_binary_writes_the_protected_config_and_records_the_terminal_user() {
        let (layout, root) = temp_layout("files");
        let exe = root.join("download").join("rcli.exe");
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        std::fs::write(&exe, b"binary").unwrap();
        let mut calls: Vec<String> = vec![];
        let mut run = |program: &str, args: &[&str]| {
            calls.push(format!("{program} {}", args.join(" ")));
            Ok(())
        };
        install_files(&layout, &opts(), &exe, &mut run).unwrap();

        assert_eq!(std::fs::read(layout.exe()).unwrap(), b"binary");
        let config = ServiceConfig::load(&layout.config()).unwrap();
        assert_eq!(config.server, "rcli.example.com:7443");
        assert_eq!(config.key, "rk_ab12_s3cr&t");
        assert_eq!(config.state_dir, layout.state_dir());
        assert_eq!(config.pipe, PIPE);
        let settings = DeviceSettings::load(&layout.state_dir());
        assert_eq!((settings.terminal_user.as_str(), settings.root), ("alice", RootAccess::Always));
        // SYSTEM and Administrators only, by well-known SID (a German Windows has no group called Administrators).
        assert_eq!(
            calls,
            vec![
                format!("icacls.exe {} /inheritance:r /grant:r *S-1-5-18:F *S-1-5-32-544:F", layout.config().display()),
                format!("icacls.exe {} /inheritance:r /grant:r *S-1-5-18:(OI)(CI)F *S-1-5-32-544:(OI)(CI)F", layout.state_dir().display()),
                format!("powershell.exe -NoProfile -NonInteractive -Command {}", shortcut_script(&layout)),
            ]
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn the_start_menu_shortcut_opens_the_installed_binary_with_no_arguments() {
        let (layout, root) = temp_layout("shortcut");
        let script = shortcut_script(&layout);
        assert!(
            layout.shortcut().ends_with(Path::new("Microsoft/Windows/Start Menu/Programs").join(SHORTCUT)),
            "{}",
            layout.shortcut().display()
        );
        assert!(script.contains(&format!("TargetPath='{}'", layout.exe().display())), "{script}");
        assert!(script.contains(&layout.shortcut().display().to_string()), "{script}");
        assert!(!script.contains(" agent "), "the shortcut must open the window, not a subcommand: {script}");

        // A Start Menu that cannot be written (locked down, no PowerShell) is not an install failure.
        let download = root.join("download").join("rcli.exe");
        std::fs::create_dir_all(download.parent().unwrap()).unwrap();
        std::fs::write(&download, b"binary").unwrap();
        let failed = install_files(&layout, &opts(), &download, &mut |program, _args| {
            anyhow::ensure!(program != "powershell.exe", "powershell is unavailable");
            Ok(())
        });
        failed.unwrap();
        assert!(layout.exe().exists(), "the rest of the install still happened");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn the_service_is_system_started_at_boot_and_never_gets_the_server_key_on_its_command_line() {
        let (layout, root) = temp_layout("service");
        let definition = service_definition(&layout);
        assert_eq!(definition.name, "rcli-agent");
        assert_eq!(definition.account, "LocalSystem");
        assert_eq!(definition.start, "auto");
        assert_eq!(definition.executable, layout.program_files.join("rcli").join("rcli.exe"));
        assert_eq!(
            definition.arguments,
            vec!["agent".to_owned(), "service".to_owned(), "--config".to_owned(), layout.config().display().to_string()]
        );
        let bin = bin_path(&layout);
        assert!(bin.contains(&format!("\"{}\"", layout.exe().display())), "a path with spaces must be quoted: {bin}");
        assert!(!bin.contains("rk_"), "the Server Key must not be visible in `sc qc`: {bin}");

        let mut calls: Vec<String> = vec![];
        let mut run = |program: &str, args: &[&str]| {
            calls.push(format!("{program} {}", args.join(" ")));
            Ok(())
        };
        install_service(&layout, &mut run).unwrap();
        assert_eq!(
            calls,
            vec![
                "sc.exe stop rcli-agent".to_owned(),
                "sc.exe delete rcli-agent".to_owned(),
                format!("sc.exe create rcli-agent binPath= {} start= auto DisplayName= rcli Agent obj= LocalSystem", bin_path(&layout)),
                "sc.exe start rcli-agent".to_owned(),
            ]
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn uninstall_stops_the_service_and_removes_the_binary_the_data_and_the_secrets() {
        let (layout, root) = temp_layout("uninstall");
        let exe = root.join("rcli.exe");
        std::fs::write(&exe, b"binary").unwrap();
        let mut calls: Vec<String> = vec![];
        {
            let mut run = |program: &str, args: &[&str]| {
                calls.push(format!("{program} {}", args.join(" ")));
                Ok(())
            };
            install_files(&layout, &opts(), &exe, &mut run).unwrap();
        }
        std::fs::create_dir_all(layout.state_dir().join("secrets")).unwrap();
        std::fs::write(layout.state_dir().join("secrets").join("device_password"), b"sealed").unwrap();
        std::fs::create_dir_all(layout.shortcut().parent().unwrap()).unwrap();
        std::fs::write(layout.shortcut(), b"shortcut").unwrap();
        assert!(layout.exe().exists() && layout.state_dir().exists() && layout.shortcut().exists());

        calls.clear();
        {
            let mut run = |program: &str, args: &[&str]| {
                calls.push(format!("{program} {}", args.join(" ")));
                Ok(())
            };
            uninstall_files(&layout, &mut run).unwrap();
        }
        assert_eq!(calls, vec!["sc.exe stop rcli-agent".to_owned(), "sc.exe delete rcli-agent".to_owned()]);
        assert!(!layout.state_dir().exists() && !layout.program_files.join("rcli").exists(), "an install file is left behind");
        assert!(!layout.shortcut().exists(), "the Start Menu shortcut is left behind");
        let _ = std::fs::remove_dir_all(root);
    }

    /// Every local user reads the machine-wide Controller configuration; only SYSTEM and Administrators change it.
    #[test]
    fn the_controller_configuration_is_read_only_for_users() {
        let (layout, root) = temp_layout("controller-config");
        let mut calls: Vec<String> = vec![];
        share_controller_config(&layout, &mut |program: &str, args: &[&str]| {
            calls.push(format!("{program} {}", args.join(" ")));
            Ok(())
        })
        .unwrap();
        assert_eq!(
            calls,
            vec![format!(
                "icacls.exe {} /inheritance:r /grant:r *S-1-5-18:F *S-1-5-32-544:F *S-1-5-32-545:R",
                layout.controller_config().display()
            )]
        );
        let _ = std::fs::remove_dir_all(root);
    }

    /// Ticket 14: User Mode needs no Administrator. Everything lands in the user's profile, the user's Run key
    /// starts the Agent at sign-in on its own pipe, a Server is optional, and uninstall leaves nothing behind.
    #[tokio::test]
    async fn a_user_mode_install_with_no_server_lives_in_the_profile_and_uninstall_removes_it() {
        let (_, root) = temp_layout("user-mode");
        let layout = UserLayout { local_app_data: root.join("Local"), config_dir: root.join("Roaming").join("rcli"), user: "carol".into() };
        let opts = Options { server: String::new(), key: String::new(), ..opts() };
        let platform = rcli_agent::platform::FakePlatform::new("dev-pc");
        let mut calls: Vec<String> = vec![];
        let mut out = vec![];
        {
            let mut run = |program: &str, args: &[&str]| {
                calls.push(format!("{program} {}", args.join(" ")));
                Ok(())
            };
            install_user_at(&layout, opts, &platform, &mut run, &mut out).await.unwrap();
        }

        let out = String::from_utf8(out).unwrap();
        assert!(out.contains("No Server yet") && !out.contains("Device ID:"), "{out}");
        assert!(root.join(r"Local/Programs/rcli/rcli.exe").is_file());
        assert_eq!(DeviceSettings::load(&root.join("Local").join("rcli")).terminal_user, "alice");
        assert!(!layout.state_dir().join("server.json").exists(), "no Server was given");
        let line = format!(
            "conhost.exe --headless \"{}\" agent run --state-dir \"{}\" --name \"dev-pc\" --socket \"\\\\.\\pipe\\rcli-agent-carol\"",
            layout.exe().display(),
            layout.state_dir().display()
        );
        assert_eq!(calls[0], format!(r"reg.exe add HKCU\Software\Microsoft\Windows\CurrentVersion\Run /v rcli /t REG_SZ /d {line} /f"));
        assert!(calls[1].starts_with("powershell.exe") && calls[1].contains("Stop-Process"), "{calls:?}");
        assert!(calls[2].starts_with("powershell.exe") && calls[2].contains("Start-Process -FilePath 'conhost.exe'"), "{calls:?}");
        assert!(!calls.iter().any(|c| c.contains("sc.exe") || c.contains("icacls")), "nothing needs Administrator: {calls:?}");

        calls.clear();
        {
            let mut run = |program: &str, args: &[&str]| {
                calls.push(format!("{program} {}", args.join(" ")));
                Ok(())
            };
            uninstall_user_at(&layout, &mut run, &mut vec![]).await.unwrap();
        }
        assert_eq!(calls[0], r"reg.exe delete HKCU\Software\Microsoft\Windows\CurrentVersion\Run /v rcli /f");
        assert!(calls[1].contains("Stop-Process"), "{calls:?}");
        assert!(!layout.state_dir().exists() && !root.join(r"Local/Programs/rcli").exists(), "an install file is left behind");
        let _ = std::fs::remove_dir_all(root);
    }

    /// A Server given at install is saved for the Agent in the profile, never on the Run key's command line.
    #[test]
    fn a_user_mode_install_keeps_the_server_key_off_the_run_key() {
        let (_, root) = temp_layout("user-mode-server");
        let layout = UserLayout { local_app_data: root.join("Local"), config_dir: root.join("Roaming").join("rcli"), user: "carol".into() };
        let exe = root.join("download").join("rcli.exe");
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        std::fs::write(&exe, b"binary").unwrap();

        install_user_files(&layout, &opts(), &exe).unwrap();

        let saved = rcli_agent::ipc::saved_server(&layout.state_dir()).unwrap();
        assert_eq!((saved.addr.as_str(), saved.server_key.as_str()), ("rcli.example.com:7443", "rk_ab12_s3cr&t"));
        assert!(!run_line(&layout, &opts()).contains("rk_"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn the_service_config_round_trips_through_its_file() {
        let (layout, root) = temp_layout("config");
        std::fs::create_dir_all(layout.state_dir()).unwrap();
        let config = ServiceConfig::new(&layout, &opts());
        std::fs::write(layout.config(), config.to_json().unwrap()).unwrap();
        assert_eq!(ServiceConfig::load(&layout.config()).unwrap(), config);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_reinstall_moves_the_running_binary_aside_before_writing_the_new_one() {
        let (layout, root) = temp_layout("replace");
        std::fs::create_dir_all(layout.exe().parent().unwrap()).unwrap();
        std::fs::write(layout.exe(), b"running build").unwrap();
        let download = root.join("download").join("rcli.exe");
        std::fs::create_dir_all(download.parent().unwrap()).unwrap();
        std::fs::write(&download, b"new build").unwrap();
        let mut run = |_program: &str, _args: &[&str]| Ok(());
        install_files(&layout, &opts(), &download, &mut run).unwrap();
        assert_eq!(std::fs::read(layout.exe()).unwrap(), b"new build");
        assert_eq!(std::fs::read(layout.exe().with_extension("old.exe")).unwrap(), b"running build");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_reinstall_does_not_copy_the_installed_binary_onto_itself() {
        let (layout, root) = temp_layout("reinstall");
        std::fs::create_dir_all(layout.exe().parent().unwrap()).unwrap();
        std::fs::write(layout.exe(), b"already installed").unwrap();
        let mut calls: Vec<String> = vec![];
        let mut run = |program: &str, args: &[&str]| {
            calls.push(format!("{program} {}", args.join(" ")));
            Ok(())
        };
        install_files(&layout, &opts(), &layout.exe(), &mut run).unwrap();
        assert_eq!(std::fs::read(layout.exe()).unwrap(), b"already installed");
        let _ = std::fs::remove_dir_all(root);
    }
}
