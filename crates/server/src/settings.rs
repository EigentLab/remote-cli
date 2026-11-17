//! Server Settings: the Server Key, the Server Address, the API Address and the Minimum Version, stored in Postgres and edited by Users.

use sqlx::PgPool;
use tokio::sync::RwLock;

use crate::{pki::Ca, server_key};

/// Current Server Settings, kept in memory and backed by Postgres.
pub struct Settings {
    key: RwLock<String>,
    address: RwLock<String>,
    /// Empty until learned from a User's Dashboard visit or set by a User.
    api_address: RwLock<String>,
    /// Empty when none is set.
    minimum_version: RwLock<String>,
}

impl Settings {
    /// Loads the settings. The address the install gives (`RCLI_AGENT_HOSTNAMES`) is applied when it differs from the
    /// one the last start applied, as the API Address is, so a User's correction stands until the install changes it.
    pub async fn load(db: &PgPool, ca: &Ca, install_address: &str) -> anyhow::Result<Self> {
        let key = server_key::load_or_create(db, ca).await?;
        // A Server from before (NULL) records it without replacing an address its Users set.
        sqlx::query(
            "UPDATE server_ca SET install_server_address = $1, server_address = CASE \
             WHEN server_address = '' OR install_server_address IS NOT NULL THEN $1 ELSE server_address END \
             WHERE install_server_address IS DISTINCT FROM $1",
        )
        .bind(install_address)
        .execute(db)
        .await?;
        let (address, api_address, minimum_version): (String, String, String) =
            sqlx::query_as("SELECT server_address, api_address, minimum_version FROM server_ca").fetch_one(db).await?;
        Ok(Self {
            key: RwLock::new(key),
            address: RwLock::new(address),
            api_address: RwLock::new(api_address),
            minimum_version: RwLock::new(minimum_version),
        })
    }

    pub async fn key(&self) -> String {
        self.key.read().await.clone()
    }

    pub async fn address(&self) -> String {
        self.address.read().await.clone()
    }

    /// A new secret for the same CA: registered Devices keep their certificates.
    pub async fn rotate_key(&self, db: &PgPool, ca: &Ca) -> anyhow::Result<String> {
        let key = server_key::rotate(db, ca).await?;
        *self.key.write().await = key.clone();
        Ok(key)
    }

    pub async fn api_address(&self) -> String {
        self.api_address.read().await.clone()
    }

    /// Sets the API Address; a `learned` one only while none is set, so a visit never overrides a User's correction.
    pub async fn set_api_address(&self, db: &PgPool, url: &str, learned: bool) -> anyhow::Result<()> {
        if learned && !self.api_address.read().await.is_empty() {
            return Ok(());
        }
        let mut current = self.api_address.write().await;
        if !learned || current.is_empty() {
            sqlx::query("UPDATE server_ca SET api_address = $1").bind(url).execute(db).await?;
            *current = url.to_owned();
        }
        Ok(())
    }

    /// The API Address the install's mode gives (`RCLI_API_ADDRESS`), applied only when it differs from the one the
    /// last start applied, so a User's correction stands until the mode changes. Empty clears it, to be learned again.
    pub async fn apply_install_api_address(&self, db: &PgPool, url: &str) -> anyhow::Result<()> {
        let changed = sqlx::query("UPDATE server_ca SET api_address = $1, install_api_address = $1 WHERE install_api_address <> $1")
            .bind(url)
            .execute(db)
            .await?
            .rows_affected();
        if changed > 0 {
            *self.api_address.write().await = url.to_owned();
        }
        Ok(())
    }

    pub async fn minimum_version(&self) -> String {
        self.minimum_version.read().await.clone()
    }

    pub async fn set_minimum_version(&self, db: &PgPool, version: &str) -> anyhow::Result<()> {
        sqlx::query("UPDATE server_ca SET minimum_version = $1").bind(version).execute(db).await?;
        *self.minimum_version.write().await = version.to_owned();
        Ok(())
    }

    pub async fn set_address(&self, db: &PgPool, address: &str) -> anyhow::Result<()> {
        sqlx::query("UPDATE server_ca SET server_address = $1").bind(address).execute(db).await?;
        *self.address.write().await = address.to_owned();
        Ok(())
    }
}

/// The names the Server's TLS certificate carries: the Server Address entries without their ports.
pub fn hostnames(address: &str) -> Vec<String> {
    address
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| {
            // `host:9443` names a port; the certificate carries the host. A bare IPv6 literal keeps its colons.
            let host = s.strip_prefix('[').map_or(s, |rest| rest.split(']').next().unwrap_or(rest));
            match host.rsplit_once(':') {
                Some((h, port)) if !h.contains(':') && port.chars().all(|c| c.is_ascii_digit()) => h,
                _ => host,
            }
        })
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::hostnames;

    #[test]
    fn hostnames_drop_ports_and_keep_ipv6() {
        assert_eq!(hostnames(" vps.example.com:9443, localhost "), ["vps.example.com", "localhost"]);
        assert_eq!(hostnames("[::1]:9443,::1"), ["::1", "::1"]);
        assert!(hostnames(" , ").is_empty());
    }
}
