//! Dashboard sign-in (password, plus TOTP when the User turned it on), the signed-in user, and their TOTP switch.

use axum::{
    extract::{rejection::JsonRejection, State},
    http::{HeaderMap, StatusCode},
    routing::{delete, get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};

use super::{bad_request, ApiError, Caller, SignedIn};
use crate::auth::{self, User};

pub fn routes() -> Router<super::State> {
    Router::new()
        .route("/api/v1/sessions", post(sign_in))
        .route("/api/v1/session", delete(sign_out))
        .route("/api/v1/me", get(me))
        .route("/api/v1/me/totp", get(totp_state).post(new_totp).put(turn_on_totp).delete(turn_off_totp))
}

#[derive(Deserialize)]
struct SignIn {
    name: String,
    password: String,
    /// Needed only when the User turned TOTP on.
    #[serde(default)]
    totp: String,
}

async fn sign_in(
    State(state): State<super::State>,
    body: Result<Json<SignIn>, JsonRejection>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let Json(b) = body.map_err(|e| bad_request(e.body_text()))?;
    let (user, token) = auth::sign_in(&state, &b.name, &b.password, b.totp.trim()).await?.map_err(|e| match e {
        auth::SignInError::Invalid => {
            ApiError::new(StatusCode::UNAUTHORIZED, "invalid_credentials", "wrong name, password or TOTP code")
        }
        auth::SignInError::TotpRequired => ApiError::new(
            StatusCode::UNAUTHORIZED,
            "totp_required",
            "TOTP is on for this User: send the code from the authenticator app",
        ),
        auth::SignInError::Locked(until) => ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "too_many_attempts",
            format!("too many failed sign-ins; try again after {until}"),
        )
        .with("locked_until", until.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)),
    })?;
    let expires_at = state.clock.now() + auth::SESSION_TTL;
    Ok((StatusCode::CREATED, Json(json!({ "token": token, "user": user, "expires_at": expires_at }))))
}

async fn sign_out(_: Caller, State(state): State<super::State>, headers: HeaderMap) -> Result<StatusCode, ApiError> {
    let token = headers.get("authorization").and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("Bearer "));
    auth::revoke_by_token(&state, token.unwrap_or_default()).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn me(Caller(user): Caller) -> Json<User> {
    Json(user)
}

async fn totp_state(SignedIn(user): SignedIn, State(state): State<super::State>) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!({ "on": auth::totp_on(&state, &user).await? })))
}

/// A fresh secret to scan; TOTP stays as it is until a `PUT` confirms it with a code.
async fn new_totp(SignedIn(user): SignedIn) -> Json<Value> {
    let t = auth::new_totp(&user);
    Json(json!({ "totp_secret": t.secret, "otpauth_url": t.url }))
}

#[derive(Deserialize)]
struct TurnOn {
    secret: String,
    code: String,
}

async fn turn_on_totp(
    SignedIn(user): SignedIn,
    State(state): State<super::State>,
    body: Result<Json<TurnOn>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    let Json(b) = body.map_err(|e| bad_request(e.body_text()))?;
    // Replacing the secret would drop TOTP without a code: turning it off asks for one.
    if auth::totp_on(&state, &user).await? {
        return Err(ApiError::new(StatusCode::CONFLICT, "totp_on", "TOTP is on: turn it off with a current code first"));
    }
    auth::turn_on_totp(&state, &user, b.secret.trim(), b.code.trim()).await.map_err(invalid_code)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct TurnOff {
    code: String,
}

async fn turn_off_totp(
    SignedIn(user): SignedIn,
    State(state): State<super::State>,
    body: Result<Json<TurnOff>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    let Json(b) = body.map_err(|e| bad_request(e.body_text()))?;
    auth::turn_off_totp(&state, &user, b.code.trim()).await.map_err(invalid_code)?;
    Ok(StatusCode::NO_CONTENT)
}

fn invalid_code(e: auth::Error) -> ApiError {
    match e {
        auth::Error::BadRequest(m) => ApiError::new(StatusCode::BAD_REQUEST, "invalid_code", m),
        e => e.into(),
    }
}
