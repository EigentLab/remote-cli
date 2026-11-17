//! Device Registry: Devices and Device Status.

use std::{collections::HashMap, sync::Mutex};

use chrono::{DateTime, Duration, Utc};
use serde::Serialize;
use uuid::Uuid;

use crate::AppState;

/// A Device stays Online this long after its connection drops.
pub const OFFLINE_AFTER: Duration = Duration::seconds(30);

/// Live Agent connections per Device (a count, so an overlapping reconnect is not a drop).
#[derive(Default)]
pub struct Connections(Mutex<HashMap<Uuid, usize>>);

#[derive(Serialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceStatus {
    Online,
    Offline,
}

#[derive(Serialize, Debug)]
pub struct Device {
    pub id: Uuid,
    /// 9-digit Device ID; the CLI and Dashboard address a Device by it.
    pub device_id: String,
    /// The caller's name for the Device (their Address Book entry's); the Device ID until one is given.
    pub name: String,
    pub status: DeviceStatus,
    /// When the Agent's last connection ended; null while connected or never connected.
    pub last_seen_at: Option<DateTime<Utc>>,
    /// Version the Agent reported on its last connection; null until it connected once.
    pub agent_version: Option<String>,
    /// A Minimum Version is set and `agent_version` is below it.
    pub needs_update: bool,
}

fn status(connected: bool, last_seen_at: Option<DateTime<Utc>>, now: DateTime<Utc>) -> DeviceStatus {
    match (connected, last_seen_at) {
        (true, _) => DeviceStatus::Online,
        (false, Some(t)) if now - t < OFFLINE_AFTER => DeviceStatus::Online,
        _ => DeviceStatus::Offline,
    }
}

pub async fn get(state: &AppState, id: Uuid) -> anyhow::Result<Device> {
    fetch(state, &[id]).await?.pop().ok_or_else(|| anyhow::anyhow!("device {id} vanished"))
}

/// The Devices `ids` that exist, in no particular order.
pub async fn fetch(state: &AppState, ids: &[Uuid]) -> anyhow::Result<Vec<Device>> {
    let rows: Vec<(Uuid, String, Option<DateTime<Utc>>, Option<String>)> =
        sqlx::query_as("SELECT id, device_number, last_seen_at, agent_version FROM devices WHERE id = ANY($1)")
            .bind(ids)
            .fetch_all(&state.db)
            .await?;
    let now = state.clock.now();
    let minimum = state.settings.minimum_version().await;
    let connected = state.connections.0.lock().unwrap();
    Ok(rows
        .into_iter()
        .map(|(id, device_id, last_seen_at, agent_version)| Device {
            id,
            name: device_id.clone(),
            device_id,
            status: status(connected.contains_key(&id), last_seen_at, now),
            last_seen_at,
            needs_update: crate::updates::below(agent_version.as_deref(), &minimum),
            agent_version,
        })
        .collect())
}

/// Finds a Device by its 9-digit Device ID or its UUID.
pub async fn resolve(state: &AppState, key: &str) -> anyhow::Result<Option<Uuid>> {
    Ok(sqlx::query_scalar("SELECT id FROM devices WHERE device_number = $1 OR id::text = $1")
        .bind(key)
        .fetch_optional(&state.db)
        .await?)
}

/// The Device holding this certificate.
pub async fn find_by_cert_fingerprint(state: &AppState, fp: &[u8]) -> anyhow::Result<Option<Uuid>> {
    Ok(sqlx::query_scalar("SELECT id FROM devices WHERE cert_fingerprint = $1")
        .bind(fp)
        .fetch_optional(&state.db)
        .await?)
}

/// Changes the Device ID; `Err` with a message if it is not 9 digits or another Device has it.
pub async fn set_device_number(state: &AppState, id: Uuid, number: &str) -> anyhow::Result<Result<(), String>> {
    if number.len() != 9 || !number.chars().all(|c| c.is_ascii_digit()) {
        return Ok(Err("the Device ID must be 9 digits".into()));
    }
    let taken: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM devices WHERE device_number = $1 AND id <> $2)")
        .bind(number)
        .bind(id)
        .fetch_one(&state.db)
        .await?;
    if taken {
        return Ok(Err(format!("Device ID {number} is already used by another Device")));
    }
    sqlx::query("UPDATE devices SET device_number = $2 WHERE id = $1").bind(id).bind(number).execute(&state.db).await?;
    Ok(Ok(()))
}

/// Stores a new Device Password hash; existing connections are unaffected (they were authenticated already). The
/// Device leaves every User's Address Book: only people with the new password reach it (they add it again).
pub async fn set_device_password(state: &AppState, id: Uuid, password: &str) -> anyhow::Result<()> {
    let hash = rcli_protocol::password::hash(password);
    sqlx::query("UPDATE devices SET device_password_hash = $2 WHERE id = $1").bind(id).bind(hash).execute(&state.db).await?;
    sqlx::query("DELETE FROM address_book WHERE device = $1").bind(id).execute(&state.db).await?;
    Ok(())
}

/// Deletes the Device (it uninstalled itself) and drops its connection; its certificate no longer gets in.
pub async fn delete(state: &AppState, id: Uuid) -> anyhow::Result<()> {
    sqlx::query("DELETE FROM devices WHERE id = $1").bind(id).execute(&state.db).await?;
    state.agents.disconnect(id, "device_deleted", "this Device was deleted");
    Ok(())
}

pub async fn agent_connected(state: &AppState, id: Uuid) -> anyhow::Result<()> {
    *state.connections.0.lock().unwrap().entry(id).or_default() += 1;
    sqlx::query("UPDATE devices SET last_seen_at = NULL WHERE id = $1").bind(id).execute(&state.db).await?;
    Ok(())
}

pub async fn agent_disconnected(state: &AppState, id: Uuid) -> anyhow::Result<()> {
    let last = {
        let mut map = state.connections.0.lock().unwrap();
        let n = map.entry(id).or_default();
        *n = n.saturating_sub(1);
        let last = *n == 0;
        if last {
            map.remove(&id);
        }
        last
    };
    if last {
        sqlx::query("UPDATE devices SET last_seen_at = $2 WHERE id = $1")
            .bind(id)
            .bind(state.clock.now())
            .execute(&state.db)
            .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn online_for_30_seconds_after_drop_then_offline() {
        let t = Utc::now();
        assert_eq!(status(true, None, t), DeviceStatus::Online);
        assert_eq!(status(false, None, t), DeviceStatus::Offline);
        assert_eq!(status(false, Some(t), t + Duration::seconds(29)), DeviceStatus::Online);
        assert_eq!(status(false, Some(t), t + Duration::seconds(30)), DeviceStatus::Offline);
    }
}
