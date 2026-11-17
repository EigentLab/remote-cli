//! Server: connects Controllers to Devices (ADR 0011): registration, the Controller API and Address Books.
//!
//! Layout: one module per server area (`registry`, `registration`, ...), the Agent
//! connection in `agent_link`, and HTTP routes per area under `api/`.

pub mod agent_link;
pub mod api;
pub mod auth;
pub mod clock;
pub mod control_lock;
pub mod metrics;
pub mod pki;
pub mod registration;
pub mod registry;
pub mod server_key;
pub mod settings;
pub mod shell;
pub mod storage;
pub mod updates;
pub mod wake;
pub mod master_key;

use std::{net::SocketAddr, sync::Arc};

use clock::Clock;
use sqlx::PgPool;
use tokio::task::JoinHandle;

#[derive(Clone, Debug)]
pub struct Config {
    pub database_url: String,
    /// Controller API (HTTP).
    pub http_addr: SocketAddr,
    /// Agent connections (TLS, mTLS once enrolled).
    pub agent_addr: SocketAddr,
    /// The Server Address the install gives: a start applies it when it changed since the last start.
    pub agent_hostnames: Vec<String>,
    /// First-run admin token: accepted only until an enabled admin user has a live session.
    pub bootstrap_admin_token: String,
    /// Seals the CA private key and TOTP secrets (`RCLI_MASTER_KEY`); None = they are stored in plaintext.
    pub master_key: Option<master_key::MasterKey>,
    /// STUN/TURN URLs handed to both WebRTC peers, e.g. `turn:vps.example.com:3478`. Empty = direct paths only.
    pub ice_urls: Vec<String>,
    /// coturn `static-auth-secret`: the Server mints short-lived TURN credentials from it.
    pub turn_secret: Option<String>,
    /// Reverse proxies in front of the Controller API (`RCLI_TRUSTED_PROXIES`): from these peers the Controller's
    /// IP is read from `X-Forwarded-For`, so the Device Password limiter sees the real client.
    pub trusted_proxies: Vec<std::net::IpAddr>,
    /// The file store holding Agent builds and install packages (`RCLI_S3_*`).
    pub s3: storage::S3Config,
    /// Where to write the Controller API's HTTPS certificate and key from the CA at start, for the proxy in front of
    /// it (`RCLI_HTTPS_CERT_FILE`, `RCLI_HTTPS_KEY_FILE`, ADR 0015). None = a proxy with a certificate of its own.
    pub https_files: Option<(std::path::PathBuf, std::path::PathBuf)>,
    /// The API Address the install's mode gives (`RCLI_API_ADDRESS`, empty = learned from a visit); a start applies
    /// it when it changed since the last start.
    pub api_address: String,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        let var = |k: &str, default: Option<&str>| -> anyhow::Result<String> {
            std::env::var(k).or_else(|_| default.map(str::to_owned).ok_or_else(|| anyhow::anyhow!("{k} is required")))
        };
        let s3 = ["RCLI_S3_ENDPOINT", "RCLI_S3_BUCKET", "RCLI_S3_ACCESS_KEY", "RCLI_S3_SECRET_KEY"]
            .map(|k| std::env::var(k).ok().filter(|v| !v.is_empty()).ok_or(k));
        let missing: Vec<&str> = s3.iter().filter_map(|v| v.as_ref().err().copied()).collect();
        anyhow::ensure!(missing.is_empty(), "the file storage settings are required; missing: {}", missing.join(", "));
        let [endpoint, bucket, access_key, secret_key] = s3.map(Result::unwrap);
        let https_files = match ["RCLI_HTTPS_CERT_FILE", "RCLI_HTTPS_KEY_FILE"].map(|k| std::env::var(k).ok().filter(|v| !v.is_empty())) {
            [Some(cert), Some(key)] => Some((cert.into(), key.into())),
            [None, None] => None,
            _ => anyhow::bail!("RCLI_HTTPS_CERT_FILE and RCLI_HTTPS_KEY_FILE go together"),
        };
        Ok(Self {
            https_files,
            api_address: var("RCLI_API_ADDRESS", Some(""))?.trim().to_owned(),
            s3: storage::S3Config { endpoint, bucket, access_key, secret_key },
            database_url: var("DATABASE_URL", None)?,
            http_addr: var("RCLI_HTTP_ADDR", Some("0.0.0.0:8080"))?.parse()?,
            agent_addr: var("RCLI_AGENT_ADDR", Some("0.0.0.0:8443"))?.parse()?,
            agent_hostnames: var("RCLI_AGENT_HOSTNAMES", Some("localhost"))?
                .split(',')
                .map(|s| s.trim().to_owned())
                .collect(),
            bootstrap_admin_token: var("RCLI_BOOTSTRAP_ADMIN_TOKEN", None)?,
            master_key: std::env::var("RCLI_MASTER_KEY").ok().filter(|k| !k.is_empty()).map(|k| k.parse()).transpose()?,
            ice_urls: var("RCLI_ICE_URLS", Some(""))?.split(',').map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned).collect(),
            turn_secret: std::env::var("RCLI_TURN_SECRET").ok().filter(|s| !s.is_empty()),
            trusted_proxies: var("RCLI_TRUSTED_PROXIES", Some(""))?
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| s.parse().map_err(|e| anyhow::anyhow!("RCLI_TRUSTED_PROXIES: {s}: {e}")))
                .collect::<anyhow::Result<_>>()?,
        })
    }
}

/// Shared state handed to every area module.
pub struct AppState {
    pub db: PgPool,
    pub storage: storage::Storage,
    pub clock: Arc<dyn Clock>,
    pub ca: pki::Ca,
    /// The Server Key and Server Address, editable by Admins.
    pub settings: settings::Settings,
    pub config: Config,
    /// Rebuilt when the Server Address changes, so new connections get the new certificate.
    pub tls: tokio::sync::RwLock<tokio_rustls::TlsAcceptor>,
    pub connections: registry::Connections,
    pub inventories: metrics::OnlineInventories,
    pub agents: agent_link::Agents,
    pub shells: shell::Shells,
    pub lock_watchers: control_lock::Watchers,
    pub lock_released: control_lock::Released,
}

/// A running Server. Dropping it stops the listeners.
pub struct Server {
    pub http_addr: SocketAddr,
    pub agent_addr: SocketAddr,
    /// CA certificate Agents must trust to reach the Server.
    pub ca_pem: String,
    /// `rk_<CA fingerprint>_<secret>`.
    pub server_key: String,
    /// The running Server's state; the testkit reaches connected Agents through it.
    pub state: Arc<AppState>,
    tasks: Vec<JoinHandle<()>>,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.tasks.iter().for_each(JoinHandle::abort);
    }
}

pub async fn start(config: Config, clock: Arc<dyn Clock>) -> anyhow::Result<Server> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let api_address = match config.api_address.as_str() {
        "" => String::new(),
        given => api::settings::api_origin(given)
            .ok_or_else(|| anyhow::anyhow!("RCLI_API_ADDRESS={given}: {}", api::settings::API_ORIGIN))?,
    };
    let db = sqlx::postgres::PgPoolOptions::new().max_connections(10).connect(&config.database_url).await?;
    sqlx::migrate!("./migrations").run(&db).await?;
    let ca = pki::Ca::load_or_create(&db, config.master_key.as_ref()).await?;
    let settings = settings::Settings::load(&db, &ca, &config.agent_hostnames.join(",")).await?;
    // Before the HTTPS certificate, so it is issued for the address the install's mode just gave.
    settings.apply_install_api_address(&db, &api_address).await?;
    if let Some(files) = &config.https_files {
        write_https_cert(&ca, &settings, files).await?;
    }
    match &config.master_key {
        Some(key) => {
            auth::seal_totp_secrets(&db, key).await?;
            api::address_book::seal_passwords(&db, key).await?;
        }
        None => tracing::warn!("no RCLI_MASTER_KEY: the CA private key, TOTP secrets and Address Book passwords are stored unencrypted"),
    }
    let ca_pem = ca.cert_pem.clone();
    let storage = storage::Storage::connect(&config.s3).await?;

    let http = tokio::net::TcpListener::bind(config.http_addr).await?;
    let agents = tokio::net::TcpListener::bind(config.agent_addr).await?;
    let (http_addr, agent_addr) = (http.local_addr()?, agents.local_addr()?);
    let acceptor = agent_link::acceptor(&ca, settings::hostnames(&settings.address().await))?;
    let server_key = settings.key().await;

    let state = Arc::new(AppState {
        db,
        storage,
        clock,
        ca,
        settings,
        config,
        tls: tokio::sync::RwLock::new(acceptor),
        connections: Default::default(),
        inventories: Default::default(),
        agents: Default::default(),
        shells: Default::default(),
        lock_watchers: Default::default(),
        lock_released: Default::default(),
    });
    let router = api::router(state.clone()).into_make_service_with_connect_info::<SocketAddr>();
    let tasks = vec![
        tokio::spawn(async move {
            if let Err(e) = axum::serve(http, router).await {
                tracing::error!("controller API stopped: {e}");
            }
        }),
        tokio::spawn(agent_link::serve(agents, state.clone())),
        tokio::spawn({
            let state = state.clone();
            async move {
                // Expired locks are also ended lazily on every read; this hides the Device indicator on time.
                let mut every_second = tokio::time::interval(std::time::Duration::from_secs(1));
                loop {
                    every_second.tick().await;
                    if let Err(e) = control_lock::expire(&state).await {
                        tracing::error!("Control Lock expiry failed: {e:?}");
                    }
                }
            }
        }),
    ];
    Ok(Server { http_addr, agent_addr, ca_pem, server_key, state, tasks })
}

/// Writes a certificate from the CA for the API Address host (the Server Address hosts while there is none) and its
/// key to `(cert, key)`, where the proxy in front of the Controller API reads them (ADR 0015).
async fn write_https_cert(ca: &pki::Ca, settings: &settings::Settings, (cert, key): &(std::path::PathBuf, std::path::PathBuf)) -> anyhow::Result<()> {
    let api = settings.api_address().await;
    let hosts = match api.parse::<axum::http::Uri>().ok().and_then(|u| u.host().map(|h| h.trim_matches(['[', ']']).to_owned())) {
        Some(host) => vec![host],
        None => settings::hostnames(&settings.address().await),
    };
    let (cert_pem, key_pem) = ca.issue_https_cert(hosts.clone())?;
    let mut key_file = std::fs::OpenOptions::new();
    key_file.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut key_file, 0o600);
    std::io::Write::write_all(&mut key_file.open(key)?, key_pem.as_bytes())?;
    std::fs::write(cert, cert_pem)?;
    tracing::info!("wrote the HTTPS certificate for {} to {}", hosts.join(", "), cert.display());
    Ok(())
}
