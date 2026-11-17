//! Action Router: checks the Control Lock for input Actions, forwards to the Agent, returns the Observation.

use axum::{
    extract::{rejection::JsonRejection, Path, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use rcli_protocol::{
    act::{self, observation},
    agent::{self as proto, agent_message, server_message},
};
use serde_json::{json, Value};

use super::{agent_error, bad_request, devices::resolve_for, ApiError, Caller};
use crate::{agent_link, control_lock};

pub fn routes() -> Router<super::State> {
    Router::new()
        .route("/api/v1/devices/{id}/actions", post(act))
        .route("/api/v1/devices/{id}/displays", get(displays))
}

/// The first Agent version with Wait Conditions; older Agents ignore them and answer at once, as if they were met.
const WAIT_CONDITIONS_SINCE: &str = "0.4.0";

/// `agent_too_old` unless the Device's reported Agent version is semver and at least `WAIT_CONDITIONS_SINCE`
/// (a missing or non-semver version may be an Agent that would ignore the wait).
async fn refuse_agents_without_wait_conditions(state: &super::State, device: uuid::Uuid) -> Result<(), ApiError> {
    let version: Option<String> = sqlx::query_scalar("SELECT agent_version FROM devices WHERE id = $1")
        .bind(device)
        .fetch_one(&state.db)
        .await
        .map_err(anyhow::Error::from)?;
    let supported = version
        .as_deref()
        .is_some_and(|v| semver::Version::parse(v).is_ok() && !crate::updates::below(Some(v), WAIT_CONDITIONS_SINCE));
    if !supported {
        let message = format!(
            "the Device's Agent is {}; Wait Conditions need Agent {WAIT_CONDITIONS_SINCE} or later",
            version.as_deref().unwrap_or("of unknown version")
        );
        return Err(ApiError::new(StatusCode::CONFLICT, "agent_too_old", message));
    }
    Ok(())
}

async fn act(
    Caller(user): Caller,
    State(state): State<super::State>,
    Path(id): Path<String>,
    body: Result<Json<Value>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let device = resolve_for(&user, &state, &id).await?;
    let Json(params) = body.map_err(|e| bad_request(e.body_text()))?;
    let parsed = act::parse(params).map_err(|e| match e {
        act::Refused::BadRequest(m) => bad_request(m),
        act::Refused::Invalid(m) => ApiError::new(StatusCode::BAD_REQUEST, "invalid_action", m),
    })?;
    if parsed.has_wait_condition {
        refuse_agents_without_wait_conditions(&state, device).await?;
    }
    // Every request from the holder, observe-only included, restarts the idle timer.
    control_lock::touch(&state, device, &user).await?;
    if !parsed.act.actions.is_empty() {
        control_lock::check(&state, device, &user).await?;
    }
    let req = proto::ActRequest { controller: user.controller_display(), ..parsed.act };
    send(&state, device, req, &parsed.step_of, parsed.waits).await
}

/// Sends `req` (its request_id is filled in) and returns the Observation JSON.
/// `step_of[i]` = the API step of protocol Action `i`, reported as `step` on failure. `waits` lengthens the timeout.
async fn send(
    state: &super::State,
    device: uuid::Uuid,
    req: proto::ActRequest,
    step_of: &[u32],
    waits: std::time::Duration,
) -> Result<Json<Value>, ApiError> {
    let req = |request_id| server_message::Msg::Act(proto::ActRequest { request_id, ..req });
    let timeout = agent_link::REQUEST_TIMEOUT + agent_link::PROMPT_BUDGET + waits;
    let agent_message::Msg::ActResult(mut result) = state.agents.request_within(device, timeout, req).await? else {
        return Err(anyhow::anyhow!("unexpected reply to ActRequest").into());
    };
    if let Some(e) = result.error.take() {
        let mut err = agent_error(e);
        if let Some(step) = result.failed_step.and_then(|s| step_of.get(s as usize)) {
            err = err.with("step", *step);
        }
        // A timed-out Wait Condition still shows the screen as it was.
        if err.code == "wait_timeout" {
            err.details.extend(observation(result).as_object().cloned().unwrap_or_default());
        }
        return Err(err);
    }
    Ok(Json(observation(result)))
}

async fn displays(Caller(user): Caller, State(state): State<super::State>, Path(id): Path<String>) -> Result<Json<Value>, ApiError> {
    let device = resolve_for(&user, &state, &id).await?;
    let req = |request_id| server_message::Msg::ListDisplays(proto::DisplaysRequest { request_id });
    let agent_message::Msg::Displays(list) = state.agents.request(device, req).await? else {
        return Err(anyhow::anyhow!("unexpected reply to DisplaysRequest").into());
    };
    if let Some(e) = list.error {
        return Err(agent_error(e));
    }
    let displays: Vec<Value> = list
        .displays
        .into_iter()
        .map(|d| {
            json!({ "index": d.index, "x": d.x, "y": d.y, "width": d.width, "height": d.height, "scale": d.scale, "primary": d.primary })
        })
        .collect();
    Ok(Json(json!(displays)))
}
