//! Messages shared by Server and Agent, plus the framing used on the Agent connection.

pub mod agent {
    include!(concat!(env!("OUT_DIR"), "/rcli.agent.v1.rs"));
}

pub mod act;
pub mod password;
pub mod server_key;
pub mod update;

/// One line telling a Controller how to fix a request that failed with error `code`.
pub fn hint(code: &str) -> &'static str {
    match code {
        "invalid_action" => "fix the field the message names: points are pixels of the Observation image, scroll dx/dy are wheel notches (at most 50)",
        "stale_mark" => "observe again with marks=true and target a Mark from that Observation",
        "ambiguous_text" => "target one of the listed Marks by number, or add a role to the text target",
        "text_not_found" => "observe with marks=true to see the text on screen, then target a Mark by number",
        "no_display" => "the Device has no display: use exec or shell instead of Actions",
        "no_accessibility_tree" => "this screen is read by OCR: wait with appears/disappears instead of value/state",
        "wait_timeout" => "look at the returned Observation, then raise timeout_ms or change the Wait Condition",
        "root_denied" => "run without root, or ask the Device Owner to allow root",
        "connect_denied" => "ask the person at the Device to accept the connection, then retry",
        "service_mode_required" => "run as the Device's own user without root, or ask the Device Owner to switch to Service Mode",
        "capability_off" => "ask the Device Owner to switch the capability on in Device Settings",
        "no_logged_in_user" => "nobody is logged in on the Device: log in at the login screen first",
        "invalid_offer" => "send a fresh WebRTC offer",
        "control_lock_required" => "call lock_acquire (rcli lock acquire) for this Device first",
        "control_lock_held" => "wait until the holder releases the Control Lock or it expires, then acquire it again",
        "device_not_connected" => "the Device is offline: wake it or wait until it is Online, then retry",
        "device_disconnected" => "the Device went offline: wait until it is Online, then start again",
        "agent_timeout" => "retry; split a long batch into shorter ones",
        "agent_too_old" => "update the Device's Agent, or leave out what the message names",
        "controller_too_slow" => "read the shell output faster, or send commands that print less",
        "no_wake_address" => "the Address Book learns the MAC and subnet while the Device is Online (the CLI on rcli connect or rcli devices): power it on by hand once, then Wake works",
        "device_not_found" => "list Devices (rcli devices) and use one of their names or IDs",
        "ambiguous_device" => "use the Device ID instead of the name",
        "forbidden_device" => "connect to the other Device with its Device ID and Device Password first",
        "unauthorized" | "invalid_credentials" => "sign in again (rcli login) or reconnect with the Device Password",
        "totp_required" => "this User turned TOTP on: sign in again with the code (rcli login --totp <code>)",
        "invalid_code" => "type the current 6-digit code from the authenticator app that scanned this secret",
        "totp_on" => "turn TOTP off with a current code first, then scan the new secret",
        "bad_request" => "fix the request body as the message says",
        "agent_error" | "platform" => "the Device's OS refused: observe to see its state, then retry or try another way",
        _ => "read the message, fix the request and retry",
    }
}

/// `Resources.session` as "Active" | "LockScreen" | "LoginScreen" | null in JSON.
fn serialize_session<S: serde::Serializer>(v: &Option<i32>, s: S) -> Result<S::Ok, S::Error> {
    use agent::SessionState::*;
    let name = match v.and_then(|v| agent::SessionState::try_from(v).ok()) {
        Some(Active) => Some("Active"),
        Some(LockScreen) => Some("LockScreen"),
        Some(LoginScreen) => Some("LoginScreen"),
        Some(Unspecified) | None => None,
    };
    s.serialize_some(&name)
}

use futures::{SinkExt, StreamExt};
use prost::Message;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_util::codec::{Framed, LengthDelimitedCodec};

/// A framed protobuf connection: sends `S`, receives `R`.
pub struct Conn<T> {
    inner: Framed<T, LengthDelimitedCodec>,
}

impl<T: AsyncRead + AsyncWrite + Unpin> Conn<T> {
    pub fn new(io: T) -> Self {
        Self { inner: Framed::new(io, LengthDelimitedCodec::new()) }
    }

    pub async fn send<S: Message>(&mut self, msg: &S) -> anyhow::Result<()> {
        self.inner.send(bytes::Bytes::from(msg.encode_to_vec())).await?;
        Ok(())
    }

    /// `Ok(None)` when the peer closed the connection.
    pub async fn recv<R: Message + Default>(&mut self) -> anyhow::Result<Option<R>> {
        match self.inner.next().await {
            None => Ok(None),
            Some(frame) => Ok(Some(R::decode(frame?)?)),
        }
    }
}
