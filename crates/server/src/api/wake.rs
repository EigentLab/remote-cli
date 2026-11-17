use axum::{
    body::Bytes,
    extract::{Path, State},
    http::StatusCode,
    routing::post,
    Json, Router,
};
use rcli_protocol::agent::{agent_message, server_message, WakeRequest};
use serde_json::{json, Value};
use uuid::Uuid;

use super::{bad_request, devices::resolve_for, ApiError, Caller};
use crate::{auth::User, metrics, wake};

pub fn routes() -> Router<super::State> {
    Router::new().route("/api/v1/devices/{device}/wake", post(wake))
}

/// Wakes an Offline Device through a Wake Relay; -> `{relay: {id, name}, mac}`. The body's
/// `{mac, subnet}` come from the caller's Address Book; a signed-in User without them gets those of their own entry.
async fn wake(
    Caller(user): Caller,
    State(state): State<super::State>,
    Path(key): Path<String>,
    body: Bytes,
) -> Result<Json<Value>, ApiError> {
    let id = resolve_for(&user, &state, &key).await?;
    let bad = |e: serde_json::Error| bad_request(format!("expected no body or {{\"mac\", \"subnet\"}}: {e}"));
    let given = match body.is_empty() {
        true => Value::Null,
        false => serde_json::from_slice(&body).map_err(bad)?,
    };
    let given: Option<wake::Address> = match given {
        Value::Null => None,
        Value::Object(m) if m.is_empty() => None,
        v => Some(serde_json::from_value(v).map_err(bad)?),
    };
    if state.agents.connected().contains(&id) {
        return Err(ApiError::new(StatusCode::CONFLICT, "device_online", format!("Device {key:?} is Online")));
    }
    let address = match given {
        Some(a) => Some(a),
        None => in_address_book(&state, &user, id).await?,
    };
    let Some(address) = address else {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "no_wake_address",
            format!("the MAC and subnet of Device {key:?} are unknown: it has not been Online since it was added to this Address Book"),
        ));
    };
    send(&state, id, &key, &address).await.map(Json)
}

/// The MAC and subnet in a signed-in User's own Address Book entry (a password connection has none on the Server).
async fn in_address_book(state: &super::State, user: &User, device: Uuid) -> Result<Option<wake::Address>, ApiError> {
    if user.device_scope.is_some() {
        return Ok(None);
    }
    let row: Option<(Option<String>, Option<String>)> =
        sqlx::query_as("SELECT mac, subnet FROM address_book WHERE user_id = $1 AND device = $2")
            .bind(user.id)
            .bind(device)
            .fetch_optional(&state.db)
            .await
            .map_err(anyhow::Error::from)?;
    Ok(row.and_then(|(mac, subnet)| Some(wake::Address { mac: mac?, subnet: subnet? })))
}

/// Tries every Online Agent on the target's subnet, as the Server holds them in memory, until one sends the packet.
async fn send(state: &super::State, target: Uuid, name: &str, address: &wake::Address) -> Result<Value, ApiError> {
    let online = metrics::online_inventories(state);
    let relays: Vec<(Uuid, &[_])> =
        online.iter().filter(|(id, _)| *id != target).map(|(id, inv)| (*id, inv.network_interfaces.as_slice())).collect();
    wake::parse(address).map_err(bad_request)?;
    let (mac, routes) =
        wake::routes(name, address, &relays).map_err(|m| ApiError::new(StatusCode::CONFLICT, "no_wake_relay", m))?;
    let mut failures = vec![];
    for relay in routes {
        let relay_name = online.iter().find(|(id, _)| *id == relay).map(|(_, inv)| inv.hostname.clone()).unwrap_or_default();
        let req = |request_id| server_message::Msg::Wake(WakeRequest { request_id, mac: mac.to_vec() });
        match state.agents.request(relay, req).await {
            Ok(agent_message::Msg::WakeResult(r)) if r.error.is_none() => {
                let relay = json!({ "id": relay, "name": relay_name });
                return Ok(json!({ "relay": relay, "mac": wake::format_mac(mac) }));
            }
            Ok(agent_message::Msg::WakeResult(r)) => {
                failures.push(format!("{relay_name}: {}", r.error.map(|e| e.message).unwrap_or_default()))
            }
            Ok(_) => failures.push(format!("{relay_name}: unexpected reply")),
            Err(e) => failures.push(format!("{relay_name}: {e:?}")),
        }
    }
    let message = format!("no Wake Relay could send the packet ({})", failures.join("; "));
    Err(ApiError::new(StatusCode::BAD_GATEWAY, "wake_relay_failed", message))
}
