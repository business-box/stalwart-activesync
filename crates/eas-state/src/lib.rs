//! Device and sync state for the EAS gateway, in SQLite.
//!
//! The store holds bookkeeping only: which devices exist and their policy
//! keys, the short folder IDs each device knows, the current and previous
//! SyncKey per folder with the response to replay on a retry, and which
//! items each device holds. Every state change that a response depends on
//! is written in one transaction through [`Store::commit_hierarchy`] or
//! [`Store::commit_sync`], so a crash never leaves a key that points at
//! half-written items.
//!
//! Postgres, for running more than one gateway replica, is left for later.

use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Row, SqlitePool};
use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, thiserror::Error)]
#[error("state store: {0}")]
pub struct StoreError(#[from] sqlx::Error);

impl From<sqlx::migrate::MigrateError> for StoreError {
    fn from(e: sqlx::migrate::MigrateError) -> StoreError {
        StoreError(sqlx::Error::Migrate(Box::new(e)))
    }
}

pub type Result<T> = std::result::Result<T, StoreError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WipeStatus {
    None,
    /// An admin asked for a wipe; the next request answers with it.
    Requested,
    /// The device confirmed it wiped itself.
    Acknowledged,
}

impl WipeStatus {
    fn from_i64(v: i64) -> WipeStatus {
        match v {
            1 => WipeStatus::Requested,
            2 => WipeStatus::Acknowledged,
            _ => WipeStatus::None,
        }
    }

    fn as_i64(self) -> i64 {
        match self {
            WipeStatus::None => 0,
            WipeStatus::Requested => 1,
            WipeStatus::Acknowledged => 2,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    /// Row ID; the key every other table uses.
    pub id: i64,
    pub username: String,
    pub device_id: String,
    pub device_type: String,
    pub policy_key: Option<u32>,
    pub pending_policy_key: Option<u32>,
    pub wipe: WipeStatus,
    pub blocked: bool,
    pub last_sync_request: Option<Vec<u8>>,
}

/// A device as the admin API lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceInfo {
    pub id: i64,
    pub username: String,
    pub device_id: String,
    pub device_type: String,
    pub user_agent: Option<String>,
    pub protocol_version: Option<String>,
    pub model: Option<String>,
    pub friendly_name: Option<String>,
    pub os: Option<String>,
    pub provisioned: bool,
    pub wipe: WipeStatus,
    pub blocked: bool,
    pub first_seen: i64,
    pub last_seen: i64,
}

impl WipeStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            WipeStatus::None => "none",
            WipeStatus::Requested => "requested",
            WipeStatus::Acknowledged => "acknowledged",
        }
    }
}

/// A folder as the device knows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Folder {
    /// Short EAS ID, such as `"7"`.
    pub server_id: String,
    /// `mailbox`, `calendar` or `addressbook`.
    pub kind: String,
    pub jmap_id: String,
    /// EAS folder type (MS-ASCMD 2.2.3.186.3).
    pub folder_type: u8,
    /// Short ID of the parent, `"0"` at the top.
    pub parent_id: String,
    pub display_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HierarchyState {
    pub sync_key: String,
    pub prev_key: Option<String>,
    pub prev_response: Option<Vec<u8>>,
    pub counter: i64,
    pub next_folder_id: i64,
}

/// Changes to the folder tree that one `FolderSync` response announced.
#[derive(Debug, Clone, Default)]
pub struct HierarchyCommit {
    /// Drop every folder's sync state and items first: the device asked
    /// for SyncKey 0 and starts from scratch.
    pub reset: bool,
    pub state: Option<HierarchyState>,
    pub upsert: Vec<Folder>,
    pub delete: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncState {
    pub sync_key: String,
    pub prev_key: Option<String>,
    pub prev_response: Option<Vec<u8>>,
    pub counter: i64,
    /// JMAP state the folder's flag changes were last computed from.
    pub jmap_state: Option<String>,
    pub filter_type: u8,
    /// Encoded `Options` element last sent for this folder.
    pub options: Option<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub server_id: String,
    pub jmap_id: String,
    /// Hash of what the device last received, to spot real changes.
    pub hash: String,
}

/// Changes to one folder's state that one `Sync` response announced.
#[derive(Debug, Clone)]
pub struct SyncCommit {
    pub folder: String,
    /// Forget the device's items in this folder first (SyncKey 0).
    pub reset: bool,
    pub state: SyncState,
    pub upsert: Vec<Item>,
    pub delete: Vec<String>,
}

#[derive(Clone)]
pub struct Store {
    pool: SqlitePool,
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn device_from_row(r: &sqlx::sqlite::SqliteRow) -> Device {
    Device {
        id: r.get("id"),
        username: r.get("username"),
        device_id: r.get("device_id"),
        device_type: r.get("device_type"),
        policy_key: r.get::<Option<i64>, _>("policy_key").map(|v| v as u32),
        pending_policy_key: r
            .get::<Option<i64>, _>("pending_policy_key")
            .map(|v| v as u32),
        wipe: WipeStatus::from_i64(r.get("wipe_status")),
        blocked: r.get::<i64, _>("blocked") != 0,
        last_sync_request: r.get("last_sync_request"),
    }
}

impl Store {
    /// Opens (and creates) the database at a `sqlite:` URL and runs
    /// migrations.
    pub async fn open(url: &str) -> Result<Store> {
        let opts = SqliteConnectOptions::from_str(url)?
            .create_if_missing(true)
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
            .foreign_keys(true)
            .busy_timeout(std::time::Duration::from_secs(10));
        let pool = SqlitePoolOptions::new()
            .max_connections(8)
            .connect_with(opts)
            .await?;
        Store::migrate(pool).await
    }

    /// A private in-memory database, for tests.
    pub async fn memory() -> Result<Store> {
        let opts = SqliteConnectOptions::from_str("sqlite::memory:")?.foreign_keys(true);
        // One connection: each in-memory connection is its own database.
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(opts)
            .await?;
        Store::migrate(pool).await
    }

    async fn migrate(pool: SqlitePool) -> Result<Store> {
        sqlx::migrate!("./migrations").run(&pool).await?;
        Ok(Store { pool })
    }

    /// Finds or registers a device and records that it was seen now.
    pub async fn touch_device(
        &self,
        username: &str,
        device_id: &str,
        device_type: &str,
        user_agent: Option<&str>,
        protocol_version: &str,
    ) -> Result<Device> {
        let t = now();
        let row = sqlx::query(
            "INSERT INTO devices (username, device_id, device_type, user_agent, protocol_version, first_seen, last_seen)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)
             ON CONFLICT (username, device_id) DO UPDATE SET
               device_type = excluded.device_type,
               user_agent = COALESCE(excluded.user_agent, devices.user_agent),
               protocol_version = excluded.protocol_version,
               last_seen = excluded.last_seen
             RETURNING *",
        )
        .bind(username)
        .bind(device_id)
        .bind(device_type)
        .bind(user_agent)
        .bind(protocol_version)
        .bind(t)
        .fetch_one(&self.pool)
        .await?;
        Ok(device_from_row(&row))
    }

    pub async fn device(&self, username: &str, device_id: &str) -> Result<Option<Device>> {
        let row = sqlx::query("SELECT * FROM devices WHERE username = ?1 AND device_id = ?2")
            .bind(username)
            .bind(device_id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.as_ref().map(device_from_row))
    }

    /// All devices, or one user's, most recently seen first.
    pub async fn list_devices(&self, username: Option<&str>) -> Result<Vec<DeviceInfo>> {
        let rows = sqlx::query(
            "SELECT * FROM devices WHERE ?1 IS NULL OR username = ?1 ORDER BY last_seen DESC",
        )
        .bind(username)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .iter()
            .map(|r| DeviceInfo {
                id: r.get("id"),
                username: r.get("username"),
                device_id: r.get("device_id"),
                device_type: r.get("device_type"),
                user_agent: r.get("user_agent"),
                protocol_version: r.get("protocol_version"),
                model: r.get("model"),
                friendly_name: r.get("friendly_name"),
                os: r.get("os"),
                provisioned: r.get::<Option<i64>, _>("policy_key").is_some(),
                wipe: WipeStatus::from_i64(r.get("wipe_status")),
                blocked: r.get::<i64, _>("blocked") != 0,
                first_seen: r.get("first_seen"),
                last_seen: r.get("last_seen"),
            })
            .collect())
    }

    /// Returns false if no such device exists.
    pub async fn set_blocked(&self, device: i64, blocked: bool) -> Result<bool> {
        let r = sqlx::query("UPDATE devices SET blocked = ?2 WHERE id = ?1")
            .bind(device)
            .bind(blocked as i64)
            .execute(&self.pool)
            .await?;
        Ok(r.rows_affected() > 0)
    }

    /// Forgets everything the device was told: it has to provision again
    /// and resync every folder. Keeps the device record and its block and
    /// wipe flags. Returns false if no such device exists.
    pub async fn reset_device(&self, device: i64) -> Result<bool> {
        let mut tx = self.pool.begin().await?;
        for table in [
            "folders",
            "hierarchy_state",
            "sync_state",
            "items",
            "client_ids",
            "ping_state",
        ] {
            sqlx::query(&format!("DELETE FROM {table} WHERE device = ?1"))
                .bind(device)
                .execute(&mut *tx)
                .await?;
        }
        let r = sqlx::query(
            "UPDATE devices SET policy_key = NULL, pending_policy_key = NULL,
               last_sync_request = NULL WHERE id = ?1",
        )
        .bind(device)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(r.rows_affected() > 0)
    }

    /// Removes the device and all its state. Returns false if it didn't
    /// exist.
    pub async fn delete_device(&self, device: i64) -> Result<bool> {
        let r = sqlx::query("DELETE FROM devices WHERE id = ?1")
            .bind(device)
            .execute(&self.pool)
            .await?;
        Ok(r.rows_affected() > 0)
    }

    /// Returns false if no such device exists.
    pub async fn request_wipe(&self, device: i64) -> Result<bool> {
        let r = sqlx::query("UPDATE devices SET wipe_status = 1 WHERE id = ?1")
            .bind(device)
            .execute(&self.pool)
            .await?;
        Ok(r.rows_affected() > 0)
    }

    /// Records the temporary key of the first Provision round trip.
    pub async fn set_pending_policy_key(&self, device: i64, key: u32) -> Result<()> {
        sqlx::query("UPDATE devices SET pending_policy_key = ?2 WHERE id = ?1")
            .bind(device)
            .bind(key as i64)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Makes `key` the device's policy key once it acknowledged the policy.
    pub async fn confirm_policy_key(&self, device: i64, key: u32) -> Result<()> {
        sqlx::query("UPDATE devices SET policy_key = ?2, pending_policy_key = NULL WHERE id = ?1")
            .bind(device)
            .bind(key as i64)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn set_wipe_status(&self, device: i64, status: WipeStatus) -> Result<()> {
        sqlx::query("UPDATE devices SET wipe_status = ?2 WHERE id = ?1")
            .bind(device)
            .bind(status.as_i64())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Stores what the device reported in `Settings`/`Provision`
    /// `DeviceInformation`; `None` keeps the old value.
    pub async fn set_device_info(
        &self,
        device: i64,
        model: Option<&str>,
        friendly_name: Option<&str>,
        os: Option<&str>,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE devices SET model = COALESCE(?2, model),
               friendly_name = COALESCE(?3, friendly_name), os = COALESCE(?4, os)
             WHERE id = ?1",
        )
        .bind(device)
        .bind(model)
        .bind(friendly_name)
        .bind(os)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn set_last_sync_request(&self, device: i64, request: &[u8]) -> Result<()> {
        sqlx::query("UPDATE devices SET last_sync_request = ?2 WHERE id = ?1")
            .bind(device)
            .bind(request)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// The JMAP ID recorded for a device-side `ClientId`, if any; lets a
    /// retried send or add be answered instead of repeated.
    pub async fn client_id(&self, device: i64, client_id: &str) -> Result<Option<String>> {
        let row =
            sqlx::query("SELECT jmap_id FROM client_ids WHERE device = ?1 AND client_id = ?2")
                .bind(device)
                .bind(client_id)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.map(|r| r.get("jmap_id")))
    }

    pub async fn record_client_id(
        &self,
        device: i64,
        client_id: &str,
        jmap_id: &str,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO client_ids (device, client_id, jmap_id) VALUES (?1, ?2, ?3)
             ON CONFLICT (device, client_id) DO UPDATE SET jmap_id = excluded.jmap_id",
        )
        .bind(device)
        .bind(client_id)
        .bind(jmap_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn hierarchy(&self, device: i64) -> Result<Option<HierarchyState>> {
        let row = sqlx::query("SELECT * FROM hierarchy_state WHERE device = ?1")
            .bind(device)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.map(|r| HierarchyState {
            sync_key: r.get("sync_key"),
            prev_key: r.get("prev_key"),
            prev_response: r.get("prev_response"),
            counter: r.get("counter"),
            next_folder_id: r.get("next_folder_id"),
        }))
    }

    pub async fn folders(&self, device: i64) -> Result<Vec<Folder>> {
        let rows = sqlx::query(
            "SELECT * FROM folders WHERE device = ?1 ORDER BY CAST(server_id AS INTEGER)",
        )
        .bind(device)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .iter()
            .map(|r| Folder {
                server_id: r.get("server_id"),
                kind: r.get("kind"),
                jmap_id: r.get("jmap_id"),
                folder_type: r.get::<i64, _>("folder_type") as u8,
                parent_id: r.get("parent_id"),
                display_name: r.get("display_name"),
            })
            .collect())
    }

    pub async fn folder(&self, device: i64, server_id: &str) -> Result<Option<Folder>> {
        Ok(self
            .folders(device)
            .await?
            .into_iter()
            .find(|f| f.server_id == server_id))
    }

    pub async fn commit_hierarchy(&self, device: i64, commit: HierarchyCommit) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        if commit.reset {
            for table in ["sync_state", "items", "client_ids", "ping_state"] {
                sqlx::query(&format!("DELETE FROM {table} WHERE device = ?1"))
                    .bind(device)
                    .execute(&mut *tx)
                    .await?;
            }
        }
        for id in &commit.delete {
            sqlx::query("DELETE FROM folders WHERE device = ?1 AND server_id = ?2")
                .bind(device)
                .bind(id)
                .execute(&mut *tx)
                .await?;
            for table in ["sync_state", "items"] {
                sqlx::query(&format!(
                    "DELETE FROM {table} WHERE device = ?1 AND folder = ?2"
                ))
                .bind(device)
                .bind(id)
                .execute(&mut *tx)
                .await?;
            }
        }
        for f in &commit.upsert {
            sqlx::query(
                "INSERT INTO folders (device, server_id, kind, jmap_id, folder_type, parent_id, display_name)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT (device, server_id) DO UPDATE SET
                   kind = excluded.kind, jmap_id = excluded.jmap_id,
                   folder_type = excluded.folder_type, parent_id = excluded.parent_id,
                   display_name = excluded.display_name",
            )
            .bind(device)
            .bind(&f.server_id)
            .bind(&f.kind)
            .bind(&f.jmap_id)
            .bind(f.folder_type as i64)
            .bind(&f.parent_id)
            .bind(&f.display_name)
            .execute(&mut *tx)
            .await?;
        }
        if let Some(s) = &commit.state {
            sqlx::query(
                "INSERT INTO hierarchy_state (device, sync_key, prev_key, prev_response, counter, next_folder_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT (device) DO UPDATE SET
                   sync_key = excluded.sync_key, prev_key = excluded.prev_key,
                   prev_response = excluded.prev_response, counter = excluded.counter,
                   next_folder_id = excluded.next_folder_id",
            )
            .bind(device)
            .bind(&s.sync_key)
            .bind(&s.prev_key)
            .bind(&s.prev_response)
            .bind(s.counter)
            .bind(s.next_folder_id)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Heartbeat and folder IDs of the device's last `Ping`.
    pub async fn ping_state(&self, device: i64) -> Result<Option<(u64, Vec<String>)>> {
        let row = sqlx::query("SELECT heartbeat, folders FROM ping_state WHERE device = ?1")
            .bind(device)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.map(|r| {
            let folders: String = r.get("folders");
            (
                r.get::<i64, _>("heartbeat") as u64,
                folders
                    .split(',')
                    .filter(|f| !f.is_empty())
                    .map(str::to_string)
                    .collect(),
            )
        }))
    }

    pub async fn set_ping_state(
        &self,
        device: i64,
        heartbeat: u64,
        folders: &[String],
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO ping_state (device, heartbeat, folders) VALUES (?1, ?2, ?3)
             ON CONFLICT (device) DO UPDATE SET heartbeat = excluded.heartbeat, folders = excluded.folders",
        )
        .bind(device)
        .bind(heartbeat as i64)
        .bind(folders.join(","))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn sync_state(&self, device: i64, folder: &str) -> Result<Option<SyncState>> {
        let row = sqlx::query("SELECT * FROM sync_state WHERE device = ?1 AND folder = ?2")
            .bind(device)
            .bind(folder)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.map(|r| SyncState {
            sync_key: r.get("sync_key"),
            prev_key: r.get("prev_key"),
            prev_response: r.get("prev_response"),
            counter: r.get("counter"),
            jmap_state: r.get("jmap_state"),
            filter_type: r.get::<i64, _>("filter_type") as u8,
            options: r.get("options"),
        }))
    }

    pub async fn items(&self, device: i64, folder: &str) -> Result<Vec<Item>> {
        let rows = sqlx::query(
            "SELECT server_id, jmap_id, hash FROM items WHERE device = ?1 AND folder = ?2",
        )
        .bind(device)
        .bind(folder)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .iter()
            .map(|r| Item {
                server_id: r.get("server_id"),
                jmap_id: r.get("jmap_id"),
                hash: r.get("hash"),
            })
            .collect())
    }

    /// Records a move the device made itself (`MoveItems`): the item leaves
    /// `src_folder` and, if given, is known under its new ID in the
    /// destination, so neither folder's next Sync reports it again.
    pub async fn move_item(
        &self,
        device: i64,
        src_folder: &str,
        src_id: &str,
        dst_folder: &str,
        dst: Option<Item>,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM items WHERE device = ?1 AND folder = ?2 AND server_id = ?3")
            .bind(device)
            .bind(src_folder)
            .bind(src_id)
            .execute(&mut *tx)
            .await?;
        if let Some(it) = dst {
            sqlx::query(
                "INSERT INTO items (device, folder, server_id, jmap_id, hash) VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT (device, folder, server_id) DO UPDATE SET
                   jmap_id = excluded.jmap_id, hash = excluded.hash",
            )
            .bind(device)
            .bind(dst_folder)
            .bind(&it.server_id)
            .bind(&it.jmap_id)
            .bind(&it.hash)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn commit_sync(&self, device: i64, commit: SyncCommit) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        if commit.reset {
            sqlx::query("DELETE FROM items WHERE device = ?1 AND folder = ?2")
                .bind(device)
                .bind(&commit.folder)
                .execute(&mut *tx)
                .await?;
        }
        for id in &commit.delete {
            sqlx::query("DELETE FROM items WHERE device = ?1 AND folder = ?2 AND server_id = ?3")
                .bind(device)
                .bind(&commit.folder)
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        for it in &commit.upsert {
            sqlx::query(
                "INSERT INTO items (device, folder, server_id, jmap_id, hash) VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT (device, folder, server_id) DO UPDATE SET
                   jmap_id = excluded.jmap_id, hash = excluded.hash",
            )
            .bind(device)
            .bind(&commit.folder)
            .bind(&it.server_id)
            .bind(&it.jmap_id)
            .bind(&it.hash)
            .execute(&mut *tx)
            .await?;
        }
        let s = &commit.state;
        sqlx::query(
            "INSERT INTO sync_state (device, folder, sync_key, prev_key, prev_response, counter, jmap_state, filter_type, options)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT (device, folder) DO UPDATE SET
               sync_key = excluded.sync_key, prev_key = excluded.prev_key,
               prev_response = excluded.prev_response, counter = excluded.counter,
               jmap_state = excluded.jmap_state, filter_type = excluded.filter_type,
               options = excluded.options",
        )
        .bind(device)
        .bind(&commit.folder)
        .bind(&s.sync_key)
        .bind(&s.prev_key)
        .bind(&s.prev_response)
        .bind(s.counter)
        .bind(&s.jmap_state)
        .bind(s.filter_type as i64)
        .bind(&s.options)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn store_with_device() -> (Store, Device) {
        let s = Store::memory().await.unwrap();
        let d = s
            .touch_device("alice", "DEV1", "iPhone", Some("iOS/18"), "16.1")
            .await
            .unwrap();
        (s, d)
    }

    #[tokio::test]
    async fn devices_upsert_and_policy() {
        let (s, d) = store_with_device().await;
        assert_eq!(d.policy_key, None);
        assert_eq!(d.wipe, WipeStatus::None);
        s.set_pending_policy_key(d.id, 42).await.unwrap();
        let again = s
            .touch_device("alice", "DEV1", "iPhone", None, "16.1")
            .await
            .unwrap();
        assert_eq!(again.id, d.id);
        assert_eq!(again.pending_policy_key, Some(42));
        s.confirm_policy_key(d.id, 4_000_000_000).await.unwrap();
        let d = s.device("alice", "DEV1").await.unwrap().unwrap();
        assert_eq!(d.policy_key, Some(4_000_000_000));
        assert_eq!(d.pending_policy_key, None);
        s.set_wipe_status(d.id, WipeStatus::Requested)
            .await
            .unwrap();
        let d = s.device("alice", "DEV1").await.unwrap().unwrap();
        assert_eq!(d.wipe, WipeStatus::Requested);
        assert!(s.device("bob", "DEV1").await.unwrap().is_none());

        let listed = s.list_devices(Some("alice")).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert!(listed[0].provisioned);
        assert!(s.list_devices(Some("bob")).await.unwrap().is_empty());
        assert!(s.set_blocked(d.id, true).await.unwrap());
        assert!(s.device("alice", "DEV1").await.unwrap().unwrap().blocked);
        assert!(s.reset_device(d.id).await.unwrap());
        let after = s.device("alice", "DEV1").await.unwrap().unwrap();
        assert_eq!(after.policy_key, None);
        assert!(after.blocked, "reset keeps the block");
        assert!(!s.set_blocked(9999, true).await.unwrap());

        assert_eq!(s.client_id(d.id, "c1").await.unwrap(), None);
        s.record_client_id(d.id, "c1", "m9").await.unwrap();
        assert_eq!(
            s.client_id(d.id, "c1").await.unwrap().as_deref(),
            Some("m9")
        );

        assert_eq!(s.ping_state(d.id).await.unwrap(), None);
        s.set_ping_state(d.id, 480, &["2".into(), "5".into()])
            .await
            .unwrap();
        assert_eq!(
            s.ping_state(d.id).await.unwrap(),
            Some((480, vec!["2".into(), "5".into()]))
        );
    }

    fn folder(id: &str, jmap: &str) -> Folder {
        Folder {
            server_id: id.into(),
            kind: "mailbox".into(),
            jmap_id: jmap.into(),
            folder_type: 12,
            parent_id: "0".into(),
            display_name: format!("F{id}"),
        }
    }

    fn sync_state(key: &str) -> SyncState {
        SyncState {
            sync_key: key.into(),
            prev_key: None,
            prev_response: None,
            counter: 1,
            jmap_state: Some("s1".into()),
            filter_type: 0,
            options: None,
        }
    }

    #[tokio::test]
    async fn hierarchy_and_sync_commits() {
        let (s, d) = store_with_device().await;
        s.commit_hierarchy(
            d.id,
            HierarchyCommit {
                reset: false,
                state: Some(HierarchyState {
                    sync_key: "1".into(),
                    prev_key: None,
                    prev_response: Some(vec![1, 2]),
                    counter: 1,
                    next_folder_id: 3,
                }),
                upsert: vec![folder("1", "a"), folder("2", "b")],
                delete: vec![],
            },
        )
        .await
        .unwrap();
        assert_eq!(s.folders(d.id).await.unwrap().len(), 2);
        assert_eq!(s.hierarchy(d.id).await.unwrap().unwrap().next_folder_id, 3);

        s.commit_sync(
            d.id,
            SyncCommit {
                folder: "2".into(),
                reset: true,
                state: sync_state("k1"),
                upsert: vec![
                    Item {
                        server_id: "2:x".into(),
                        jmap_id: "x".into(),
                        hash: "h".into(),
                    },
                    Item {
                        server_id: "2:y".into(),
                        jmap_id: "y".into(),
                        hash: "h".into(),
                    },
                ],
                delete: vec![],
            },
        )
        .await
        .unwrap();
        s.commit_sync(
            d.id,
            SyncCommit {
                folder: "2".into(),
                reset: false,
                state: sync_state("k2"),
                upsert: vec![],
                delete: vec!["2:x".into()],
            },
        )
        .await
        .unwrap();
        assert_eq!(s.items(d.id, "2").await.unwrap().len(), 1);
        s.move_item(
            d.id,
            "2",
            "2:y",
            "1",
            Some(Item {
                server_id: "1:y".into(),
                jmap_id: "y".into(),
                hash: "h".into(),
            }),
        )
        .await
        .unwrap();
        assert!(s.items(d.id, "2").await.unwrap().is_empty());
        assert_eq!(s.items(d.id, "1").await.unwrap()[0].server_id, "1:y");
        assert_eq!(
            s.sync_state(d.id, "2").await.unwrap().unwrap().sync_key,
            "k2"
        );

        // Deleting the folder drops its sync state and items.
        s.commit_hierarchy(
            d.id,
            HierarchyCommit {
                delete: vec!["2".into()],
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert!(s.sync_state(d.id, "2").await.unwrap().is_none());
        assert!(s.items(d.id, "2").await.unwrap().is_empty());
        assert_eq!(s.folders(d.id).await.unwrap().len(), 1);

        // A reset drops every folder's sync state but keeps the folder IDs.
        s.commit_sync(
            d.id,
            SyncCommit {
                folder: "1".into(),
                reset: false,
                state: sync_state("k"),
                upsert: vec![],
                delete: vec![],
            },
        )
        .await
        .unwrap();
        s.commit_hierarchy(
            d.id,
            HierarchyCommit {
                reset: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert!(s.sync_state(d.id, "1").await.unwrap().is_none());
        assert_eq!(s.folders(d.id).await.unwrap().len(), 1);
    }
}
