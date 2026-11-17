//! The inventory each connected Agent reports, held in memory only: Wake Relays are picked from it (ADR 0011).

use std::{collections::HashMap, sync::Mutex};

use rcli_protocol::agent::Inventory;
use uuid::Uuid;

use crate::AppState;

/// The latest inventory of each connected Agent, in memory only: the Server picks Wake Relays from it (ADR 0011).
#[derive(Default)]
pub struct OnlineInventories(Mutex<HashMap<Uuid, Inventory>>);

/// Holds a connected Agent's inventory, and records its MAC and subnet in every Address Book entry for the Device
/// (the Dashboard's "last connection").
pub async fn hold_inventory(state: &AppState, device: Uuid, inventory: &Inventory) -> anyhow::Result<()> {
    state.inventories.0.lock().unwrap().insert(device, inventory.clone());
    if let Some(a) = crate::wake::address(&inventory.network_interfaces) {
        sqlx::query("UPDATE address_book SET mac = $2, subnet = $3 WHERE device = $1")
            .bind(device)
            .bind(a.mac)
            .bind(a.subnet)
            .execute(&state.db)
            .await?;
    }
    Ok(())
}

/// The Agent disconnected: its inventory is no longer held.
pub fn drop_inventory(state: &AppState, device: Uuid) {
    state.inventories.0.lock().unwrap().remove(&device);
}

/// Every connected Agent's held inventory.
pub fn online_inventories(state: &AppState) -> Vec<(Uuid, Inventory)> {
    state.inventories.0.lock().unwrap().iter().map(|(id, inv)| (*id, inv.clone())).collect()
}

/// The MAC and subnet a connected Agent reports, if any.
pub fn wake_address(state: &AppState, device: Uuid) -> Option<crate::wake::Address> {
    state.inventories.0.lock().unwrap().get(&device).and_then(|inv| crate::wake::address(&inv.network_interfaces))
}
