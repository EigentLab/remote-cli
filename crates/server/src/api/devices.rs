use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::get,
    Json, Router,
};
use rcli_protocol::agent::{agent_message, server_message, DeviceInfo, DeviceInfoRequest, Inventory, Process, ProcessListRequest, Resources};
use serde::Serialize;
use uuid::Uuid;

use super::{ApiError, Caller};
use crate::registry;

pub fn routes() -> Router<super::State> {
    Router::new()
        .route("/api/v1/devices/{device}", get(show))
        .route("/api/v1/devices/{device}/processes", get(processes))
}

/// `{device}` (a Device ID or UUID, or a signed-in User's own name for it) as the caller may reach it: a password
/// connection only its own Device, a signed-in User only Devices in their Address Book (ADR 0011). Any other Device
/// is "not found", so none is revealed.
pub(super) async fn resolve_for(user: &crate::auth::User, state: &super::State, key: &str) -> Result<Uuid, ApiError> {
    let not_found = || ApiError::new(StatusCode::NOT_FOUND, "device_not_found", format!("no Device {key:?}"));
    match user.device_scope {
        Some(own) => match registry::resolve(state, key).await? {
            Some(id) if id == own => Ok(id),
            Some(_) => Err(ApiError::new(StatusCode::FORBIDDEN, "forbidden_device", "a password connection reaches only its own Device")),
            None => Err(not_found()),
        },
        None => {
            let ids: Vec<Uuid> = sqlx::query_scalar(&format!(
                "SELECT device FROM address_book WHERE {} OR (user_id = $1 AND name = $2) LIMIT 2",
                super::address_book::ENTRY
            ))
            .bind(user.id)
            .bind(key)
            .fetch_all(&state.db)
            .await
            .map_err(anyhow::Error::from)?;
            match ids[..] {
                [] => Err(not_found()),
                [id] => Ok(id),
                _ => Err(ApiError::new(StatusCode::CONFLICT, "ambiguous_device", format!("more than one Device is named {key:?}; use its Device ID"))),
            }
        }
    }
}

#[derive(Serialize)]
struct DeviceDetails {
    #[serde(flatten)]
    device: registry::Device,
    /// Read from the Agent when asked; null unless the Device is Online and answered.
    inventory: Option<Inventory>,
    /// Read from the Agent when asked; null unless the Device is Online and answered.
    resources: Option<Resources>,
    /// The MAC and subnet an Online Device's Agent reports, for the caller's Address Book to wake it later.
    wake: Option<crate::wake::Address>,
}

async fn show(
    Caller(user): Caller,
    State(state): State<super::State>,
    Path(key): Path<String>,
) -> Result<Json<DeviceDetails>, ApiError> {
    let id = resolve_for(&user, &state, &key).await?;
    let mut device = registry::get(&state, id).await?;
    if let Some(name) = super::address_book::name(&state, &user, id).await? {
        device.name = name;
    }
    let (inventory, resources) = match device.status {
        registry::DeviceStatus::Online => match device_info(&state, id).await {
            Ok(info) => (info.inventory, info.resources),
            // The page still shows the Device; its numbers are simply missing this time.
            Err(e) => {
                tracing::warn!(device = %id, "cannot read device info: {}", e.message);
                (None, None)
            }
        },
        _ => (None, None),
    };
    let wake = crate::metrics::wake_address(&state, id);
    Ok(Json(DeviceDetails { inventory, resources, wake, device }))
}

/// Asks the Agent for its inventory and current resources now.
async fn device_info(state: &super::State, id: Uuid) -> Result<DeviceInfo, ApiError> {
    let req = |request_id| server_message::Msg::DeviceInfo(DeviceInfoRequest { request_id });
    match state.agents.request(id, req).await? {
        agent_message::Msg::DeviceInfo(i) if i.error.is_empty() => Ok(i),
        agent_message::Msg::DeviceInfo(i) => Err(ApiError::new(StatusCode::BAD_GATEWAY, "agent_error", i.error)),
        _ => Err(anyhow::anyhow!("unexpected reply to DeviceInfo").into()),
    }
}

/// Asks the Agent for its process list now; never streamed.
async fn processes(
    Caller(user): Caller,
    State(state): State<super::State>,
    Path(key): Path<String>,
) -> Result<Json<Vec<Process>>, ApiError> {
    let id = resolve_for(&user, &state, &key).await?;
    let req = |request_id| server_message::Msg::ListProcesses(ProcessListRequest { request_id });
    match state.agents.request(id, req).await? {
        agent_message::Msg::Processes(p) if p.error.is_empty() => Ok(Json(p.processes)),
        agent_message::Msg::Processes(p) => Err(ApiError::new(StatusCode::BAD_GATEWAY, "agent_error", p.error)),
        _ => Err(anyhow::anyhow!("unexpected reply to ListProcesses").into()),
    }
}
