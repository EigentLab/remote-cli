//! Agent core: registration and the persistent connection to the Server.

pub mod contract;
mod clip;
mod coords;
pub mod download;
mod encoder;
mod executor;
pub mod ipc;
#[cfg(target_os = "linux")]
pub mod linux;
mod live;
#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(unix)]
pub mod macos_session;
mod marks;
mod media;
mod observer;
pub mod platform;
pub mod secret_store;
pub mod session;
pub mod settings;
#[cfg(windows)]
pub mod windows_platform;
#[cfg(windows)]
pub mod windows_session;

use std::{
    io::Read,
    sync::{Arc, Mutex},
    time::Duration,
};

use executor::Executor;

use platform::Platform;
use rcli_protocol::{
    agent::{
        agent_message, server_message, AgentHello, AgentMessage, AgentUpdate,  DeviceInfo, DisconnectController,
        DisplayList, Error, ExecRequest, ExecResult, Heartbeat, LiveAnswer,
        PowerResult, ProcessList, RegisterRequest, RegisterResponse, ReleaseControlLock, Resources, ServerMessage, ShellExit, ShellOpen,
        ShellOpened, ShellOutput, UnregisterRequest, UpdateFailed, WakeResult,
    },
    server_key::ServerKey,
    Conn,
};
use rustls::{
    client::WebPkiServerVerifier,
    pki_types::{pem::PemObject, CertificateDer, PrivateKeyDer, ServerName},
    ClientConfig, DigitallySignedStruct, RootCertStore, SignatureScheme,
};
use serde::{Deserialize, Serialize};
use tokio::net::TcpStream;
use tokio_rustls::{client::TlsStream, TlsConnector};

const HEARTBEAT_EVERY: Duration = Duration::from_secs(10);
/// Inventory is checked this often (the Server picks Wake Relays from it).
const SAMPLE_EVERY: Duration = Duration::from_secs(5);
const MAX_BACKOFF: Duration = Duration::from_secs(30);

/// Where the Server's Agent port is and the Server Key that pins its CA.
#[derive(Clone, Debug)]
pub struct ServerAccess {
    /// `host:port`; `host` must be one of the Server's `agent_hostnames`.
    pub addr: String,
    /// `rk_<CA fingerprint>_<secret>`.
    pub server_key: String,
}

/// `<os>-<arch>` of this Agent binary; Agent builds are signed for one target.
pub fn target() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

/// The Device's identity, obtained once by registering.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Credentials {
    /// 9-digit Device ID.
    pub device_id: String,
    pub cert_pem: String,
    pub key_pem: String,
    /// DER of the Server's CA that the Server Key's fingerprint pinned at registration.
    pub ca_der: Vec<u8>,
    /// The Device Password returned at registration. Never written to `credentials.json`: the Agent keeps it in its
    /// Secret Store and the Server only its hash.
    #[serde(default, skip_serializing)]
    pub device_password: String,
    /// The API Address returned at registration (empty until the Server has learned it). Not written to
    /// `credentials.json`: the Server sends the current one on every connection (`ipc::Status::api_address`).
    #[serde(default, skip_serializing)]
    pub api_address: String,
}

fn parse_server_key(key: &str) -> anyhow::Result<ServerKey> {
    ServerKey::parse(key).ok_or_else(|| anyhow::anyhow!("the Server Key must look like rk_<fingerprint>_<secret>"))
}

/// A TLS verifier that accepts only a chain containing the CA named by `fingerprint` (hex SHA-256 of its DER),
/// then checks the Server certificate against that CA and the connection's hostname.
#[derive(Debug)]
struct PinnedCa {
    fingerprint: String,
    provider: std::sync::Arc<rustls::crypto::CryptoProvider>,
}

impl rustls::client::danger::ServerCertVerifier for PinnedCa {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        // The Server sends [leaf, CA]; the pinned CA may also arrive as an intermediate.
        let ca = std::iter::once(end_entity)
            .chain(intermediates.iter())
            .find(|c| hex::encode(sha256(c)) == self.fingerprint)
            .ok_or_else(|| rustls::Error::General("the Server's CA fingerprint does not match the Server Key".into()))?;
        let mut roots = RootCertStore::empty();
        roots.add(ca.clone()).map_err(|e| rustls::Error::General(format!("bad Server CA: {e}")))?;
        let verifier = WebPkiServerVerifier::builder_with_provider(roots.into(), self.provider.clone())
            .build()
            .map_err(|e| rustls::Error::General(format!("cannot verify the Server: {e}")))?;
        verifier.verify_server_cert(end_entity, intermediates, server_name, ocsp_response, now)
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider.signature_verification_algorithms.supported_schemes()
    }
}

fn sha256(der: &[u8]) -> Vec<u8> {
    use sha2::{Digest, Sha256};
    Sha256::digest(der).to_vec()
}

/// Hex SHA-256 of a CA certificate (PEM).
pub fn ca_fingerprint(ca_pem: &str) -> anyhow::Result<String> {
    Ok(hex::encode(sha256(&CertificateDer::from_pem_slice(ca_pem.as_bytes())?)))
}

/// PEM of a certificate's DER: the Server's CA as an install keeps it for the Controller API (ADR 0015).
pub fn cert_pem(der: &[u8]) -> String {
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD.encode(der);
    let lines: Vec<&str> = b64.as_bytes().chunks(64).map(|l| std::str::from_utf8(l).expect("base64 is ASCII")).collect();
    format!("-----BEGIN CERTIFICATE-----\n{}\n-----END CERTIFICATE-----\n", lines.join("\n"))
}

/// The one HTTP client for the Controller API (ADR 0015): the public CAs and, when `ca_pem` is not empty, the
/// Server's CA this install kept at registration, so a Server on an IP address is reached over HTTPS too.
pub fn http_client(ca_pem: &str) -> reqwest::Client {
    let builder = reqwest::Client::builder();
    let builder = match reqwest::Certificate::from_pem(ca_pem.as_bytes()) {
        Ok(ca) if !ca_pem.is_empty() => builder.add_root_certificate(ca),
        _ => builder,
    };
    // A damaged stored CA must not stop calls to a Server with a public certificate.
    builder.build().unwrap_or_else(|e| {
        tracing::warn!("ignoring the stored Server CA: {e}");
        reqwest::Client::new()
    })
}

fn config(server: &ServerAccess, creds: Option<&Credentials>) -> anyhow::Result<ClientConfig> {
    config_pinned(&parse_server_key(&server.server_key)?.fingerprint, creds)
}

/// `config` with the pinned CA fingerprint given directly: an uninstall may not have the Server Key any more, but
/// it still has the CA the Device pinned at registration.
fn config_pinned(fingerprint: &str, creds: Option<&Credentials>) -> anyhow::Result<ClientConfig> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let provider = rustls::crypto::CryptoProvider::get_default()
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("no crypto provider"))?;
    let builder = ClientConfig::builder().dangerous().with_custom_certificate_verifier(std::sync::Arc::new(PinnedCa {
        fingerprint: fingerprint.to_owned(),
        provider,
    }));
    Ok(match creds {
        None => builder.with_no_client_auth(),
        Some(c) => builder.with_client_auth_cert(
            vec![CertificateDer::from_pem_slice(c.cert_pem.as_bytes())?],
            PrivateKeyDer::from_pem_slice(c.key_pem.as_bytes())?,
        )?,
    })
}

async fn connect(server: &ServerAccess, creds: Option<&Credentials>) -> anyhow::Result<Conn<TlsStream<TcpStream>>> {
    let config = config(server, creds)?;
    connect_with_config(&server.addr, config).await
}

async fn connect_with_config(addr: &str, config: ClientConfig) -> anyhow::Result<Conn<TlsStream<TcpStream>>> {
    let host = addr.rsplit_once(':').map_or(addr, |(h, _)| h);
    let name = ServerName::try_from(host.trim_matches(['[', ']']).to_owned())?;
    let tcp = TcpStream::connect(addr).await?;
    let tls = TlsConnector::from(Arc::new(config)).connect(name, tcp).await?;
    Ok(Conn::new(tls))
}

/// Registers this Device with the Server Key: checks the Server's CA against the key's fingerprint, presents its
/// secret, and gets back this Device's mTLS certificate and 9-digit Device ID.
/// The private key never leaves the Agent; only a CSR is sent.
pub async fn register(server: &ServerAccess, platform: &dyn Platform) -> anyhow::Result<Credentials> {
    let key = rcgen::KeyPair::generate()?;
    let csr = rcgen::CertificateParams::new(vec![])?.serialize_request(&key)?.pem()?;
    let (r, ca_der) = send_register(server, csr, platform.hostname()).await?;
    Ok(Credentials {
        device_id: r.device_id,
        cert_pem: r.cert_pem,
        key_pem: key.serialize_pem(),
        ca_der,
        device_password: r.device_password,
        api_address: r.api_address,
    })
}

/// Checks the Server Key against the Server without registering: a reinstall's first step, so a wrong key fails
/// before the existing install is touched.
pub async fn check_server_key(server: &ServerAccess) -> anyhow::Result<()> {
    send_register(server, String::new(), String::new()).await.map(drop)
}

/// One RegisterRequest (an empty `csr` only checks the key); the reply and the Server's CA it pinned.
async fn send_register(server: &ServerAccess, csr: String, hostname: String) -> anyhow::Result<(RegisterResponse, Vec<u8>)> {
    let server_key = parse_server_key(&server.server_key)?;
    let mut conn = connect(server, None).await?;
    let req = RegisterRequest { server_key: server.server_key.clone(), csr_pem: csr, hostname };
    conn.send(&AgentMessage { msg: Some(agent_message::Msg::Register(req)) }).await?;
    match conn.recv::<ServerMessage>().await?.and_then(|m| m.msg) {
        Some(server_message::Msg::Registered(r)) => {
            // The verifier accepted the chain only because it held the pinned CA; the Server sends it here.
            let ca_der = CertificateDer::from_pem_slice(r.ca_pem.as_bytes())?.to_vec();
            anyhow::ensure!(
                hex::encode(sha256(&ca_der)) == server_key.fingerprint,
                "the Server CA's fingerprint does not match the Server Key; refusing to register"
            );
            Ok((r, ca_der))
        }
        Some(server_message::Msg::Error(e)) => anyhow::bail!("registration rejected: {}: {}", e.code, e.message),
        Some(_) | None => anyhow::bail!("server closed the connection during registration"),
    }
}

/// Removes this Device from the Server, what `sudo rcli uninstall` does before it deletes the local
/// credentials. It authenticates with the Device's certificate, not the Server Key: the Device may already have
/// forgotten the key, and the Server only needs to know which Device asks.
pub async fn unregister(server: &ServerAccess, creds: &Credentials) -> anyhow::Result<()> {
    unregister_at(&server.addr, creds).await
}

/// `unregister` at a bare `host:port`, trusted through the CA the Device pinned at registration.
pub async fn unregister_at(addr: &str, creds: &Credentials) -> anyhow::Result<()> {
    let fingerprint = hex::encode(sha256(&creds.ca_der));
    let config = config_pinned(&fingerprint, Some(creds))?;
    let mut conn = connect_with_config(addr, config).await?;
    conn.send(&AgentMessage { msg: Some(agent_message::Msg::Unregister(UnregisterRequest {})) }).await?;
    // The Server deletes the Device and closes the connection, whether or not the row still existed.
    let _ = tokio::time::timeout(Duration::from_secs(5), conn.recv::<ServerMessage>()).await;
    Ok(())
}

/// Keeps a connection to the Server open forever, reconnecting with backoff.
/// Resources are sampled every 5 s whether connected or not; samples taken offline are backfilled on reconnect.
/// With a `status`, a changed Server Address or Server Key reconnects at once and Device identity changes go over the connection.
pub async fn run(server: ServerAccess, mut creds: Credentials, platform: Arc<dyn Platform>, status: Option<Arc<ipc::Status>>) {
    // With IPC, the Status owns the Server Address and Server Key (a change reconnects) and receives the changes to forward.
    let mut server_access = status.as_ref().map_or(server, |s| s.server_access());
    // ADR 0005: rcli stores no OS password; delete the unlock password an older Agent may have left.
    if let Some(s) = &status {
        s.set_platform(platform.clone());
    }
    if let Some(Err(e)) = status.as_ref().map(|s| s.secrets().remove("unlock_password")) {
        tracing::warn!("cannot delete the leftover unlock password: {e:#}");
    }
    let mut inbox = Inbox {
        server_access: status.as_ref().map(|s| s.subscribe()),
        changes: status.as_ref().and_then(|s| s.take_changes()),
        disconnects: status.as_ref().and_then(|s| s.take_disconnects()),
        lock_releases: status.as_ref().and_then(|s| s.take_lock_releases()),
    };
    let agent = AgentState {
        // Outlives sessions: the last acted-on display and what each Controller last saw survive a reconnect.
        // Local control (`rcli shot local`) shares it, so Actions from both run one at a time.
        executor: status.as_ref().map_or_else(|| Arc::new(Mutex::new(Executor::default())), |s| s.executor()),
        // Live streams are peer-to-peer, so they survive a lost Server connection.
        live: live::Live::new(platform.clone()),
        media: Arc::new(media::Media::default()),
        streams: Default::default(),
        // Device Settings survive reconnects; with no Status (no IPC) the defaults apply.
        settings: status.as_ref().map_or_else(|| Arc::new(settings::Handle::new()), |s| s.settings()),
        // A root grant lasts one session (one connection to the Server) and only for the Controller it was given to.
        grant: status.as_ref().map_or_else(|| Arc::new(Grant::default()), |s| s.grant()),
        // Connected Controllers outlive a reconnect: they end on `disconnect`, not when the Server link blips.
        controllers: status.as_ref().map_or_else(|| Arc::new(Controllers::default()), |s| s.controllers()),
        platform,
        status,
    };
    let mut backoff = Duration::from_secs(1);
    loop {
        // Another Server Address is one switch away; pick up whichever is current.
        if let Some(rx) = &mut inbox.server_access {
            server_access = rx.borrow_and_update().clone();
        }
        let mut reconnect = false;
        match connect(&server_access, Some(&creds)).await {
            Ok(conn) => {
                backoff = Duration::from_secs(1);
                // A session grant belongs to the session that ended with the last connection.
                agent.grant.clear();
                if let Some(s) = &agent.status {
                    s.set_connected(true);
                }
                match session(conn, &server_access.server_key, &mut creds, &agent, &mut inbox).await {
                    Ok(now) => reconnect = now,
                    Err(e) => tracing::warn!("connection to server lost: {e}"),
                }
                if let Some(s) = &agent.status {
                    s.set_connected(false);
                }
            }
            Err(e) => {
                if let Some(s) = &agent.status {
                    s.set_connected(false);
                }
                tracing::warn!("cannot connect to server: {e}");
            }
        }
        // A new Server Address reconnects at once; otherwise wait out the backoff.
        if reconnect {
            continue;
        }
        match &mut inbox.server_access {
            Some(rx) => tokio::select! {
                _ = tokio::time::sleep(backoff) => {}
                changed = rx.changed() => if changed.is_ok() { continue },
            },
            None => tokio::time::sleep(backoff).await,
        }
        backoff = (backoff * 2).min(MAX_BACKOFF);
    }
}

/// What outlives one connection to the Server: every session of `run` shares it.
struct AgentState {
    platform: Arc<dyn Platform>,
    executor: Arc<Mutex<Executor>>,
    live: Arc<live::Live>,
    media: Arc<media::Media>,
    /// Who opened each live stream and media channel (by session), so a `disconnect` ends only theirs.
    // shortcut: a stream that ends on its own stays listed until `disconnect` or LiveClose; closing it is a no-op.
    streams: Arc<Mutex<std::collections::HashMap<String, String>>>,
    status: Option<Arc<ipc::Status>>,
    settings: Arc<settings::Handle>,
    grant: Arc<Grant>,
    controllers: Arc<Controllers>,
}

/// What the person at the Device sends the running Agent over the IPC; all `None` without a Status.
struct Inbox {
    /// A new Server Address or Key.
    server_access: Option<tokio::sync::watch::Receiver<ServerAccess>>,
    /// Device identity changes to forward to the Server.
    changes: Option<tokio::sync::mpsc::UnboundedReceiver<ipc::Change>>,
    /// Controllers the person at the Device disconnected.
    disconnects: Option<tokio::sync::mpsc::UnboundedReceiver<String>>,
    /// Control Lock force-releases by the Device Owner.
    lock_releases: Option<tokio::sync::mpsc::UnboundedReceiver<()>>,
}

/// Waits until the Server Address changes; never, without a Status.
async fn server_access_changed(rx: &mut Option<tokio::sync::watch::Receiver<ServerAccess>>) -> bool {
    match rx {
        Some(rx) => rx.changed().await.is_ok(),
        None => std::future::pending().await,
    }
}

/// The next message on an IPC channel; never, without a Status.
async fn next<T>(rx: &mut Option<tokio::sync::mpsc::UnboundedReceiver<T>>) -> Option<T> {
    match rx {
        Some(rx) => rx.recv().await,
        None => std::future::pending().await,
    }
}

async fn session(
    mut conn: Conn<TlsStream<TcpStream>>,
    server_key: &str,
    creds: &mut Credentials,
    agent: &AgentState,
    inbox: &mut Inbox,
) -> anyhow::Result<bool> {
    let platform = agent.platform.as_ref();
    let status = agent.status.as_deref();
    let hello = AgentHello { version: platform.agent_version(), target: target() };
    let send = |msg| AgentMessage { msg: Some(msg) };
    conn.send(&send(agent_message::Msg::Hello(hello))).await?;
    let mut tick = tokio::time::interval(HEARTBEAT_EVERY);
    let mut sample = tokio::time::interval(SAMPLE_EVERY);
    // What the Server has seen this session; a new session starts from nothing, so it gets everything.
    let mut inventory = None;
    // Replies from work that runs off this loop (Actions block on the OS for up to seconds; prompts for 30 s).
    let (replies, mut replies_rx) = tokio::sync::mpsc::unbounded_channel();
    // Device identity changes sent to the Server, awaiting its answer by request_id.
    let mut pending: std::collections::HashMap<u64, tokio::sync::oneshot::Sender<Result<String, String>>> = Default::default();
    // Shells are opened off this loop (a prompt may wait 30 s), so the map is shared with those tasks.
    let shells = Arc::new(Mutex::new(Shells::default()));
    // Updates download and install off this loop, one task per version; each ends `true` once installed. Dropping the
    // set with the session aborts them, and the next session's task resumes the partial download.
    let mut updates = tokio::task::JoinSet::new();
    let mut updating: std::collections::HashMap<String, tokio::task::AbortHandle> = Default::default();
    // What every request that may prompt at the Device needs, cloned into the task that serves it.
    let gate = || Gate {
        platform: agent.platform.clone(),
        settings: agent.settings.get(),
        controllers: agent.controllers.clone(),
        replies: replies.clone(),
    };
    // Ends the connections of Controllers that left on their own, idle for `idle`, and their root grant.
    let end_idle = |idle| {
        let streams = agent.streams.lock().unwrap();
        let shells = shells.lock().unwrap();
        let busy = |name: &str| streams.values().any(|c| c == name) || shells.0.values().any(|s| s.controller == name);
        for name in agent.controllers.end_idle(busy, idle) {
            agent.grant.revoke(&name);
        }
    };
    // Ends the shells of Controllers whose terminal capability was switched off after they opened one.
    let end_shells_without_terminal = || {
        let allowed = agent.settings.get().capabilities;
        shells.lock().unwrap().end_if(|controller| !allowed.and(agent.controllers.capabilities(controller)).terminal);
    };
    loop {
        tokio::select! {
            _ = tick.tick() => {
                end_idle(CONNECTION_IDLE);
                end_shells_without_terminal();
                conn.send(&send(agent_message::Msg::Heartbeat(Heartbeat {}))).await?;
            }
            _ = sample.tick() => {
                // shortcut: inventory is re-read every sample; OS adapters may want change notifications instead.
                match platform.inventory() {
                    Ok(inv) if inventory.as_ref() != Some(&inv) => {
                        conn.send(&send(agent_message::Msg::Inventory(inv.clone()))).await?;
                        inventory = Some(inv);
                    }
                    Ok(_) => {}
                    Err(e) => tracing::warn!("cannot read inventory: {e:#}"),
                }
            }
            Some(done) = updates.join_next() => {
                // Reconnect so the Server sees the new version (real adapters restart instead).
                if matches!(done, Ok(true)) {
                    return Ok(true);
                }
            }
            Some(reply) = replies_rx.recv() => {
                if let agent_message::Msg::ShellExit(e) = &reply {
                    shells.lock().unwrap().0.remove(&e.session);
                }
                conn.send(&send(reply)).await?
            }
            Some(change) = next(&mut inbox.changes) => {
                pending.insert(change.update.request_id, change.reply);
                conn.send(&send(agent_message::Msg::IdentityUpdate(change.update))).await?;
            }
            Some(controller) = next(&mut inbox.disconnects) => {
                // End this Controller's shells, live streams and media channels; the Server releases its Control Lock.
                shells.lock().unwrap().end_if(|c| c == controller);
                let ended: Vec<String> = {
                    let mut streams = agent.streams.lock().unwrap();
                    let ended = streams.iter().filter(|(_, c)| **c == controller).map(|(s, _)| s.clone()).collect();
                    streams.retain(|_, c| *c != controller);
                    ended
                };
                for session in ended {
                    agent.live.close(&session).await;
                    agent.media.close(&session).await;
                }
                conn.send(&send(agent_message::Msg::DisconnectController(DisconnectController { controller }))).await?;
            }
            Some(()) = next(&mut inbox.lock_releases) => {
                conn.send(&send(agent_message::Msg::ReleaseControlLock(ReleaseControlLock {}))).await?;
            }
            // A new Server Address or Key: drop this connection so `run` connects with the new values.
            _ = server_access_changed(&mut inbox.server_access) => return Ok(true),
            msg = conn.recv::<ServerMessage>() => {
                match msg?.and_then(|m| m.msg) {
                    None => anyhow::bail!("server closed the connection"),
                    Some(server_message::Msg::Error(e)) => anyhow::bail!("server refused: {}: {}", e.code, e.message),
                    Some(server_message::Msg::Act(req)) => {
                        // The Executor's lock runs requests one at a time, in arrival order.
                        let (platform, executor, replies) = (agent.platform.clone(), agent.executor.clone(), replies.clone());
                        let (media, rt) = (agent.media.clone(), tokio::runtime::Handle::current());
                        let gate = gate();
                        tokio::task::spawn_blocking(move || {
                            let failed = |error| {
                                let _ = replies.send(agent_message::Msg::ActResult(rcli_protocol::agent::ActResult {
                                    request_id: req.request_id,
                                    error: Some(error),
                                    ..Default::default()
                                }));
                            };
                            if let Some(e) = out_of_user_mode_reach(platform.as_ref()) {
                                return failed(e);
                            }
                            use rcli_protocol::agent::action::Kind;
                            // Waits and observing need no capability; every other Action is mouse or keyboard input.
                            let input = req.actions.iter().any(|a| !matches!(a.kind, None | Some(Kind::WaitMs(_) | Kind::WaitUntil(_))));
                            let admitted = if input { gate.admit_to(&req.controller, Capability::MouseKeyboard) } else { gate.admit(&req.controller) };
                            if let Err(e) = admitted {
                                return failed(e);
                            }
                            let mut result = executor.lock().unwrap().act(platform.as_ref(), &req);
                            if !req.media_session.is_empty() {
                                rt.block_on(media.offload(&req.media_session, &mut result));
                            }
                            let _ = replies.send(agent_message::Msg::ActResult(result));
                        });
                    }
                    Some(server_message::Msg::LiveOffer(offer)) => {
                        // Answering waits for ICE gathering (and maybe an accept prompt); keep the loop running.
                        let (live, media, streams, replies) = (agent.live.clone(), agent.media.clone(), agent.streams.clone(), replies.clone());
                        let gate = gate();
                        tokio::spawn(async move {
                            let controller = offer.controller.clone();
                            let admitted = tokio::task::spawn_blocking(move || {
                                out_of_user_mode_reach(gate.platform.as_ref()).map_or_else(|| gate.admit(&controller), Err)
                            })
                            .await;
                            let answer = match admitted {
                                Ok(Ok(())) => {
                                    streams.lock().unwrap().insert(offer.session.clone(), offer.controller.clone());
                                    if offer.media { media.open(offer).await } else { live.open(offer).await }
                                }
                                Ok(Err(error)) => LiveAnswer { request_id: offer.request_id, error: Some(error), ..Default::default() },
                                Err(e) => LiveAnswer { request_id: offer.request_id, error: Some(platform_error(e)), ..Default::default() },
                            };
                            let _ = replies.send(agent_message::Msg::LiveAnswer(answer));
                        });
                    }
                    Some(server_message::Msg::LiveClose(c)) => {
                        agent.streams.lock().unwrap().remove(&c.session);
                        let (live, media) = (agent.live.clone(), agent.media.clone());
                        tokio::spawn(async move {
                            live.close(&c.session).await;
                            media.close(&c.session).await;
                        });
                    }
                    Some(server_message::Msg::Exec(req)) => {
                        // Off the loop: a command may run for minutes; commands run concurrently.
                        let grant = agent.grant.clone();
                        let gate = gate();
                        tokio::task::spawn_blocking(move || {
                            let _ = gate.replies.send(agent_message::Msg::ExecResult(exec(&gate, &req, &grant)));
                        });
                    }
                    Some(server_message::Msg::ShellOpen(req)) => {
                        // Off the loop: the accept and root prompts may each wait 30 s.
                        let (grant, shells) = (agent.grant.clone(), shells.clone());
                        let gate = gate();
                        tokio::task::spawn_blocking(move || open_shell(&gate, &req, &grant, &shells));
                    }
                    Some(server_message::Msg::ShellInput(i)) => {
                        end_shells_without_terminal();
                        // shortcut: written inline; a PTY whose buffer is full (shell not reading) would stall this loop.
                        if let Some(shell) = shells.lock().unwrap().0.get_mut(&i.session) {
                            let _ = std::io::Write::write_all(&mut shell.input, &i.data);
                        }
                    }
                    Some(server_message::Msg::ShellResize(r)) => {
                        if let Some(shell) = shells.lock().unwrap().0.get(&r.session) {
                            let _ = shell.control.lock().unwrap().resize(clamp_u16(r.cols), clamp_u16(r.rows));
                        }
                    }
                    Some(server_message::Msg::ShellClose(c)) => {
                        if let Some(shell) = shells.lock().unwrap().0.remove(&c.session) {
                            let _ = shell.control.lock().unwrap().kill();
                        }
                    }
                    Some(server_message::Msg::ListDisplays(req)) => {
                        let (displays, error) = match platform.displays() {
                            Ok(d) => (d, None),
                            Err(e) => (vec![], Some(Error { code: "platform".into(), message: e.to_string() })),
                        };
                        let reply = DisplayList { request_id: req.request_id, displays, error };
                        conn.send(&send(agent_message::Msg::Displays(reply))).await?;
                    }
                    Some(server_message::Msg::DeviceInfo(req)) => {
                        let reply = match platform.inventory().and_then(|i| Ok((i, sample_resources(platform)?))) {
                            Ok((i, r)) => DeviceInfo { request_id: req.request_id, inventory: Some(i), resources: Some(r), error: String::new() },
                            Err(e) => DeviceInfo { request_id: req.request_id, error: format!("{e:#}"), ..Default::default() },
                        };
                        conn.send(&send(agent_message::Msg::DeviceInfo(reply))).await?;
                    }
                    Some(server_message::Msg::ListProcesses(req)) => {
                        let reply = match platform.processes() {
                            Ok(processes) => ProcessList { request_id: req.request_id, processes, error: String::new() },
                            Err(e) => ProcessList { request_id: req.request_id, processes: vec![], error: format!("{e:#}") },
                        };
                        conn.send(&send(agent_message::Msg::Processes(reply))).await?;
                    }
                    Some(server_message::Msg::Wake(req)) => {
                        let sent = <[u8; 6]>::try_from(req.mac.as_slice())
                            .map_err(|_| anyhow::anyhow!("MAC must be 6 bytes"))
                            .and_then(|mac| platform.send_wake(mac));
                        let error = sent.err().map(|e| Error { code: "platform".into(), message: format!("{e:#}") });
                        let reply = WakeResult { request_id: req.request_id, error };
                        conn.send(&send(agent_message::Msg::WakeResult(reply))).await?;
                    }
                    Some(server_message::Msg::Power(req)) => {
                        use rcli_protocol::agent::PowerAction as P;
                        let action = match req.action() {
                            P::Restart => platform::PowerAction::Restart,
                            P::Shutdown => platform::PowerAction::Shutdown,
                            P::Sleep => platform::PowerAction::Sleep,
                            P::LockScreen => platform::PowerAction::Lock,
                        };
                        // Off the loop: adapters may take a moment to hand the request to the OS.
                        let gate = gate();
                        tokio::task::spawn_blocking(move || {
                            let done = gate.admit_to(&req.controller, Capability::Power).and_then(|()| {
                                gate.platform.power(action).map_err(|e| Error { code: "platform".into(), message: format!("{e:#}") })
                            });
                            let reply = PowerResult { request_id: req.request_id, error: done.err() };
                            let _ = gate.replies.send(agent_message::Msg::PowerResult(reply));
                        });
                    }
                    Some(server_message::Msg::Update(update)) if !agent.settings.get().auto_update => {
                        tracing::info!("auto-update is off at this Device: not installing Agent {}", update.version);
                    }
                    Some(server_message::Msg::Update(update)) => {
                        if updating.get(&update.version).is_none_or(|t| t.is_finished()) {
                            let api = status.map(|s| s.api_address()).filter(|a| !a.is_empty());
                            let job = UpdateJob {
                                http: http_client(&cert_pem(&creds.ca_der)),
                                url: api.map(|api| format!("{}/api/v1/agent-builds/{}", api.trim_end_matches('/'), target())),
                                server_key: server_key.to_owned(),
                                dir: agent.settings.dir().unwrap_or_else(std::env::temp_dir),
                                platform: agent.platform.clone(),
                                replies: replies.clone(),
                                update,
                            };
                            updating.insert(job.update.version.clone(), updates.spawn(update_agent(job)));
                        }
                    }
                    Some(server_message::Msg::ControlIndicator(c)) => {
                        agent.live.set_lock_indicator(c.on);
                        // The Control Lock ended: whoever has no live stream or shell left has gone.
                        if !c.on {
                            end_idle(Duration::ZERO);
                        }
                    }
                    Some(server_message::Msg::ApiAddress(a)) => {
                        if let Some(s) = status {
                            s.set_api_address(&a.url);
                        }
                    }
                    Some(server_message::Msg::IdentityUpdated(r)) => {
                        if let Some(reply) = pending.remove(&r.request_id) {
                            let result = match r.error {
                                Some(e) => Err(e.message),
                                None => {
                                    if let Some(s) = status {
                                        s.set_device_id(&r.device_id);
                                    }
                                    creds.device_id = r.device_id.clone();
                                    Ok(r.device_id)
                                }
                            };
                            let _ = reply.send(result);
                        }
                    }
                    // Later tickets handle more requests from the Server here.
                    Some(_) => {}
                }
            }
        }
    }
}

fn platform_error(e: impl std::fmt::Display) -> Error {
    Error { code: "platform".into(), message: e.to_string() }
}

/// How long the person at the Device has to answer a root prompt.
const ROOT_PROMPT_TIMEOUT: Duration = Duration::from_secs(30);

/// How long the person at the Device has to accept a new connection.
const ACCEPT_PROMPT_TIMEOUT: Duration = Duration::from_secs(30);

/// Whether the Controller display name `name` (e.g. "alice (Dashboard)", "dev-laptop (10.0.0.5)") is the one the
/// person at the Device means by `who`: the full name, or the part before " (".
pub fn names(name: &str, who: &str) -> bool {
    name == who || name.strip_prefix(who).is_some_and(|rest| rest.starts_with(" ("))
}

/// Whether a root grant given as `who` covers the Controller display name `controller`. Unlike `names`, a bare name
/// covers only the Dashboard User of that name (the Server authenticated it): a password connection picks its own
/// machine name, so only its full display name `<host> (<IP>)` covers it.
fn granted(controller: &str, who: &str) -> bool {
    controller == who || controller == format!("{who} (Dashboard)")
}

/// Root granted to the current session: one Controller, for as long as the Agent's connection to the Server lasts
/// or until the person at the Device disconnects that Controller. The person at the Device (or its GUI) gives it.
#[derive(Default)]
pub struct Grant(Mutex<Option<String>>);

impl Grant {
    /// Grants root to the Controller the person calls `who` (see `granted`) for this session only.
    pub fn grant(&self, who: &str) {
        *self.0.lock().unwrap() = Some(who.to_owned());
    }

    fn holds(&self, controller: &str) -> bool {
        self.0.lock().unwrap().as_deref().is_some_and(|who| granted(controller, who))
    }

    /// Ends the grant if `controller` holds it: a disconnected Controller does not keep root when it reconnects.
    pub fn revoke(&self, controller: &str) {
        let mut grant = self.0.lock().unwrap();
        if grant.as_deref().is_some_and(|who| granted(controller, who)) {
            *grant = None;
        }
    }

    /// Ends the grant, at the end of the session it was given for.
    pub fn clear(&self) {
        *self.0.lock().unwrap() = None;
    }
}

/// Controllers using this Device right now, keyed by their display name (as `rcli agent info` lists them), with
/// whether each has been accepted at the Device (only matters in `password+accept` mode) and the capability switches
/// of that connection. Survives a reconnect.
#[derive(Default)]
pub struct Controllers(std::sync::Mutex<std::collections::HashMap<String, Connection>>);

struct Connection {
    accepted: bool,
    /// Narrows the Device Settings' capabilities for this connection only; all on until the person changes it.
    capabilities: settings::Capabilities,
    /// When it connected, and what its last request did, for its Session Window.
    since: std::time::Instant,
    doing: &'static str,
    /// Its last request, so a Controller that left on its own is dropped.
    last: std::time::Instant,
}

impl Default for Connection {
    fn default() -> Self {
        let now = std::time::Instant::now();
        Self { accepted: false, capabilities: Default::default(), since: now, doing: "watching the screen", last: now }
    }
}

/// A Controller with no live stream or shell and no request for this long has left (the Control Lock's idle timeout,
/// so a lock holder is never dropped while its lock lives).
const CONNECTION_IDLE: Duration = Duration::from_secs(10 * 60);

impl Controllers {
    /// Records `name` as connected (keeping any earlier accept).
    fn touch(&self, name: &str) {
        self.0.lock().unwrap().entry(name.to_owned()).or_default().last = std::time::Instant::now();
    }

    /// Ends the connections that have no live stream or shell (`busy`) and made no request for `idle`: the Controller
    /// left on its own (decision 17). Returns the display names ended.
    fn end_idle(&self, busy: impl Fn(&str) -> bool, idle: Duration) -> Vec<String> {
        let mut map = self.0.lock().unwrap();
        let ended: Vec<String> = map.iter().filter(|(n, c)| !busy(n) && c.last.elapsed() >= idle).map(|(n, _)| n.clone()).collect();
        for name in &ended {
            map.remove(name);
        }
        ended
    }

    /// Records what `name`'s last request did.
    fn doing(&self, name: &str, doing: &'static str) {
        if let Some(c) = self.0.lock().unwrap().get_mut(name) {
            c.doing = doing;
        }
    }

    /// Each connection as its Session Window shows it: name, seconds connected, what it is doing, its capabilities
    /// within `allowed` (the Device Settings) and whether it holds the root grant.
    pub fn connections(&self, allowed: settings::Capabilities, grant: &Grant) -> serde_json::Value {
        let map = self.0.lock().unwrap();
        map.iter()
            .map(|(name, c)| {
                serde_json::json!({
                    "name": name,
                    "connected_secs": c.since.elapsed().as_secs(),
                    "doing": c.doing,
                    "accepted": c.accepted,
                    "capabilities": allowed.and(c.capabilities),
                    "root": grant.holds(name),
                })
            })
            .collect()
    }

    fn accept(&self, name: &str) {
        self.0.lock().unwrap().entry(name.to_owned()).or_default().accepted = true;
    }

    fn accepted(&self, name: &str) -> bool {
        self.0.lock().unwrap().get(name).is_some_and(|c| c.accepted)
    }

    fn capabilities(&self, name: &str) -> settings::Capabilities {
        self.0.lock().unwrap().get(name).map(|c| c.capabilities).unwrap_or_default()
    }

    /// Changes the capability switches of the connections the person calls `who` (see `names`), returning the display
    /// names changed.
    pub fn set_capabilities(&self, who: &str, f: impl Fn(&mut settings::Capabilities)) -> Vec<String> {
        let mut map = self.0.lock().unwrap();
        let mut changed = Vec::new();
        for (name, conn) in map.iter_mut().filter(|(n, _)| names(n, who)) {
            f(&mut conn.capabilities);
            changed.push(name.clone());
        }
        changed
    }

    /// The display names of the connected Controllers.
    pub fn list(&self) -> Vec<String> {
        self.0.lock().unwrap().keys().cloned().collect()
    }

    /// Removes the Controllers the person calls `who` (see `names`), returning the display names that were ended.
    pub fn remove(&self, who: &str) -> Vec<String> {
        let mut map = self.0.lock().unwrap();
        let removed: Vec<String> = map.keys().filter(|n| names(n, who)).cloned().collect();
        for name in &removed {
            map.remove(name);
        }
        removed
    }

    /// Removes every Controller, returning their display names.
    pub fn clear(&self) -> Vec<String> {
        self.0.lock().unwrap().drain().map(|(k, _)| k).collect()
    }
}

/// How a request that can prompt the person at the Device was decided, as the Controller is told.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Decision {
    /// Root: the setting is `always`.
    Always,
    /// Root: the person granted it to this session.
    Session,
    Accepted,
    Rejected,
    /// Nobody is logged in to show the prompt to.
    NoSession,
    /// Root: the setting is `never`.
    Never,
}

impl Decision {
    fn as_str(self) -> &'static str {
        match self {
            Decision::Always => "always",
            Decision::Session => "session",
            Decision::Accepted => "accepted",
            Decision::Rejected => "rejected",
            Decision::NoSession => "no_session",
            Decision::Never => "never",
        }
    }
}

/// The wire form of a decision: empty when nothing was decided (no prompt, root not asked for).
fn decision_name(decision: Option<Decision>) -> String {
    decision.map_or("", Decision::as_str).into()
}

/// Prompts appear in the logged-in session; with nobody logged in there is no session to show one in.
fn nobody_to_ask(platform: &dyn Platform) -> bool {
    platform.session_state().is_ok_and(|s| s != platform::SessionState::LoggedIn)
}

/// User Mode runs in its user's login session: the lock and login screens are out of its reach (ADR 0012).
pub(crate) fn out_of_user_mode_reach(platform: &dyn Platform) -> Option<Error> {
    (platform.user_mode().is_some() && nobody_to_ask(platform)).then(|| Error {
        code: "service_mode_required".into(),
        message: format!("the lock or login screen {}", platform::SERVICE_MODE_NEEDED),
    })
}

/// A refused request and how it was decided.
type Refusal = (Error, Option<Decision>);

/// Who a terminal command or session runs as: the Device's Terminal User, or root when asked and allowed.
/// `grant` is the current session's root grant, held by `controller` (a display name). `requester` is who the root
/// prompt names. Returns the account and how root was decided.
fn account_for(
    platform: &dyn Platform,
    settings: &settings::DeviceSettings,
    as_root: bool,
    requester: &str,
    controller: &str,
    grant: &Grant,
) -> Result<(platform::Account, Option<Decision>), Refusal> {
    let denied = |message: &str, decision| (Error { code: "root_denied".into(), message: message.into() }, Some(decision));
    // User Mode runs commands only as the account the Agent runs as; it never asks for root (ADR 0012).
    if let Some(me) = platform.user_mode() {
        let wanted = if as_root { "root" } else { settings.terminal_user.as_str() };
        if wanted != me {
            let message = format!("running as {wanted} {}", platform::SERVICE_MODE_NEEDED);
            return Err((Error { code: "service_mode_required".into(), message }, None));
        }
    }
    if !as_root {
        return Ok((platform::Account::User(settings.terminal_user.clone()), None));
    }
    match settings.root {
        settings::RootAccess::Always => return Ok((platform::Account::Root, Some(Decision::Always))),
        settings::RootAccess::Never => return Err(denied("root access is turned off on this Device", Decision::Never)),
        settings::RootAccess::Ask => {}
    }
    if grant.holds(controller) {
        return Ok((platform::Account::Root, Some(Decision::Session)));
    }
    if nobody_to_ask(platform) {
        return Err(denied("root access needs a person at the Device to accept, and nobody can be asked right now", Decision::NoSession));
    }
    match platform.ask_root(requester, ROOT_PROMPT_TIMEOUT) {
        Ok(true) => Ok((platform::Account::Root, Some(Decision::Accepted))),
        Ok(false) => Err(denied("the person at the Device did not accept root access", Decision::Rejected)),
        Err(e) => Err((platform_error(format!("cannot ask at the Device: {e:#}")), None)),
    }
}

/// A Controller capability a request needs (see `settings::Capabilities`).
enum Capability {
    MouseKeyboard,
    Terminal,
    Power,
}

/// What a request that may prompt the person at the Device needs, off the session loop.
struct Gate {
    platform: Arc<dyn Platform>,
    settings: settings::DeviceSettings,
    controllers: Arc<Controllers>,
    replies: tokio::sync::mpsc::UnboundedSender<agent_message::Msg>,
}

impl Gate {
    /// Admits `controller` (a display name) to any request on this Device and lists it as connected. In
    /// `password+accept` mode a person at the Device accepts each new Controller once (re-asked after a
    /// `disconnect`); refused when no session can show the prompt.
    fn admit(&self, controller: &str) -> Result<(), Error> {
        self.controllers.touch(controller);
        self.controllers.doing(controller, "watching the screen");
        if self.settings.accept == settings::AcceptMode::Password || self.controllers.accepted(controller) {
            return Ok(());
        }
        let denied = |message: &str| Error { code: "connect_denied".into(), message: message.into() };
        if nobody_to_ask(self.platform.as_ref()) {
            return Err(denied("a person at the Device must accept this connection, and nobody can be asked right now"));
        }
        match self.platform.ask_accept(controller, ACCEPT_PROMPT_TIMEOUT) {
            Ok(true) => {
                self.controllers.accept(controller);
                Ok(())
            }
            Ok(false) => Err(denied("the person at the Device did not accept this connection")),
            Err(e) => Err(platform_error(format!("cannot ask at the Device: {e:#}"))),
        }
    }

    /// Admits `controller` (see `admit`) to a request that needs a capability the Device Settings and this
    /// connection's switches both allow.
    fn admit_to(&self, controller: &str, needs: Capability) -> Result<(), Error> {
        self.admit(controller)?;
        let caps = self.settings.capabilities.and(self.controllers.capabilities(controller));
        let (on, what, doing) = match needs {
            Capability::MouseKeyboard => (caps.mouse_keyboard, "mouse and keyboard", "using the mouse and keyboard"),
            Capability::Terminal => (caps.terminal, "the terminal", "using the terminal"),
            Capability::Power => (caps.power, "power", "restarting, sleeping or locking the Device"),
        };
        match on {
            true => {
                self.controllers.doing(controller, doing);
                Ok(())
            }
            false => Err(Error {
                code: "capability_off".into(),
                message: format!("{what} is switched off for this Controller"),
            }),
        }
    }

    /// Admits `controller` to the terminal, then decides the account its command or shell runs as (see `account_for`).
    fn account(&self, controller: &str, as_root: bool, requester: &str, grant: &Grant) -> Result<(platform::Account, Option<Decision>), Refusal> {
        self.admit_to(controller, Capability::Terminal).map_err(|e| (e, None))?;
        account_for(self.platform.as_ref(), &self.settings, as_root, requester, controller, grant)
    }
}

/// Runs a one-shot command (see `account_for`).
fn exec(gate: &Gate, req: &ExecRequest, grant: &Grant) -> ExecResult {
    let failed = |error, decision| ExecResult {
        request_id: req.request_id,
        error: Some(error),
        root_decision: decision_name(decision),
        ..Default::default()
    };
    let (account, decision) = match gate.account(&req.controller, req.as_root, &req.requester, grant) {
        Ok(r) => r,
        Err((e, decision)) => return failed(e, decision),
    };
    let as_root = account == platform::Account::Root;
    match gate.platform.shell(&req.command, account, Duration::from_millis(req.timeout_ms.into())) {
        Ok(out) => ExecResult {
            request_id: req.request_id,
            exit_code: out.exit_code,
            stdout: out.stdout,
            stderr: out.stderr,
            timed_out: out.timed_out,
            as_root,
            error: None,
            root_decision: decision_name(decision),
        },
        Err(e) => failed(platform_error(format!("{e:#}")), decision),
    }
}

/// A live PTY session and the Controller (display name) that opened it.
struct Shell {
    input: Box<dyn std::io::Write + Send>,
    control: SharedControl,
    controller: String,
}

/// Live PTY sessions of this connection, by session id. Dropping it (connection lost) kills them all.
#[derive(Default)]
struct Shells(std::collections::HashMap<String, Shell>);

type SharedControl = Arc<Mutex<Box<dyn platform::PtyControl>>>;

impl Shells {
    /// Kills the sessions of the Controllers `ended` names (disconnected at the Device, or their terminal switched off).
    fn end_if(&mut self, ended: impl Fn(&str) -> bool) {
        self.0.retain(|_, shell| {
            if !ended(&shell.controller) {
                return true;
            }
            let _ = shell.control.lock().unwrap().kill();
            false
        });
    }
}

impl Drop for Shells {
    fn drop(&mut self) {
        for shell in self.0.values() {
            let _ = shell.control.lock().unwrap().kill();
        }
    }
}

/// Starts a PTY session (see `account_for`) and answers with ShellOpened; a thread then relays its output, then its exit
/// code, through the gate's replies.
fn open_shell(gate: &Gate, req: &ShellOpen, grant: &Grant, shells: &Mutex<Shells>) {
    let opened = |as_root, error, decision| {
        let reply = ShellOpened { request_id: req.request_id, as_root, error, root_decision: decision_name(decision) };
        let _ = gate.replies.send(agent_message::Msg::ShellOpened(reply));
    };
    let (account, decision) = match gate.account(&req.controller, req.as_root, &req.requester, grant) {
        Ok(r) => r,
        Err((e, decision)) => return opened(false, Some(e), decision),
    };
    let as_root = account == platform::Account::Root;
    let pty = match gate.platform.pty(account, clamp_u16(req.cols), clamp_u16(req.rows)) {
        Ok(p) => p,
        Err(e) => return opened(false, Some(platform_error(format!("{e:#}"))), decision),
    };
    let control = Arc::new(Mutex::new(pty.control));
    let shell = Shell { input: pty.input, control: control.clone(), controller: req.controller.clone() };
    shells.lock().unwrap().0.insert(req.session.clone(), shell);
    // Answered before the first output, so the Server sees the session open first.
    opened(as_root, None, decision);
    let (session, replies, mut output) = (req.session.clone(), gate.replies.clone(), pty.output);
    std::thread::spawn(move || {
        let mut buf = vec![0; 16 * 1024];
        while let Ok(n @ 1..) = output.read(&mut buf) {
            let msg = agent_message::Msg::ShellOutput(ShellOutput { session: session.clone(), data: buf[..n].to_vec() });
            if replies.send(msg).is_err() {
                return; // connection gone; `Shells` was dropped and killed the PTY
            }
        }
        let exit_code = control.lock().unwrap().wait().unwrap_or(-1);
        let _ = replies.send(agent_message::Msg::ShellExit(ShellExit { session, exit_code }));
    });
}

fn clamp_u16(n: u32) -> u16 {
    n.clamp(1, u16::MAX.into()) as u16
}

/// What an update task needs, taken from the session that received the `AgentUpdate`.
struct UpdateJob {
    update: AgentUpdate,
    /// Trusts the Server's CA too: the API Address may be served with a certificate from it (ADR 0015).
    http: reqwest::Client,
    /// The build lookup endpoint on the API Address; `None` while no API Address is known.
    url: Option<String>,
    server_key: String,
    /// The Agent's data directory; the download goes to `updates/` in it.
    dir: std::path::PathBuf,
    platform: Arc<dyn Platform>,
    replies: tokio::sync::mpsc::UnboundedSender<agent_message::Msg>,
}

/// Downloads, verifies and installs `job.update`; `true` once installed. A network failure or 5xx retries after a
/// random 30-60 s; anything else is reported to the Server as `UpdateFailed`.
async fn update_agent(job: UpdateJob) -> bool {
    let version = job.update.version.clone();
    loop {
        match download_and_install(&job).await {
            Ok(()) => return true,
            Err(e) if download::transient(&e) => {
                tracing::warn!("Agent update {version} failed, retrying: {e:#}");
                let mut byte = [0u8];
                let _ = ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut byte);
                tokio::time::sleep(Duration::from_secs(30 + u64::from(byte[0]) % 31)).await;
            }
            Err(e) => {
                tracing::warn!("rejected Agent update {version}: {e:#}");
                let _ = job.replies.send(agent_message::Msg::UpdateFailed(UpdateFailed { version, reason: format!("{e:#}") }));
                return false;
            }
        }
    }
}

/// Checks `job.update` against this Agent's target, downloads the build, checks its sha256, then installs it.
async fn download_and_install(job: &UpdateJob) -> anyhow::Result<()> {
    let update = &job.update;
    anyhow::ensure!(update.target == target(), "build is for {}, this Agent is {}", update.target, target());
    let url = job.url.as_deref().ok_or_else(|| anyhow::anyhow!("this Agent knows no API Address to download the build from"))?;
    let build = download::fetch_build(&job.http, url, &job.server_key, &job.dir, |_, _| {}).await?;
    if (build.sha256.as_str(), build.size) != (update.sha256.as_str(), update.size) {
        let _ = tokio::fs::remove_file(&build.path).await;
        anyhow::bail!("the Server served a build ({} bytes, sha256 {}) other than the one it offered", build.size, build.sha256);
    }
    let (platform, version, path) = (job.platform.clone(), build.version, build.path.clone());
    let installed = tokio::task::spawn_blocking(move || platform.install_update(&version, &path)).await?;
    let _ = tokio::fs::remove_file(&build.path).await;
    installed
}

fn sample_resources(platform: &dyn Platform) -> anyhow::Result<Resources> {
    use platform::SessionState as S;
    use rcli_protocol::agent::SessionState as P;
    let mut r = platform.metrics()?;
    let session = match platform.session_state()? {
        S::LoggedIn => P::Active,
        S::Locked => P::LockScreen,
        S::LoginScreen => P::LoginScreen,
    };
    r.session = Some(session.into());
    Ok(r)
}
