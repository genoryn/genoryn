//! Indexed, crash-consistent local username persistence; independent of consensus.
use crate::username::{ReceiverBundle, Registry, canonicalize};
use anyhow::{Context, Result, ensure};
use genoryn_core::{chain::unix_time, params::Network};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use std::path::Path;

pub const MAX_REGISTERED_IDENTITIES: u64 = 100_000;
const MAX_RECORD_BYTES: usize = 4096;
pub struct RegistryStore {
    db: Connection,
    network: Network,
}
fn record(db: &Connection, name: &str) -> Result<Option<ReceiverBundle>> {
    let length: Option<i64> = db
        .query_row(
            "SELECT length(bundle) FROM receiver_records WHERE username=?",
            [name],
            |r| r.get(0),
        )
        .optional()?;
    let Some(length) = length else {
        return Ok(None);
    };
    ensure!(
        length > 0 && length <= MAX_RECORD_BYTES as i64,
        "corrupt receiver record length"
    );
    let bytes: Vec<u8> = db.query_row(
        "SELECT bundle FROM receiver_records WHERE username=?",
        [name],
        |r| r.get(0),
    )?;
    let result: ReceiverBundle = serde_json::from_slice(&bytes)?;
    ensure!(result.username == name, "receiver record index mismatch");
    Ok(Some(result))
}
fn validate_update(old: Option<&ReceiverBundle>, new: &ReceiverBundle) -> Result<()> {
    if let Some(old) = old {
        ensure!(
            old.owner_key == new.owner_key,
            "username belongs to another key; local v1 does not reassign expired names"
        );
        ensure!(
            new.sequence == old.sequence.checked_add(1).context("sequence overflow")?
                && new.issued_at >= old.issued_at,
            "replayed receiver bundle"
        );
    } else {
        ensure!(
            new.sequence == 0,
            "first registration sequence must be zero"
        );
    }
    Ok(())
}
impl RegistryStore {
    pub fn open(path: &Path, network: Network, legacy: Option<&Path>) -> Result<Self> {
        network.ensure_runnable()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut db = Connection::open(path)?;
        db.pragma_update(None, "journal_mode", "WAL")?;
        db.pragma_update(None, "synchronous", "FULL")?;
        db.busy_timeout(std::time::Duration::from_secs(5))?;
        let integrity: String = db.query_row("PRAGMA quick_check", [], |r| r.get(0))?;
        ensure!(
            integrity == "ok",
            "username database integrity check failed"
        );
        db.execute_batch("CREATE TABLE IF NOT EXISTS registry_meta(key TEXT PRIMARY KEY,value TEXT NOT NULL); CREATE TABLE IF NOT EXISTS receiver_records(username TEXT PRIMARY KEY,bundle BLOB NOT NULL CHECK(length(bundle)<=4096)); CREATE TABLE IF NOT EXISTS registry_stats(id INTEGER PRIMARY KEY CHECK(id=1),records INTEGER NOT NULL CHECK(records>=0 AND records<=100000)); INSERT OR IGNORE INTO registry_stats(id,records) VALUES(1,0);")?;
        for (key, value) in [("schema", "1".to_owned()), ("network", network.chain_id())] {
            let existing: Option<String> = db
                .query_row("SELECT value FROM registry_meta WHERE key=?", [key], |r| {
                    r.get(0)
                })
                .optional()?;
            if let Some(existing) = existing {
                ensure!(existing == value, "incompatible username database {key}");
            } else {
                db.execute(
                    "INSERT INTO registry_meta(key,value) VALUES(?,?)",
                    params![key, value],
                )?;
            }
        }
        let migrated: Option<String> = db
            .query_row(
                "SELECT value FROM registry_meta WHERE key='legacy_imported'",
                [],
                |r| r.get(0),
            )
            .optional()?;
        if migrated.is_none() {
            let entries = if let Some(path) = legacy.filter(|p| p.exists()) {
                Registry::load(path)?.entries
            } else {
                Default::default()
            };
            ensure!(
                entries.len() as u64 <= MAX_REGISTERED_IDENTITIES,
                "legacy registry exceeds capacity"
            );
            let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let current: u64 =
                tx.query_row("SELECT records FROM registry_stats WHERE id=1", [], |r| {
                    r.get(0)
                })?;
            ensure!(
                current == 0 || entries.is_empty(),
                "refusing to merge legacy records into an existing registry"
            );
            for (name, bundle) in &entries {
                ensure!(
                    canonicalize(name)? == *name && bundle.username == *name,
                    "legacy record name mismatch"
                );
                bundle.verify_for_renewal(network, unix_time())?;
                let bytes = serde_json::to_vec(bundle)?;
                ensure!(bytes.len() <= MAX_RECORD_BYTES, "legacy record too large");
                tx.execute(
                    "INSERT INTO receiver_records(username,bundle) VALUES(?,?)",
                    params![name, bytes],
                )?;
            }
            tx.execute(
                "UPDATE registry_stats SET records=records+? WHERE id=1",
                [entries.len() as u64],
            )?;
            tx.execute(
                "INSERT INTO registry_meta(key,value) VALUES('legacy_imported','1')",
                [],
            )?;
            tx.commit()?;
        }
        let actual: u64 =
            db.query_row("SELECT COUNT(*) FROM receiver_records", [], |r| r.get(0))?;
        let count: u64 =
            db.query_row("SELECT records FROM registry_stats WHERE id=1", [], |r| {
                r.get(0)
            })?;
        ensure!(
            actual == count && count <= MAX_REGISTERED_IDENTITIES,
            "username record count mismatch"
        );
        Ok(Self { db, network })
    }
    pub fn len(&self) -> Result<u64> {
        Ok(self
            .db
            .query_row("SELECT records FROM registry_stats WHERE id=1", [], |r| {
                r.get(0)
            })?)
    }
    pub fn is_empty(&self) -> Result<bool> {
        Ok(self.len()? == 0)
    }
    pub fn register(&mut self, bundle: ReceiverBundle, network: Network, now: u64) -> Result<()> {
        ensure!(network == self.network, "registry network mismatch");
        bundle.verify(network, now)?;
        let bytes = serde_json::to_vec(&bundle)?;
        ensure!(
            bytes.len() <= MAX_RECORD_BYTES,
            "receiver bundle exceeds record limit"
        );
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let old = record(&tx, &bundle.username)?;
        if let Some(existing) = &old {
            existing.verify_for_renewal(network, now)?;
        }
        validate_update(old.as_ref(), &bundle)?;
        if old.is_none() {
            let count: u64 =
                tx.query_row("SELECT records FROM registry_stats WHERE id=1", [], |r| {
                    r.get(0)
                })?;
            ensure!(
                count < MAX_REGISTERED_IDENTITIES,
                "registry capacity reached"
            );
            tx.execute("UPDATE registry_stats SET records=records+1 WHERE id=1", [])?;
        }
        tx.execute("INSERT INTO receiver_records(username,bundle) VALUES(?,?) ON CONFLICT(username) DO UPDATE SET bundle=excluded.bundle",params![bundle.username,bytes])?;
        tx.commit()?;
        Ok(())
    }
    pub fn resolve(&self, name: &str, network: Network, now: u64) -> Result<ReceiverBundle> {
        ensure!(network == self.network, "registry network mismatch");
        let name = canonicalize(name)?;
        let bundle =
            record(&self.db, &name)?.context("username is not registered with this resolver")?;
        bundle.verify(network, now)?;
        Ok(bundle)
    }
    pub fn record_for_renewal(
        &self,
        name: &str,
        network: Network,
        now: u64,
    ) -> Result<ReceiverBundle> {
        ensure!(network == self.network, "registry network mismatch");
        let name = canonicalize(name)?;
        let bundle = record(&self.db, &name)?.context("username is not registered")?;
        bundle.verify_for_renewal(network, now)?;
        Ok(bundle)
    }
    pub fn checkpoint(&self) -> Result<()> {
        self.db.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::Wallet;
    #[test]
    fn persistence_renewal_and_atomic_ownership_rules() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("names.sqlite");
        let w = Wallet::create(Network::Devnet).unwrap();
        let mut store = RegistryStore::open(&path, Network::Devnet, None).unwrap();
        let first = w.receiver_bundle("alice_store", 0, 1000).unwrap();
        store
            .register(first.clone(), Network::Devnet, 1000)
            .unwrap();
        assert_eq!(store.len().unwrap(), 1);
        assert!(store.register(first, Network::Devnet, 1000).is_err());
        assert_eq!(store.len().unwrap(), 1);
        drop(store);
        let mut store = RegistryStore::open(&path, Network::Devnet, None).unwrap();
        assert_eq!(
            store
                .resolve("alice_store", Network::Devnet, 1001)
                .unwrap()
                .sequence,
            0
        );
        let attacker = Wallet::create(Network::Devnet).unwrap();
        assert!(
            store
                .register(
                    attacker.receiver_bundle("alice_store", 1, 1001).unwrap(),
                    Network::Devnet,
                    1001
                )
                .is_err()
        );
        let later = 1001 + crate::username::MAX_LIFETIME;
        assert!(
            store
                .resolve("alice_store", Network::Devnet, later)
                .is_err()
        );
        store
            .register(
                w.receiver_bundle("alice_store", 1, later).unwrap(),
                Network::Devnet,
                later,
            )
            .unwrap();
        assert_eq!(store.len().unwrap(), 1);
        assert!(RegistryStore::open(&path, Network::Testnet, None).is_err());
    }
    #[test]
    fn legacy_migration_is_validated_and_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("names.json");
        let path = dir.path().join("names.sqlite");
        let wallet = Wallet::create(Network::Devnet).unwrap();
        let mut legacy = Registry::default();
        legacy
            .register(
                wallet
                    .receiver_bundle("legacy_alice", 0, unix_time())
                    .unwrap(),
                Network::Devnet,
                unix_time(),
            )
            .unwrap();
        legacy.save(&old).unwrap();
        let store = RegistryStore::open(&path, Network::Devnet, Some(&old)).unwrap();
        assert_eq!(store.len().unwrap(), 1);
        drop(store);
        let store = RegistryStore::open(&path, Network::Devnet, Some(&old)).unwrap();
        assert_eq!(store.len().unwrap(), 1);
        assert!(old.exists());
    }
}
