//! A signed-in User's Address Book: the Devices they added by Device ID and Device Password (ADR 0011).
//! Each entry carries the User's own name for the Device; the password is sealed with the master key.

use axum::{
    extract::{rejection::JsonRejection, Path, State},
    http::StatusCode,
    routing::{get, patch},
    Json, Router,
};
use serde::Deserialize;
use uuid::Uuid;

use super::{bad_request, ApiError, ClientIp, SignedIn};
use crate::{auth::User, metrics, registry, wake};

pub fn routes() -> Router<super::State> {
    Router::new()
        .route("/api/v1/address-book", get(list).post(add))
        .route("/api/v1/address-book/{device}", patch(rename).delete(remove))
}

/// The User's Devices as `registry::Device`s, each under the name the User gave it.
async fn list(SignedIn(user): SignedIn, State(state): State<super::State>) -> Result<Json<Vec<registry::Device>>, ApiError> {
    let names: std::collections::HashMap<Uuid, String> =
        sqlx::query_as::<_, (Uuid, String)>("SELECT device, name FROM address_book WHERE user_id = $1")
            .bind(user.id)
            .fetch_all(&state.db)
            .await
            .map_err(anyhow::Error::from)?
            .into_iter()
            .collect();
    let mut devices = registry::fetch(&state, &names.keys().copied().collect::<Vec<_>>()).await?;
    devices.iter_mut().for_each(|d| d.name = names[&d.id].clone());
    devices.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(Json(devices))
}

#[derive(Deserialize)]
struct Add {
    device_id: String,
    password: String,
    /// Defaults to the Device ID.
    name: Option<String>,
    /// The Device's MAC and subnet, for an Offline Device another Address Book already knows; an Online Device's
    /// own report wins.
    #[serde(flatten)]
    wake: Option<wake::Address>,
}

/// Adds (or updates) a Device after checking its Device Password exactly as a password connection does, wrong
/// passwords counted against the same per-(Device, IP) limit.
async fn add(
    SignedIn(user): SignedIn,
    State(state): State<super::State>,
    ClientIp(ip): ClientIp,
    body: Result<Json<Add>, JsonRejection>,
) -> Result<(StatusCode, Json<registry::Device>), ApiError> {
    let Json(body) = body.map_err(|e| bad_request(e.body_text()))?;
    let name = valid_name(body.name.as_deref().unwrap_or(&body.device_id))?;
    let key = state.settings.key().await;
    let checked = crate::auth::authenticate_controller(&state, &key, &body.device_id, &body.password, &user.name, &ip).await?;
    let id = checked.map_err(|e| super::controller_refused(e, &state))?.device_scope.expect("a password connection has its Device");
    if let Some(given) = &body.wake {
        wake::parse(given).map_err(bad_request)?;
    }
    let address = metrics::wake_address(&state, id).or(body.wake);
    let (password, sealed) = match &state.config.master_key {
        Some(k) => (k.seal_bytes(&aad(user.id, id), body.password.as_bytes())?, true),
        None => (body.password.into_bytes(), false),
    };
    sqlx::query(
        "INSERT INTO address_book (user_id, device, name, password, password_sealed, added_at, mac, subnet)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
         ON CONFLICT (user_id, device) DO UPDATE SET name = $3, password = $4, password_sealed = $5,
           mac = COALESCE($7, address_book.mac), subnet = COALESCE($8, address_book.subnet)",
    )
    .bind(user.id)
    .bind(id)
    .bind(&name)
    .bind(password)
    .bind(sealed)
    .bind(state.clock.now())
    .bind(address.as_ref().map(|a| &a.mac))
    .bind(address.as_ref().map(|a| &a.subnet))
    .execute(&state.db)
    .await
    .map_err(anyhow::Error::from)?;
    let mut device = registry::get(&state, id).await?;
    device.name = name;
    Ok((StatusCode::CREATED, Json(device)))
}

#[derive(Deserialize)]
struct Rename {
    name: String,
}

async fn rename(
    SignedIn(user): SignedIn,
    State(state): State<super::State>,
    Path(key): Path<String>,
    body: Result<Json<Rename>, JsonRejection>,
) -> Result<Json<registry::Device>, ApiError> {
    let Json(body) = body.map_err(|e| bad_request(e.body_text()))?;
    let name = valid_name(&body.name)?;
    let id: Option<Uuid> = sqlx::query_scalar(&format!("UPDATE address_book SET name = $3 WHERE {ENTRY} RETURNING device"))
        .bind(user.id)
        .bind(&key)
        .bind(&name)
        .fetch_optional(&state.db)
        .await
        .map_err(anyhow::Error::from)?;
    let mut device = registry::get(&state, id.ok_or_else(|| not_in_book(&key))?).await?;
    device.name = name;
    Ok(Json(device))
}

/// Removes the entry and so its password; the Device itself is untouched.
async fn remove(SignedIn(user): SignedIn, State(state): State<super::State>, Path(key): Path<String>) -> Result<StatusCode, ApiError> {
    let removed = sqlx::query(&format!("DELETE FROM address_book WHERE {ENTRY}"))
        .bind(user.id)
        .bind(&key)
        .execute(&state.db)
        .await
        .map_err(anyhow::Error::from)?;
    match removed.rows_affected() {
        0 => Err(not_in_book(&key)),
        _ => Ok(StatusCode::NO_CONTENT),
    }
}

/// The caller's (`$1`) entry for `$2`, a Device ID or the Device's UUID.
pub(super) const ENTRY: &str =
    "user_id = $1 AND device IN (SELECT id FROM devices WHERE device_number = $2 OR id::text = $2)";

/// The name a signed-in User gave `device` in their Address Book; None for a password connection.
pub(super) async fn name(state: &super::State, user: &User, device: Uuid) -> anyhow::Result<Option<String>> {
    Ok(sqlx::query_scalar("SELECT name FROM address_book WHERE user_id = $1 AND device = $2")
        .bind(user.id)
        .bind(device)
        .fetch_optional(&state.db)
        .await?
        .filter(|_| user.device_scope.is_none()))
}

fn not_in_book(key: &str) -> ApiError {
    ApiError::new(StatusCode::NOT_FOUND, "device_not_found", format!("no Device {key:?} in your Address Book"))
}

fn valid_name(name: &str) -> Result<String, ApiError> {
    let name = name.trim();
    match name.chars().count() {
        1..=255 => Ok(name.to_owned()),
        _ => Err(bad_request("name must be 1-255 characters")),
    }
}

/// Binds a sealed password to its entry: it opens only for the same User and Device.
fn aad(user: Uuid, device: Uuid) -> Vec<u8> {
    [user.as_bytes().as_slice(), device.as_bytes()].concat()
}

/// Seals the passwords stored while the Server had no master key.
pub(crate) async fn seal_passwords(db: &sqlx::PgPool, key: &crate::master_key::MasterKey) -> anyhow::Result<()> {
    let plain: Vec<(Uuid, Uuid, Vec<u8>)> =
        sqlx::query_as("SELECT user_id, device, password FROM address_book WHERE NOT password_sealed").fetch_all(db).await?;
    for (user, device, password) in plain {
        sqlx::query("UPDATE address_book SET password = $3, password_sealed = TRUE WHERE user_id = $1 AND device = $2 AND NOT password_sealed")
            .bind(user)
            .bind(device)
            .bind(key.seal_bytes(&aad(user, device), &password)?)
            .execute(db)
            .await?;
    }
    Ok(())
}
