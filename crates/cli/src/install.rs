//! Small helpers shared by the per-OS `rcli install` implementations: the options every install takes, the
//! command seam, the rooted path, the file writer, the Device Settings an install records, the Terminal User
//! fallback, and the registration an install starts with and an uninstall undoes.

use std::path::{Path, PathBuf};

use rcli_agent::settings::{DeviceSettings, RootAccess};

/// Runs an external program (`systemctl`, `launchctl`, `sc.exe`, `icacls.exe`, `ditto`); a seam for tests.
pub type Run<'a> = &'a mut dyn FnMut(&str, &[&str]) -> anyhow::Result<()>;

/// Runs `program` for real; the command must succeed.
pub fn system(program: &str, args: &[&str]) -> anyhow::Result<()> {
    let out = std::process::Command::new(program).args(args).output()?;
    anyhow::ensure!(out.status.success(), "{program} {} failed: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim());
    Ok(())
}

/// What `rcli install` was given, on every platform.
pub struct Options {
    pub server: String,
    pub key: String,
    pub name: String,
    /// The Terminal User, already resolved (`--terminal-user`, else the installing or console account).
    pub terminal_user: String,
    pub allow_root: bool,
}

/// The Terminal User an install records when `--terminal-user` is absent: the account that ran `sudo`, else the
/// account running the install. `root` is not a Terminal User when a person is available.
pub fn terminal_user(flag: Option<String>) -> String {
    flag.or_else(|| std::env::var("SUDO_USER").ok())
        .filter(|u| !u.trim().is_empty() && u != "root")
        .unwrap_or_else(rcli_agent::settings::default_terminal_user)
}

/// `path` under a `root`-prefixed layout, so tests point every installed path at a temp directory.
pub fn at(root: &Path, path: &str) -> PathBuf {
    root.join(path.trim_start_matches('/'))
}

/// Writes `contents` to `path` with `mode`, creating the parent directory.
#[cfg(unix)]
pub fn write(path: &Path, contents: impl AsRef<[u8]>, mode: u32) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, contents)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))?;
    Ok(())
}

/// Records the Device Settings an install was asked for: the Terminal User, and root always with `--allow-root`.
pub fn save_settings(state: &Path, opts: &Options) -> anyhow::Result<()> {
    let mut settings = DeviceSettings::load(state);
    settings.terminal_user = opts.terminal_user.clone();
    if opts.allow_root {
        settings.root = RootAccess::Always;
    }
    settings.save(state)
}

/// This Device's credentials as the Agent saved them in `state`, if it is registered.
pub fn device_credentials(state: &Path) -> Option<rcli_agent::Credentials> {
    serde_json::from_slice(&std::fs::read(state.join("credentials.json")).ok()?).ok()
}

/// An install's first step: checks the Server Key with the Server, so a wrong one fails before anything is written
/// (on a reinstall, before the working install is touched).
pub async fn check_server_key(opts: &Options) -> anyhow::Result<()> {
    let server = rcli_agent::ServerAccess { addr: opts.server.clone(), server_key: opts.key.clone() };
    rcli_agent::check_server_key(&server).await.map_err(|e| e.context(REGISTER_FAILED))
}

pub const REGISTER_FAILED: &str = "cannot register this Device; is the Server Address and Server Key right?";

/// What every User Mode `rcli install` does around its OS's own files (`files`): takes the Server saved in `state`
/// (a reinstall keeps it), checks the Server Key before anything is written, then registers the Device; if that fails
/// on a first install, `remove` takes the files away again. Without a Server the Agent serves only local control
/// until one is set. Returns the options in effect and the lines to print.
pub async fn user_install(
    state: &Path,
    controller_config: &Path,
    opts: Options,
    platform: &dyn rcli_agent::platform::Platform,
    run: Run<'_>,
    files: impl FnOnce(&Options, Run<'_>) -> anyhow::Result<()>,
    remove: impl FnOnce(Run<'_>),
) -> anyhow::Result<(Options, String)> {
    let opts = with_saved_server(state, opts);
    let with_server = !opts.server.is_empty();
    if with_server {
        check_server_key(&opts).await?;
    }
    let reinstall = state.join("credentials.json").exists();
    files(&opts, &mut *run)?;
    if !with_server {
        let text = "No Server yet: this Device serves only local control. It gets its Device ID and Device Password \
                    once you run `rcli agent server set <Server Address> --key <Server Key>`.\n";
        return Ok((opts, text.to_owned()));
    }
    match register(&opts, platform, state, controller_config).await {
        Ok(registered) => {
            // Per user, and it holds the Server Key: this user's only.
            #[cfg(unix)]
            std::fs::set_permissions(controller_config, std::os::unix::fs::PermissionsExt::from_mode(0o600))?;
            Ok((opts, registered))
        }
        Err(e) => {
            if !reinstall {
                remove(run);
            }
            Err(e.context(REGISTER_FAILED))
        }
    }
}

/// `rcli service uninstall`: hands the Service Mode install (`service_state`, its Controller configuration
/// `machine`) to the User Mode `user_state` (see `hand_to_user`), and returns the lines to print and the options a
/// User Mode install of it takes: the Device Settings' Terminal User, the Server it was just given.
pub fn hand_to_user_opts(service_state: &Path, machine: &Path, user_state: &Path, name: String) -> anyhow::Result<(String, Options)> {
    let identity = hand_to_user(service_state, machine, user_state)?;
    let terminal_user = DeviceSettings::load(user_state).terminal_user;
    Ok((identity, Options { server: String::new(), key: String::new(), name, terminal_user, allow_root: false }))
}

/// Registers this Device with the Server the install was given (or keeps its existing registration), so a wrong
/// Server Key fails before the service starts, then writes the machine-wide Controller configuration at `machine`
/// (ADR 0006). Returns the Device ID and Device Password lines the install prints.
pub async fn register(
    opts: &Options,
    platform: &dyn rcli_agent::platform::Platform,
    state: &Path,
    machine: &Path,
) -> anyhow::Result<String> {
    let server = rcli_agent::ServerAccess { addr: opts.server.clone(), server_key: opts.key.clone() };
    let secrets = crate::agent::secret_store(state);
    let creds = crate::agent::load_or_register(&server, platform, state, secrets.as_ref()).await?;
    let password = secrets.get(rcli_agent::ipc::DEVICE_PASSWORD)?.unwrap_or_default();
    // A reinstall keeps its saved registration, which does not carry the API Address: keep the one already known.
    let known = crate::config::load_machine(machine).ok().flatten().map(|m| m.api_address).unwrap_or_default();
    let api_address = if creds.api_address.is_empty() { known } else { creds.api_address.clone() };
    let config = crate::config::Machine {
        server: opts.server.clone(),
        server_key: opts.key.clone(),
        api_address,
        // `load_or_register` checked it against the Server Key's fingerprint.
        ca_pem: rcli_agent::cert_pem(&creds.ca_der),
    };
    crate::config::save_machine(machine, &config)?;
    let mut text = format!("Device ID: {}\nDevice Password: {password}\n", creds.device_id);
    if config.api_address.is_empty() {
        text += "The Server has no API Address yet, so `rcli connect` and `rcli mcp` wait until an Admin opens the \
                 Dashboard over https:// once.\n";
    }
    Ok(text)
}

/// The Device's state directory under `root` (the `--root` test seam): credentials, Device Settings, Secret Store.
pub fn state_dir(root: &Path) -> PathBuf {
    #[cfg(target_os = "macos")]
    // The seam installs macOS in User Mode, with `root` as the home.
    return crate::macos_install::UserLayout { home: root.into(), config_dir: root.into() }.state_dir();
    #[cfg(target_os = "linux")]
    // The seam installs Linux in User Mode, with `root` as the home.
    return crate::linux_install::UserLayout { home: root.into(), config_dir: root.into() }.state_dir();
    #[cfg(windows)]
    return crate::windows_install::Layout::under(root).state_dir();
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    root.join("rcli")
}

/// An uninstall's first steps: removes the Device from the Server at `server` (the Server Address the install
/// recorded), then deletes its secrets. An unreachable Server is a warning, not a failure: the local install must
/// still be removable, and an Admin can delete the Device from the Dashboard.
pub async fn unregister(server: Option<String>, state: &Path, out: &mut dyn std::io::Write) -> anyhow::Result<()> {
    if let (Some(server), Some(creds)) = (server, device_credentials(state)) {
        if let Err(e) = rcli_agent::unregister_at(&server, &creds).await {
            writeln!(out, "warning: could not remove the Device from the Server: {e:#}")?;
        }
    }
    let secrets = crate::agent::secret_store(state);
    if let Err(e) = secrets.remove(rcli_agent::ipc::DEVICE_PASSWORD) {
        writeln!(out, "warning: {e:#}")?;
    }
    Ok(())
}

/// A User Mode reinstall given no Server keeps the one saved in `state` (the one `rcli service uninstall` hands over).
pub fn with_saved_server(state: &Path, opts: Options) -> Options {
    match rcli_agent::ipc::saved_server(state) {
        Some(saved) if opts.server.is_empty() => Options { server: saved.addr, key: saved.server_key, ..opts },
        _ => opts,
    }
}

/// The Secret Store of a User Mode install in `state`, whoever opens it: its user, or root switching Install Mode.
pub fn user_secret_store(state: &Path) -> Box<dyn rcli_agent::secret_store::SecretStore> {
    #[cfg(windows)]
    return Box::new(rcli_agent::secret_store::DpapiSecretStore::new(state.join("secrets"), false));
    #[cfg(not(windows))]
    Box::new(rcli_agent::secret_store::FileSecretStore::new(state))
}

/// Switching Install Mode (ticket 15): copies the Device's credentials, Device Settings and Device Password from one
/// Install Mode's state dir to the other's, so the Device ID and Device Password stay. Returns the lines to print.
fn hand_over(
    from: &Path,
    from_secrets: &dyn rcli_agent::secret_store::SecretStore,
    to: &Path,
    to_secrets: &dyn rcli_agent::secret_store::SecretStore,
) -> anyhow::Result<String> {
    let creds = device_credentials(from).ok_or_else(|| anyhow::anyhow!("this Device has no Device ID yet: set a Server first"))?;
    std::fs::create_dir_all(to)?;
    #[cfg(unix)]
    std::fs::set_permissions(to, std::os::unix::fs::PermissionsExt::from_mode(0o700))?;
    for file in ["credentials.json", "settings.json"] {
        if from.join(file).exists() {
            std::fs::copy(from.join(file), to.join(file))?;
        }
    }
    let password = from_secrets.get(rcli_agent::ipc::DEVICE_PASSWORD)?.unwrap_or_default();
    to_secrets.set(rcli_agent::ipc::DEVICE_PASSWORD, &password)?;
    Ok(format!("Device ID: {}\nDevice Password: {password}\n", creds.device_id))
}

/// `rcli service install`: hands the User Mode install in `user_state` to Service Mode's `service_state` and returns
/// the options its install takes: the Server the Device has and the Terminal User it records.
pub fn hand_to_service(user_state: &Path, service_state: &Path, name: String) -> anyhow::Result<Options> {
    let server = rcli_agent::ipc::saved_server(user_state).ok_or_else(|| {
        anyhow::anyhow!("no User Mode install with a Server here; Service Mode needs one: `rcli install --server … --key …`")
    })?;
    hand_over(user_state, &*user_secret_store(user_state), service_state, &*crate::agent::secret_store(service_state))?;
    let terminal_user = DeviceSettings::load(service_state).terminal_user;
    Ok(Options { server: server.addr, key: server.server_key, name, terminal_user, allow_root: false })
}

/// `rcli service uninstall`: hands the Service Mode install (`service_state`, its Controller configuration `machine`)
/// to the User Mode `user_state`, with the Server saved for the User Mode Agent. Returns the lines to print.
pub fn hand_to_user(service_state: &Path, machine: &Path, user_state: &Path) -> anyhow::Result<String> {
    let machine = crate::config::load_machine(machine)?.ok_or_else(|| anyhow::anyhow!("this Device is not in Service Mode"))?;
    let text = hand_over(service_state, &*crate::agent::secret_store(service_state), user_state, &*user_secret_store(user_state))?;
    let server = serde_json::json!({ "server": machine.server, "key": machine.server_key }).to_string();
    let path = user_state.join(rcli_agent::ipc::SERVER_FILE);
    #[cfg(unix)]
    write(&path, &server, 0o600)?;
    #[cfg(windows)]
    std::fs::write(&path, server)?;
    Ok(text)
}

/// The account a switch run through `sudo` moves the install from or to.
#[cfg(unix)]
pub struct Owner {
    pub name: String,
    pub home: PathBuf,
    pub uid: u32,
    pub gid: u32,
}

#[cfg(unix)]
impl Owner {
    /// The account that ran `sudo` (`SUDO_USER`), its home from the user database.
    pub fn from_sudo() -> anyhow::Result<Self> {
        use std::os::unix::fs::MetadataExt;
        let name = std::env::var("SUDO_USER")
            .ok()
            .filter(|u| !u.is_empty() && u != "root")
            .ok_or_else(|| anyhow::anyhow!("run it with sudo from the account whose install it switches: sudo rcli service …"))?;
        #[cfg(target_os = "macos")]
        let out = std::process::Command::new("/usr/bin/dscl").args([".", "-read", &format!("/Users/{name}"), "NFSHomeDirectory"]).output()?;
        #[cfg(not(target_os = "macos"))]
        let out = std::process::Command::new("getent").args(["passwd", &name]).output()?;
        let text = String::from_utf8_lossy(&out.stdout);
        #[cfg(target_os = "macos")]
        let home = text.trim().strip_prefix("NFSHomeDirectory:").map(str::trim);
        #[cfg(not(target_os = "macos"))]
        let home = text.trim().split(':').nth(5);
        let home = PathBuf::from(home.filter(|h| !h.is_empty()).ok_or_else(|| anyhow::anyhow!("no home directory for {name}"))?);
        let meta = std::fs::metadata(&home)?;
        Ok(Self { name, uid: meta.uid(), gid: meta.gid(), home })
    }

    /// Gives `dir` (and everything in it, and the directories above it that root created under the home) to this
    /// account: root wrote it for them.
    pub fn give(&self, dir: &Path) -> anyhow::Result<()> {
        fn all(path: &Path, uid: u32, gid: u32) -> std::io::Result<()> {
            std::os::unix::fs::lchown(path, Some(uid), Some(gid))?;
            if std::fs::symlink_metadata(path)?.is_dir() {
                for entry in std::fs::read_dir(path)? {
                    all(&entry?.path(), uid, gid)?;
                }
            }
            Ok(())
        }
        all(dir, self.uid, self.gid)?;
        for parent in dir.ancestors().skip(1).take_while(|p| *p != self.home && p.starts_with(&self.home)) {
            std::os::unix::fs::lchown(parent, Some(self.uid), Some(self.gid))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_terminal_user_is_the_flag_first_then_the_installing_account() {
        assert_eq!(terminal_user(Some("alice".into())), "alice");
        // Without the flag: the account that ran `sudo` when there is one, else this process's account.
        let from_sudo = std::env::var("SUDO_USER").ok().filter(|u| !u.trim().is_empty() && u != "root");
        match from_sudo {
            Some(user) => assert_eq!(terminal_user(None), user),
            None => assert_eq!(terminal_user(None), rcli_agent::settings::default_terminal_user()),
        }
        // An explicit root is not a Terminal User; the fallback decides, so it equals the no-flag answer (which is
        // still "root" when root is the account running the install, as in a container).
        assert_eq!(terminal_user(Some("root".into())), terminal_user(None));
    }

    #[test]
    fn paths_hang_off_the_layout_root() {
        let root = Path::new("/tmp/rcli-root");
        assert_eq!(at(root, "/usr/local/lib/rcli/rcli"), root.join("usr/local/lib/rcli/rcli"));
        assert_eq!(at(root, "relative"), root.join("relative"));
    }
}
