//! The durable pool ledger.
//!
//! One ledger, two drivers: SQLite for a pool on one machine (and tests) and
//! PostgreSQL for gateways on many machines sharing one ledger. Every state
//! change is one database transaction. Amounts owed to miners are kept in
//! units of 2^-32 gori as decimal text (they can exceed 64 bits).
//!
//! States kept apart, as a miner sees them:
//! - verified work (`work` rows, acknowledged only after they are committed);
//! - immature pool rewards (`credits` of `immature` blocks: on the chain, not
//!   yet 60 blocks deep, never payable);
//! - matured credits (`balances.matured`), payable once above the threshold;
//! - pending payouts (`balances.pending`, in a planned or submitted batch);
//! - paid (`balances.paid`, only after the payout transaction is confirmed).
//!
//! Block lifecycle: `submitted` -> `immature` -> `mature`, or `rejected`,
//! `orphaned` (left the chain before maturity: its credits never count),
//! `reversed` (left the chain after maturity: credits are taken back from
//! unpaid balances; what was already paid is recorded as operator deficit),
//! `ineligible` (below the activation height: never shared).
use crate::{
    ONE_GORI,
    config::PoolPolicy,
    pplns::{self, Allocation, MinerId},
};
use anyhow::{Context, Result, bail, ensure};
use serde::Serialize;
use std::{collections::BTreeMap, path::Path};

const SCHEMA_VERSION: &str = "1";

#[derive(Clone, Debug, PartialEq)]
pub enum Val {
    Int(i64),
    Text(String),
    Null,
}
impl Val {
    fn int(&self) -> Result<i64> {
        match self {
            Self::Int(value) => Ok(*value),
            other => bail!("expected an integer column, found {other:?}"),
        }
    }
    fn text(&self) -> Result<&str> {
        match self {
            Self::Text(value) => Ok(value),
            other => bail!("expected a text column, found {other:?}"),
        }
    }
    fn opt_int(&self) -> Result<Option<i64>> {
        match self {
            Self::Null => Ok(None),
            other => other.int().map(Some),
        }
    }
    fn opt_text(&self) -> Result<Option<String>> {
        match self {
            Self::Null => Ok(None),
            other => other.text().map(|text| Some(text.to_owned())),
        }
    }
    fn fixed(&self) -> Result<u128> {
        self.text()?.parse().context("ledger amount")
    }
}
fn i(value: impl TryInto<i64>) -> Val {
    Val::Int(value.try_into().unwrap_or(i64::MAX))
}
fn t(value: impl ToString) -> Val {
    Val::Text(value.to_string())
}

/// One SQL transaction; statements use `$1, $2, ...` placeholders.
pub trait Tx {
    fn exec(&mut self, sql: &str, params: &[Val]) -> Result<u64>;
    fn rows(&mut self, sql: &str, params: &[Val]) -> Result<Vec<Vec<Val>>>;
    fn is_postgres(&self) -> bool {
        false
    }
}

/// Rows inserted per statement: one round trip per chunk instead of per row.
const INSERT_CHUNK: usize = 1_000;

/// `INSERT INTO head VALUES (...), (...) ... returning` in chunks; `rows` all have
/// the same width. Returns the `returning` rows of every chunk.
fn insert_many(
    tx: &mut dyn Tx,
    head: &str,
    rows: &[Vec<Val>],
    returning: &str,
) -> Result<Vec<Vec<Val>>> {
    let mut out = Vec::new();
    for chunk in rows.chunks(INSERT_CHUNK) {
        let width = chunk.first().map_or(0, Vec::len);
        let mut sql = format!("INSERT INTO {head} VALUES ");
        let mut params = Vec::with_capacity(chunk.len() * width);
        for (row_index, row) in chunk.iter().enumerate() {
            ensure!(row.len() == width, "rows of different width");
            if row_index > 0 {
                sql.push(',');
            }
            sql.push('(');
            for column in 0..width {
                if column > 0 {
                    sql.push(',');
                }
                sql.push_str(&format!("${}", row_index * width + column + 1));
            }
            sql.push(')');
            params.extend(row.iter().cloned());
        }
        if returning.is_empty() {
            tx.exec(&sql, &params)?;
        } else {
            sql.push(' ');
            sql.push_str(returning);
            out.extend(tx.rows(&sql, &params)?);
        }
    }
    Ok(out)
}

fn sqlite_sql(sql: &str) -> String {
    sql.replace('$', "?")
}
struct SqliteTx<'a>(rusqlite::Transaction<'a>);
fn to_sqlite(value: &Val) -> rusqlite::types::Value {
    match value {
        Val::Int(v) => rusqlite::types::Value::Integer(*v),
        Val::Text(v) => rusqlite::types::Value::Text(v.clone()),
        Val::Null => rusqlite::types::Value::Null,
    }
}
impl Tx for SqliteTx<'_> {
    fn exec(&mut self, sql: &str, params: &[Val]) -> Result<u64> {
        Ok(self.0.execute(
            &sqlite_sql(sql),
            rusqlite::params_from_iter(params.iter().map(to_sqlite)),
        )? as u64)
    }
    fn rows(&mut self, sql: &str, params: &[Val]) -> Result<Vec<Vec<Val>>> {
        let mut statement = self.0.prepare(&sqlite_sql(sql))?;
        let columns = statement.column_count();
        let rows = statement.query_map(
            rusqlite::params_from_iter(params.iter().map(to_sqlite)),
            |row| {
                (0..columns)
                    .map(|index| {
                        Ok(match row.get_ref(index)? {
                            rusqlite::types::ValueRef::Integer(v) => Val::Int(v),
                            rusqlite::types::ValueRef::Text(v) => {
                                Val::Text(String::from_utf8_lossy(v).into_owned())
                            }
                            rusqlite::types::ValueRef::Null => Val::Null,
                            _ => Val::Null,
                        })
                    })
                    .collect::<rusqlite::Result<Vec<_>>>()
            },
        )?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }
}

struct PostgresTx<'a>(postgres::Transaction<'a>);
fn postgres_params(params: &[Val]) -> Vec<Box<dyn postgres::types::ToSql + Sync>> {
    params
        .iter()
        .map(|value| -> Box<dyn postgres::types::ToSql + Sync> {
            match value {
                Val::Int(v) => Box::new(*v),
                Val::Text(v) => Box::new(v.clone()),
                Val::Null => Box::new(None::<String>),
            }
        })
        .collect()
}
impl Tx for PostgresTx<'_> {
    fn is_postgres(&self) -> bool {
        true
    }
    fn exec(&mut self, sql: &str, params: &[Val]) -> Result<u64> {
        let owned = postgres_params(params);
        let refs: Vec<&(dyn postgres::types::ToSql + Sync)> =
            owned.iter().map(|value| value.as_ref()).collect();
        Ok(self.0.execute(sql, &refs)?)
    }
    fn rows(&mut self, sql: &str, params: &[Val]) -> Result<Vec<Vec<Val>>> {
        let owned = postgres_params(params);
        let refs: Vec<&(dyn postgres::types::ToSql + Sync)> =
            owned.iter().map(|value| value.as_ref()).collect();
        let rows = self.0.query(sql, &refs)?;
        rows.iter()
            .map(|row| {
                (0..row.len())
                    .map(|index| {
                        let kind = row.columns()[index].type_();
                        Ok(if *kind == postgres::types::Type::INT8 {
                            row.try_get::<_, Option<i64>>(index)?
                                .map_or(Val::Null, Val::Int)
                        } else if *kind == postgres::types::Type::INT4 {
                            row.try_get::<_, Option<i32>>(index)?
                                .map_or(Val::Null, |v| Val::Int(v.into()))
                        } else {
                            row.try_get::<_, Option<String>>(index)?
                                .map_or(Val::Null, Val::Text)
                        })
                    })
                    .collect::<Result<Vec<_>>>()
            })
            .collect()
    }
}

enum Backend {
    Sqlite(rusqlite::Connection),
    Postgres(Box<postgres::Client>),
}

fn serialization_failure(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<postgres::Error>()
            .and_then(postgres::Error::code)
            .is_some_and(|code| {
                *code == postgres::error::SqlState::T_R_SERIALIZATION_FAILURE
                    || *code == postgres::error::SqlState::T_R_DEADLOCK_DETECTED
            })
    })
}

fn schema(identity: &str) -> Vec<String> {
    [
        "CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY, value TEXT NOT NULL)",
        "CREATE TABLE IF NOT EXISTS miners(id {ID}, address TEXT NOT NULL UNIQUE, created_at BIGINT NOT NULL)",
        "CREATE TABLE IF NOT EXISTS work(seq {ID}, miner_id BIGINT NOT NULL, work BIGINT NOT NULL, shares BIGINT NOT NULL, gateway TEXT NOT NULL, recorded_at BIGINT NOT NULL)",
        "CREATE INDEX IF NOT EXISTS work_by_miner ON work(miner_id, seq)",
        "CREATE TABLE IF NOT EXISTS blocks(id {ID}, height BIGINT NOT NULL, hash TEXT NOT NULL UNIQUE, reward BIGINT NOT NULL, difficulty BIGINT NOT NULL, last_seq BIGINT NOT NULL, gateway TEXT NOT NULL, found_at BIGINT NOT NULL, status TEXT NOT NULL, fee BIGINT NOT NULL DEFAULT 0, window_work BIGINT NOT NULL DEFAULT 0, dust TEXT NOT NULL DEFAULT '0', updated_at BIGINT NOT NULL)",
        "CREATE INDEX IF NOT EXISTS blocks_by_status ON blocks(status, height)",
        "CREATE TABLE IF NOT EXISTS credits(block_id BIGINT NOT NULL, miner_id BIGINT NOT NULL, work BIGINT NOT NULL, fixed TEXT NOT NULL, PRIMARY KEY(block_id, miner_id))",
        "CREATE TABLE IF NOT EXISTS balances(miner_id BIGINT PRIMARY KEY, matured TEXT NOT NULL, pending BIGINT NOT NULL, paid TEXT NOT NULL)",
        "CREATE TABLE IF NOT EXISTS operator(id BIGINT PRIMARY KEY, fees TEXT NOT NULL, dust TEXT NOT NULL, deficit TEXT NOT NULL, network_fees BIGINT NOT NULL)",
        "CREATE TABLE IF NOT EXISTS batches(id TEXT PRIMARY KEY, status TEXT NOT NULL, txid TEXT, fee BIGINT NOT NULL, created_at BIGINT NOT NULL, updated_at BIGINT NOT NULL, confirmed_height BIGINT)",
        "CREATE TABLE IF NOT EXISTS payouts(batch_id TEXT NOT NULL, miner_id BIGINT NOT NULL, address TEXT NOT NULL, amount BIGINT NOT NULL, PRIMARY KEY(batch_id, miner_id))",
    ]
    .iter()
    .map(|statement| statement.replace("{ID}", identity))
    .collect()
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct BlockRow {
    pub id: i64,
    pub height: u64,
    pub hash: String,
    pub reward: u64,
    pub difficulty: u64,
    pub last_seq: i64,
    pub status: String,
    pub found_at: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct MinerView {
    pub miner_id: i64,
    pub address: String,
    /// Verified work in the current PPLNS window (expected hashes).
    pub window_work: u64,
    pub pool_window_work: u64,
    /// In units of 2^-32 gori.
    pub immature_fixed: String,
    pub matured_fixed: String,
    pub pending_gori: u64,
    pub paid_gori: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Batch {
    pub id: String,
    pub status: String,
    pub txid: Option<String>,
    pub fee: u64,
    pub payouts: Vec<(String, u64)>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Audit {
    pub mature_rewards_fixed: String,
    pub miners_fixed: String,
    pub operator_fees_fixed: String,
    pub operator_dust_fixed: String,
    pub operator_deficit_fixed: String,
    pub network_fees_gori: u64,
    pub balanced: bool,
}

pub struct Store {
    backend: Backend,
    miners: std::collections::HashMap<String, MinerId>,
}
/// Miner ids remembered per process before the memo starts over.
const MINER_CACHE: usize = 2_000_000;

impl Store {
    pub fn open_sqlite(path: &Path) -> Result<Self> {
        let connection = rusqlite::Connection::open(path)?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "synchronous", "FULL")?;
        connection.busy_timeout(std::time::Duration::from_secs(30))?;
        let mut store = Self {
            backend: Backend::Sqlite(connection),
            miners: Default::default(),
        };
        store.create_schema()?;
        Ok(store)
    }
    /// `url` like `postgresql://user@host/genoryn_pool`. TLS is expected to be
    /// provided by the network (loopback, a private network or a tunnel).
    pub fn open_postgres(url: &str) -> Result<Self> {
        let client = postgres::Client::connect(url, postgres::NoTls)
            .context("cannot connect to the PostgreSQL pool ledger")?;
        let mut store = Self {
            backend: Backend::Postgres(Box::new(client)),
            miners: Default::default(),
        };
        store.create_schema()?;
        Ok(store)
    }
    pub fn driver(&self) -> &'static str {
        match self.backend {
            Backend::Sqlite(_) => "sqlite",
            Backend::Postgres(_) => "postgresql",
        }
    }
    fn create_schema(&mut self) -> Result<()> {
        let identity = match self.backend {
            Backend::Sqlite(_) => "INTEGER PRIMARY KEY AUTOINCREMENT",
            Backend::Postgres(_) => "BIGINT GENERATED BY DEFAULT AS IDENTITY PRIMARY KEY",
        };
        let statements = schema(identity);
        self.tx(|tx| {
            for statement in &statements {
                tx.exec(statement, &[])?;
            }
            tx.exec(
                "INSERT INTO operator(id, fees, dust, deficit, network_fees) VALUES (1, '0', '0', '0', 0) ON CONFLICT(id) DO NOTHING",
                &[],
            )?;
            Ok(())
        })
    }
    /// Runs `work` in one transaction: all of it commits, or none of it.
    /// SQLite writers take the database lock up front; PostgreSQL runs
    /// serializable transactions and retries one that lost a conflict with
    /// another gateway or service, so no balance update is ever lost.
    pub fn tx<T>(&mut self, mut work: impl FnMut(&mut dyn Tx) -> Result<T>) -> Result<T> {
        match &mut self.backend {
            Backend::Sqlite(connection) => {
                let mut tx = SqliteTx(
                    connection
                        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?,
                );
                let value = work(&mut tx)?;
                tx.0.commit()?;
                Ok(value)
            }
            Backend::Postgres(client) => {
                let mut attempt = 0;
                loop {
                    attempt += 1;
                    let result = (|| {
                        let mut tx = PostgresTx(
                            client
                                .build_transaction()
                                .isolation_level(postgres::IsolationLevel::Serializable)
                                .start()?,
                        );
                        let value = work(&mut tx)?;
                        tx.0.commit()?;
                        Ok(value)
                    })();
                    match result {
                        Err(error) if attempt < 8 && serialization_failure(&error) => {
                            std::thread::sleep(std::time::Duration::from_millis(5 << attempt));
                        }
                        other => return other,
                    }
                }
            }
        }
    }

    /// Binds the ledger to one network and activation height on first use and
    /// refuses a ledger made for another.
    pub fn bind(&mut self, policy: &PoolPolicy) -> Result<()> {
        let network = policy.network.name().to_owned();
        let genesis = hex::encode(policy.network.genesis());
        let activation = policy.activation_height.to_string();
        self.tx(|tx| {
            let stored: BTreeMap<String, String> = tx
                .rows("SELECT key, value FROM meta", &[])?
                .into_iter()
                .map(|row| Ok((row[0].text()?.to_owned(), row[1].text()?.to_owned())))
                .collect::<Result<_>>()?;
            if stored.is_empty() {
                for (key, value) in [
                    ("schema", SCHEMA_VERSION),
                    ("network", network.as_str()),
                    ("genesis", genesis.as_str()),
                    ("activation_height", activation.as_str()),
                ] {
                    tx.exec(
                        "INSERT INTO meta(key, value) VALUES ($1, $2)",
                        &[t(key), t(value)],
                    )?;
                }
                return Ok(());
            }
            ensure!(
                stored.get("schema").map(String::as_str) == Some(SCHEMA_VERSION),
                "unsupported pool ledger schema"
            );
            ensure!(
                stored.get("network") == Some(&network) && stored.get("genesis") == Some(&genesis),
                "this pool ledger belongs to another network or genesis"
            );
            ensure!(
                stored.get("activation_height") == Some(&activation),
                "this pool ledger was opened with activation height {}; it cannot change",
                stored.get("activation_height").map_or("?", String::as_str)
            );
            Ok(())
        })
    }
    /// Whether the ledger already holds accounting state (work or blocks).
    pub fn has_state(&mut self) -> Result<bool> {
        self.tx(|tx| {
            Ok(!tx.rows("SELECT seq FROM work LIMIT 1", &[])?.is_empty()
                || !tx.rows("SELECT id FROM blocks LIMIT 1", &[])?.is_empty())
        })
    }
    pub fn miner(&mut self, address: &str, now: u64) -> Result<MinerId> {
        // A miner's id never changes: reconnects need no database round trip.
        if let Some(id) = self.miners.get(address) {
            return Ok(*id);
        }
        let address = address.to_owned();
        let id = self.tx(|tx| {
            tx.exec(
                "INSERT INTO miners(address, created_at) VALUES ($1, $2) ON CONFLICT(address) DO NOTHING",
                &[t(&address), i(now)],
            )?;
            tx.rows("SELECT id FROM miners WHERE address = $1", &[t(&address)])?
                .first()
                .context("miner row")?[0]
                .int()
        })?;
        if self.miners.len() >= MINER_CACHE {
            self.miners.clear();
        }
        self.miners.insert(address, id);
        Ok(id)
    }
    /// Records verified work durably; returns the sequence of each row, in the
    /// order of `rows` (one row per miner). Shares are acknowledged to miners
    /// only after this returns.
    pub fn record_work(
        &mut self,
        gateway: &str,
        rows: &[(MinerId, u64, u32)],
        now: u64,
    ) -> Result<Vec<i64>> {
        let gateway = gateway.to_owned();
        let mut values = Vec::with_capacity(rows.len());
        let mut seen = std::collections::BTreeSet::new();
        for (miner, work, shares) in rows {
            ensure!(*work > 0 && *work <= i64::MAX as u64, "work out of range");
            ensure!(seen.insert(*miner), "one work row per miner per batch");
            values.push(vec![i(*miner), i(*work), i(*shares), t(&gateway), i(now)]);
        }
        self.tx(|tx| {
            let returned = insert_many(
                tx,
                "work(miner_id, work, shares, gateway, recorded_at)",
                &values,
                "RETURNING miner_id, seq",
            )?;
            let by_miner: BTreeMap<MinerId, i64> = returned
                .iter()
                .map(|row| Ok((row[0].int()?, row[1].int()?)))
                .collect::<Result<_>>()?;
            rows.iter()
                .map(|(miner, _, _)| by_miner.get(miner).copied().context("work sequence"))
                .collect()
        })
    }
    /// A block the pool found and submitted; `last_seq` is the newest work row
    /// committed before it, which closes its PPLNS window.
    #[allow(clippy::too_many_arguments)]
    pub fn record_block(
        &mut self,
        height: u64,
        hash: &str,
        reward: u64,
        difficulty: u64,
        last_seq: i64,
        gateway: &str,
        now: u64,
    ) -> Result<i64> {
        let (hash, gateway) = (hash.to_owned(), gateway.to_owned());
        self.tx(|tx| {
            tx.rows(
                "INSERT INTO blocks(height, hash, reward, difficulty, last_seq, gateway, found_at, status, updated_at) VALUES ($1, $2, $3, $4, $5, $6, $7, 'submitted', $7) RETURNING id",
                &[i(height), t(&hash), i(reward), i(difficulty), i(last_seq), t(&gateway), i(now)],
            )?
            .first()
            .context("block id")?[0]
                .int()
        })
    }
    pub fn last_work_seq(&mut self) -> Result<i64> {
        self.tx(|tx| {
            Ok(tx
                .rows("SELECT MAX(seq) FROM work", &[])?
                .first()
                .map(|row| row[0].opt_int())
                .transpose()?
                .flatten()
                .unwrap_or(0))
        })
    }
    fn block_rows(tx: &mut dyn Tx, filter: &str, params: &[Val]) -> Result<Vec<BlockRow>> {
        tx.rows(
            &format!("SELECT id, height, hash, reward, difficulty, last_seq, status, found_at FROM blocks {filter} ORDER BY height, id"),
            params,
        )?
        .into_iter()
        .map(|row| {
            Ok(BlockRow {
                id: row[0].int()?,
                height: row[1].int()? as u64,
                hash: row[2].text()?.to_owned(),
                reward: row[3].int()? as u64,
                difficulty: row[4].int()? as u64,
                last_seq: row[5].int()?,
                status: row[6].text()?.to_owned(),
                found_at: row[7].int()? as u64,
            })
        })
        .collect()
    }
    /// Blocks whose chain status still needs watching at chain tip `tip`.
    pub fn tracked_blocks(&mut self, tip: u64, watch: u64) -> Result<Vec<BlockRow>> {
        let floor = tip.saturating_sub(watch);
        self.tx(|tx| {
            Self::block_rows(
                tx,
                "WHERE status IN ('submitted', 'immature') OR (status = 'mature' AND height >= $1)",
                &[i(floor)],
            )
        })
    }
    pub fn blocks(&mut self, limit: u64) -> Result<Vec<BlockRow>> {
        self.tx(|tx| {
            let mut rows = Self::block_rows(tx, "", &[])?;
            rows.reverse();
            rows.truncate(limit as usize);
            Ok(rows)
        })
    }
    fn set_status(tx: &mut dyn Tx, block: i64, from: &[&str], to: &str, now: u64) -> Result<()> {
        let list = from
            .iter()
            .map(|status| format!("'{status}'"))
            .collect::<Vec<_>>()
            .join(", ");
        let changed = tx.exec(
            &format!("UPDATE blocks SET status = $1, updated_at = $2 WHERE id = $3 AND status IN ({list})"),
            &[t(to), i(now), i(block)],
        )?;
        ensure!(
            changed == 1,
            "block {block} is not in a state that allows {to}"
        );
        Ok(())
    }
    pub fn reject_block(&mut self, block: i64, now: u64) -> Result<()> {
        self.tx(|tx| Self::set_status(tx, block, &["submitted"], "rejected", now))
    }
    pub fn mark_ineligible(&mut self, block: i64, now: u64) -> Result<()> {
        self.tx(|tx| Self::set_status(tx, block, &["submitted"], "ineligible", now))
    }
    /// The PPLNS window that closes at `last_seq`: newest work first, until
    /// `target` work. Read in pages so a large window never loads at once.
    pub fn window(&mut self, last_seq: i64, target: u64) -> Result<(BTreeMap<MinerId, u64>, u64)> {
        self.tx(|tx| {
            let mut rows = Vec::new();
            let mut below = last_seq.saturating_add(1);
            let mut total = 0u64;
            loop {
                let page = tx.rows(
                    "SELECT seq, miner_id, work FROM work WHERE seq < $1 ORDER BY seq DESC LIMIT 10000",
                    &[i(below)],
                )?;
                if page.is_empty() {
                    break;
                }
                for row in &page {
                    let work = row[2].int()? as u64;
                    rows.push((row[1].int()?, work));
                    total = total.saturating_add(work);
                    below = row[0].int()?;
                }
                if total >= target {
                    break;
                }
            }
            Ok(pplns::collect_window(rows, target))
        })
    }
    /// Stores the block's division as immature credits.
    pub fn allocate_block(&mut self, block: i64, allocation: &Allocation, now: u64) -> Result<()> {
        self.tx(|tx| {
            Self::set_status(tx, block, &["submitted"], "immature", now)?;
            tx.exec(
                "UPDATE blocks SET fee = $1, window_work = $2, dust = $3 WHERE id = $4",
                &[
                    i(allocation.fee),
                    i(allocation.window_work),
                    t(allocation.dust),
                    i(block),
                ],
            )?;
            let rows: Vec<Vec<Val>> = allocation
                .credits
                .iter()
                .map(|credit| vec![i(block), i(credit.miner), i(credit.work), t(credit.fixed)])
                .collect();
            insert_many(tx, "credits(block_id, miner_id, work, fixed)", &rows, "")?;
            Ok(())
        })
    }
    fn balance(tx: &mut dyn Tx, miner: MinerId) -> Result<(u128, i64, u128)> {
        Ok(
            match tx
                .rows(
                    "SELECT matured, pending, paid FROM balances WHERE miner_id = $1",
                    &[i(miner)],
                )?
                .first()
            {
                Some(row) => (row[0].fixed()?, row[1].int()?, row[2].fixed()?),
                None => {
                    tx.exec(
                        "INSERT INTO balances(miner_id, matured, pending, paid) VALUES ($1, '0', 0, '0')",
                        &[i(miner)],
                    )?;
                    (0, 0, 0)
                }
            },
        )
    }
    fn set_balance(
        tx: &mut dyn Tx,
        miner: MinerId,
        matured: u128,
        pending: i64,
        paid: u128,
    ) -> Result<()> {
        tx.exec(
            "UPDATE balances SET matured = $1, pending = $2, paid = $3 WHERE miner_id = $4",
            &[t(matured), i(pending), t(paid), i(miner)],
        )?;
        Ok(())
    }
    fn operator(tx: &mut dyn Tx) -> Result<(u128, u128, u128, i64)> {
        let row = tx
            .rows(
                "SELECT fees, dust, deficit, network_fees FROM operator WHERE id = 1",
                &[],
            )?
            .into_iter()
            .next()
            .context("operator row")?;
        Ok((
            row[0].fixed()?,
            row[1].fixed()?,
            row[2].fixed()?,
            row[3].int()?,
        ))
    }
    fn set_operator(
        tx: &mut dyn Tx,
        fees: u128,
        dust: u128,
        deficit: u128,
        network_fees: i64,
    ) -> Result<()> {
        tx.exec(
            "UPDATE operator SET fees = $1, dust = $2, deficit = $3, network_fees = $4 WHERE id = 1",
            &[t(fees), t(dust), t(deficit), i(network_fees)],
        )?;
        Ok(())
    }
    fn credits(tx: &mut dyn Tx, block: i64) -> Result<Vec<(MinerId, u128)>> {
        tx.rows(
            "SELECT miner_id, fixed FROM credits WHERE block_id = $1 ORDER BY miner_id",
            &[i(block)],
        )?
        .into_iter()
        .map(|row| Ok((row[0].int()?, row[1].fixed()?)))
        .collect()
    }
    fn block_terms(tx: &mut dyn Tx, block: i64) -> Result<(u128, u128)> {
        let row = tx
            .rows("SELECT fee, dust FROM blocks WHERE id = $1", &[i(block)])?
            .into_iter()
            .next()
            .context("block")?;
        Ok((row[0].int()? as u128 * ONE_GORI, row[1].fixed()?))
    }
    /// The block is 60 blocks deep on the chain: its credits become payable.
    pub fn mature_block(&mut self, block: i64, now: u64) -> Result<()> {
        self.tx(|tx| {
            Self::set_status(tx, block, &["immature"], "mature", now)?;
            if tx.is_postgres() {
                // One set-based statement: exact NUMERIC arithmetic in the server.
                tx.exec(
                    "INSERT INTO balances(miner_id, matured, pending, paid) SELECT miner_id, fixed, 0, '0' FROM credits WHERE block_id = $1 ON CONFLICT(miner_id) DO UPDATE SET matured = ((balances.matured)::numeric + (excluded.matured)::numeric)::text",
                    &[i(block)],
                )?;
            } else {
                for (miner, fixed) in Self::credits(tx, block)? {
                    let (matured, pending, paid) = Self::balance(tx, miner)?;
                    Self::set_balance(tx, miner, matured + fixed, pending, paid)?;
                }
            }
            let (fee, dust) = Self::block_terms(tx, block)?;
            let (fees, old_dust, deficit, network) = Self::operator(tx)?;
            Self::set_operator(tx, fees + fee, old_dust + dust, deficit, network)
        })
    }
    /// The block left the chain before maturity: its credits never count.
    pub fn orphan_block(&mut self, block: i64, now: u64) -> Result<()> {
        self.tx(|tx| Self::set_status(tx, block, &["submitted", "immature"], "orphaned", now))
    }
    /// The block left the chain after maturity: take its credits back from
    /// unpaid balances; what cannot be taken back is the operator's deficit.
    pub fn reverse_block(&mut self, block: i64, now: u64) -> Result<()> {
        self.tx(|tx| {
            Self::set_status(tx, block, &["mature"], "reversed", now)?;
            let (fees, dust, mut deficit, network) = Self::operator(tx)?;
            for (miner, fixed) in Self::credits(tx, block)? {
                let (matured, pending, paid) = Self::balance(tx, miner)?;
                let taken = matured.min(fixed);
                deficit += fixed - taken;
                Self::set_balance(tx, miner, matured - taken, pending, paid)?;
            }
            let (fee, block_dust) = Self::block_terms(tx, block)?;
            let fee_taken = fees.min(fee);
            let dust_taken = dust.min(block_dust);
            deficit += (fee - fee_taken) + (block_dust - dust_taken);
            Self::set_operator(tx, fees - fee_taken, dust - dust_taken, deficit, network)
        })
    }
    pub fn miner_view(&mut self, address: &str, window_target: u64) -> Result<Option<MinerView>> {
        let address = address.to_owned();
        let last = self.last_work_seq()?;
        let (window, pool_total) = self.window(last, window_target)?;
        self.tx(|tx| {
            let Some(row) = tx
                .rows("SELECT id FROM miners WHERE address = $1", &[t(&address)])?
                .into_iter()
                .next()
            else {
                return Ok(None);
            };
            let miner = row[0].int()?;
            let immature = tx
                .rows(
                    "SELECT c.fixed FROM credits c JOIN blocks b ON b.id = c.block_id WHERE c.miner_id = $1 AND b.status = 'immature'",
                    &[i(miner)],
                )?
                .iter()
                .map(|row| row[0].fixed())
                .sum::<Result<u128>>()?;
            let (matured, pending, paid) = match tx
                .rows("SELECT matured, pending, paid FROM balances WHERE miner_id = $1", &[i(miner)])?
                .first()
            {
                Some(row) => (row[0].fixed()?, row[1].int()?, row[2].fixed()?),
                None => (0, 0, 0),
            };
            Ok(Some(MinerView {
                miner_id: miner,
                address: address.clone(),
                window_work: window.get(&miner).copied().unwrap_or(0),
                pool_window_work: pool_total,
                immature_fixed: immature.to_string(),
                matured_fixed: matured.to_string(),
                pending_gori: pending as u64,
                paid_gori: paid.to_string(),
            }))
        })
    }
    /// Moves matured balances of at least `policy.payout_threshold` into new
    /// payout batches, at most `available` gori in total (recipients plus the
    /// network fee each batch pays). Whole gori only; the fraction stays.
    pub fn plan_payouts(
        &mut self,
        policy: &PoolPolicy,
        available: u64,
        max_batches: usize,
        now: u64,
    ) -> Result<Vec<Batch>> {
        let threshold = u128::from(policy.payout_threshold) * ONE_GORI;
        self.tx(|tx| {
            let mut candidates: Vec<(MinerId, String, u128, i64, u128)> = tx
                .rows(
                    "SELECT b.miner_id, m.address, b.matured, b.pending, b.paid FROM balances b JOIN miners m ON m.id = b.miner_id",
                    &[],
                )?
                .into_iter()
                .map(|row| {
                    Ok((
                        row[0].int()?,
                        row[1].text()?.to_owned(),
                        row[2].fixed()?,
                        row[3].int()?,
                        row[4].fixed()?,
                    ))
                })
                .collect::<Result<Vec<_>>>()?
                .into_iter()
                .filter(|candidate| candidate.2 >= threshold)
                .collect();
            candidates.sort_by(|a, b| b.2.cmp(&a.2).then(a.0.cmp(&b.0)));
            let mut budget = u128::from(available);
            let mut batches = Vec::new();
            for group in candidates.chunks(policy.payout_batch) {
                if batches.len() >= max_batches || budget <= u128::from(policy.payout_fee) {
                    break;
                }
                budget -= u128::from(policy.payout_fee);
                let mut nonce = [0u8; 8];
                rand08::RngCore::fill_bytes(&mut rand08::rngs::OsRng, &mut nonce);
                let id = format!("payout-{now}-{}", hex::encode(nonce));
                let mut payouts = Vec::new();
                for (miner, address, matured, pending, paid) in group {
                    let amount = (matured / ONE_GORI).min(i64::MAX as u128);
                    if amount > budget {
                        continue;
                    }
                    budget -= amount;
                    Self::set_balance(
                        tx,
                        *miner,
                        matured - amount * ONE_GORI,
                        pending + amount as i64,
                        *paid,
                    )?;
                    tx.exec(
                        "INSERT INTO payouts(batch_id, miner_id, address, amount) VALUES ($1, $2, $3, $4)",
                        &[t(&id), i(*miner), t(address), i(amount)],
                    )?;
                    payouts.push((address.clone(), amount as u64));
                }
                if payouts.is_empty() {
                    budget += u128::from(policy.payout_fee);
                    continue;
                }
                tx.exec(
                    "INSERT INTO batches(id, status, fee, created_at, updated_at) VALUES ($1, 'planned', $2, $3, $3)",
                    &[t(&id), i(policy.payout_fee), i(now)],
                )?;
                batches.push(Batch {
                    id,
                    status: "planned".into(),
                    txid: None,
                    fee: policy.payout_fee,
                    payouts,
                });
            }
            Ok(batches)
        })
    }
    /// Batches whose outcome is not settled: planned (maybe never sent) or
    /// submitted (waiting for a block).
    pub fn open_batches(&mut self) -> Result<Vec<Batch>> {
        self.tx(|tx| {
            let batches = tx.rows(
                "SELECT id, status, txid, fee FROM batches WHERE status IN ('planned', 'submitted') ORDER BY created_at, id",
                &[],
            )?;
            batches
                .into_iter()
                .map(|row| {
                    let id = row[0].text()?.to_owned();
                    let payouts = tx
                        .rows(
                            "SELECT address, amount FROM payouts WHERE batch_id = $1 ORDER BY miner_id",
                            &[t(&id)],
                        )?
                        .into_iter()
                        .map(|p| Ok((p[0].text()?.to_owned(), p[1].int()? as u64)))
                        .collect::<Result<Vec<_>>>()?;
                    Ok(Batch {
                        id,
                        status: row[1].text()?.to_owned(),
                        txid: row[2].opt_text()?,
                        fee: row[3].int()? as u64,
                        payouts,
                    })
                })
                .collect()
        })
    }
    pub fn batch_submitted(&mut self, batch: &str, txid: &str, now: u64) -> Result<()> {
        let (batch, txid) = (batch.to_owned(), txid.to_owned());
        self.tx(|tx| {
            let changed = tx.exec(
                "UPDATE batches SET status = 'submitted', txid = $1, updated_at = $2 WHERE id = $3 AND (status = 'planned' OR (status = 'submitted' AND txid = $1))",
                &[t(&txid), i(now), t(&batch)],
            )?;
            ensure!(changed == 1, "payout batch {batch} cannot take transaction {txid}");
            Ok(())
        })
    }
    fn batch_payouts(tx: &mut dyn Tx, batch: &str) -> Result<Vec<(MinerId, i64)>> {
        tx.rows(
            "SELECT miner_id, amount FROM payouts WHERE batch_id = $1",
            &[t(batch)],
        )?
        .into_iter()
        .map(|row| Ok((row[0].int()?, row[1].int()?)))
        .collect()
    }
    /// The payout transaction is in a block: the amounts are paid.
    pub fn batch_confirmed(&mut self, batch: &str, height: u64, now: u64) -> Result<()> {
        let batch = batch.to_owned();
        self.tx(|tx| {
            let fee = tx
                .rows("SELECT fee FROM batches WHERE id = $1 AND status = 'submitted'", &[t(&batch)])?
                .first()
                .context("only a submitted batch can confirm")?[0]
                .int()?;
            for (miner, amount) in Self::batch_payouts(tx, &batch)? {
                let (matured, pending, paid) = Self::balance(tx, miner)?;
                ensure!(pending >= amount, "pending payout accounting broken");
                Self::set_balance(tx, miner, matured, pending - amount, paid + amount as u128)?;
            }
            let (fees, dust, deficit, network) = Self::operator(tx)?;
            Self::set_operator(tx, fees, dust, deficit, network + fee)?;
            tx.exec(
                "UPDATE batches SET status = 'confirmed', confirmed_height = $1, updated_at = $2 WHERE id = $3",
                &[i(height), i(now), t(&batch)],
            )?;
            Ok(())
        })
    }
    /// The payout transaction can never confirm (the wallet saw it expire or
    /// its notes spent elsewhere): the amounts return to matured balances.
    pub fn batch_failed(&mut self, batch: &str, now: u64) -> Result<()> {
        let batch = batch.to_owned();
        self.tx(|tx| {
            let changed = tx.exec(
                "UPDATE batches SET status = 'failed', updated_at = $1 WHERE id = $2 AND status IN ('planned', 'submitted')",
                &[i(now), t(&batch)],
            )?;
            ensure!(changed == 1, "batch {batch} is already settled");
            for (miner, amount) in Self::batch_payouts(tx, &batch)? {
                let (matured, pending, paid) = Self::balance(tx, miner)?;
                Self::set_balance(
                    tx,
                    miner,
                    matured + amount as u128 * ONE_GORI,
                    pending - amount,
                    paid,
                )?;
            }
            Ok(())
        })
    }
    /// Conservation: everything owed to miners plus the operator's fee and
    /// dust, minus the deficit, equals the matured blocks' rewards exactly.
    pub fn audit(&mut self) -> Result<Audit> {
        self.tx(|tx| {
            let rewards = tx
                .rows("SELECT reward FROM blocks WHERE status = 'mature'", &[])?
                .iter()
                .map(|row| Ok(row[0].int()? as u128 * ONE_GORI))
                .sum::<Result<u128>>()?;
            let miners = tx
                .rows("SELECT matured, pending, paid FROM balances", &[])?
                .iter()
                .map(|row| {
                    Ok(row[0].fixed()?
                        + row[1].int()? as u128 * ONE_GORI
                        + row[2].fixed()? * ONE_GORI)
                })
                .sum::<Result<u128>>()?;
            let (fees, dust, deficit, network) = Self::operator(tx)?;
            Ok(Audit {
                mature_rewards_fixed: rewards.to_string(),
                miners_fixed: miners.to_string(),
                operator_fees_fixed: fees.to_string(),
                operator_dust_fixed: dust.to_string(),
                operator_deficit_fixed: deficit.to_string(),
                network_fees_gori: network as u64,
                balanced: miners + fees + dust == rewards + deficit,
            })
        })
    }
}

/// What the accountant needs from a validating node.
pub trait ChainView {
    fn tip(&mut self) -> Result<u64>;
    /// The hash of the block at `height` on the node's chain, if it has one.
    fn hash_at(&mut self, height: u64) -> Result<Option<String>>;
}

/// The node's chain at the heights an accounting pass looks at, read before
/// the pass so the ledger never waits on the network inside a transaction.
#[derive(Clone, Debug, Default)]
pub struct ChainSnapshot {
    pub tip: u64,
    pub hashes: BTreeMap<u64, String>,
}
impl ChainView for ChainSnapshot {
    fn tip(&mut self) -> Result<u64> {
        Ok(self.tip)
    }
    fn hash_at(&mut self, height: u64) -> Result<Option<String>> {
        Ok(self.hashes.get(&height).cloned())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Progress {
    pub allocated: Vec<u64>,
    pub matured: Vec<u64>,
    pub orphaned: Vec<u64>,
    pub reversed: Vec<u64>,
    pub ineligible: Vec<u64>,
}

/// One accounting pass: follows every tracked block on the node's chain.
/// A block is shared only once the node has it at its height, and becomes
/// payable only `maturity` blocks later, exactly as consensus unlocks it.
pub fn account(
    store: &mut Store,
    chain: &mut dyn ChainView,
    policy: &PoolPolicy,
    now: u64,
) -> Result<Progress> {
    let tip = chain.tip()?;
    let maturity = policy.network.maturity();
    let mut progress = Progress::default();
    for block in store.tracked_blocks(tip, policy.reorg_watch)? {
        let canonical = block.height <= tip
            && chain.hash_at(block.height)?.as_deref() == Some(block.hash.as_str());
        match block.status.as_str() {
            "submitted" if canonical => {
                if !policy.eligible(block.height) {
                    store.mark_ineligible(block.id, now)?;
                    progress.ineligible.push(block.height);
                    continue;
                }
                let target = pplns::window_target(block.difficulty, policy.window_factor);
                let (window, _) = store.window(block.last_seq, target)?;
                let allocation = pplns::allocate(block.reward, policy.fee_bps, &window)?;
                store.allocate_block(block.id, &allocation, now)?;
                progress.allocated.push(block.height);
            }
            // Another block took its height and the chain moved on.
            "submitted" if tip >= block.height + 2 => {
                store.orphan_block(block.id, now)?;
                progress.orphaned.push(block.height);
            }
            "immature" if !canonical => {
                store.orphan_block(block.id, now)?;
                progress.orphaned.push(block.height);
            }
            "immature" if tip >= block.height + maturity => {
                store.mature_block(block.id, now)?;
                progress.matured.push(block.height);
            }
            "mature" if !canonical => {
                store.reverse_block(block.id, now)?;
                progress.reversed.push(block.height);
            }
            _ => {}
        }
    }
    Ok(progress)
}

#[cfg(test)]
mod tests {
    use super::*;
    use genoryn_core::params::Network;

    pub(crate) struct FakeChain {
        pub tip: u64,
        pub hashes: BTreeMap<u64, String>,
    }
    impl ChainView for FakeChain {
        fn tip(&mut self) -> Result<u64> {
            Ok(self.tip)
        }
        fn hash_at(&mut self, height: u64) -> Result<Option<String>> {
            Ok(self.hashes.get(&height).cloned())
        }
    }

    fn policy() -> PoolPolicy {
        PoolPolicy {
            payout_threshold: 1_000,
            payout_fee: genoryn_core::params::MIN_FEE,
            ..PoolPolicy::defaults(Network::Devnet)
        }
    }

    pub(crate) fn exercise(store: &mut Store) {
        let policy = policy();
        store.bind(&policy).unwrap();
        assert!(!store.has_state().unwrap());
        let weak = store.miner("gnrd1weak", 1).unwrap();
        let strong = store.miner("gnrd1strong", 1).unwrap();
        assert_eq!(
            store.miner("gnrd1weak", 2).unwrap(),
            weak,
            "one row per address"
        );
        // A 10 H/s and a 10,000 H/s miner over the same ten minutes.
        let seqs = store
            .record_work("g1", &[(weak, 6_000, 40), (strong, 6_000_000, 400)], 10)
            .unwrap();
        assert_eq!(seqs.len(), 2);
        let last = store.last_work_seq().unwrap();
        let reward = 4_825_437_595_129u64;
        store
            .record_block(100, "aa", reward, 6_006_000, last, "g1", 11)
            .unwrap();
        store
            .record_work("g1", &[(weak, 999_999_999, 1)], 12)
            .unwrap();
        let mut chain = FakeChain {
            tip: 100,
            hashes: [(100, "aa".to_owned())].into(),
        };
        let progress = account(store, &mut chain, &policy, 13).unwrap();
        assert_eq!(progress.allocated, vec![100]);
        let target = pplns::window_target(6_006_000, policy.window_factor);
        let weak_view = store.miner_view("gnrd1weak", target).unwrap().unwrap();
        assert_eq!(
            weak_view.matured_fixed, "0",
            "immature rewards are never payable"
        );
        let immature: u128 = weak_view.immature_fixed.parse().unwrap();
        let distributable = u128::from(reward - reward / 100);
        assert_eq!(immature, distributable * ONE_GORI * 6_000 / 6_006_000);
        // Work after the block was found does not change its division.
        let strong_view = store.miner_view("gnrd1strong", target).unwrap().unwrap();
        let strong_immature: u128 = strong_view.immature_fixed.parse().unwrap();
        assert_eq!(
            strong_immature,
            distributable * ONE_GORI * 6_000_000 / 6_006_000
        );
        // Not yet 2 blocks deep (devnet maturity), then mature.
        chain.tip = 101;
        chain.hashes.insert(101, "bb".into());
        assert!(
            account(store, &mut chain, &policy, 14)
                .unwrap()
                .matured
                .is_empty()
        );
        chain.tip = 102;
        chain.hashes.insert(102, "cc".into());
        assert_eq!(
            account(store, &mut chain, &policy, 15).unwrap().matured,
            vec![100]
        );
        let audit = store.audit().unwrap();
        assert!(audit.balanced, "{audit:?}");
        // A second block orphaned before maturity never pays anything.
        let last = store.last_work_seq().unwrap();
        let orphan = store
            .record_block(103, "dd", reward, 6_006_000, last, "g1", 16)
            .unwrap();
        chain.tip = 103;
        chain.hashes.insert(103, "dd".into());
        account(store, &mut chain, &policy, 17).unwrap();
        chain.hashes.insert(103, "ee".into());
        assert_eq!(
            account(store, &mut chain, &policy, 18).unwrap().orphaned,
            vec![103]
        );
        let _ = orphan;
        assert!(store.audit().unwrap().balanced);
        // Payouts: whole gori only; the fraction stays for later.
        let batches = store.plan_payouts(&policy, u64::MAX, 4, 20).unwrap();
        assert_eq!(batches.len(), 1);
        let paid_strong = batches[0]
            .payouts
            .iter()
            .find(|(a, _)| a == "gnrd1strong")
            .unwrap()
            .1;
        assert_eq!(u128::from(paid_strong), strong_immature / ONE_GORI);
        assert!(
            store
                .plan_payouts(&policy, u64::MAX, 4, 21)
                .unwrap()
                .is_empty(),
            "nothing twice"
        );
        let view = store.miner_view("gnrd1strong", target).unwrap().unwrap();
        assert_eq!(view.pending_gori, paid_strong);
        assert_eq!(view.matured_fixed, (strong_immature % ONE_GORI).to_string());
        store.batch_submitted(&batches[0].id, "tx1", 22).unwrap();
        store.batch_submitted(&batches[0].id, "tx1", 22).unwrap();
        assert!(
            store.batch_submitted(&batches[0].id, "tx2", 22).is_err(),
            "one transaction per batch"
        );
        assert_eq!(store.open_batches().unwrap().len(), 1);
        store.batch_confirmed(&batches[0].id, 104, 23).unwrap();
        assert!(
            store.batch_confirmed(&batches[0].id, 104, 23).is_err(),
            "confirmed once"
        );
        assert!(
            store.batch_failed(&batches[0].id, 24).is_err(),
            "settled for good"
        );
        let view = store.miner_view("gnrd1strong", target).unwrap().unwrap();
        assert_eq!(
            (view.pending_gori, view.paid_gori.clone()),
            (0, paid_strong.to_string())
        );
        assert!(store.audit().unwrap().balanced);
        // A deep reorganization after payment: what was paid becomes deficit.
        chain.hashes.insert(100, "zz".into());
        assert_eq!(
            account(store, &mut chain, &policy, 25).unwrap().reversed,
            vec![100]
        );
        let audit = store.audit().unwrap();
        assert!(audit.balanced, "{audit:?}");
        assert_ne!(audit.operator_deficit_fixed, "0");
        // A failed batch returns its amounts.
        let last = store.last_work_seq().unwrap();
        let block2 = store
            .record_block(105, "ff", reward, 6_006_000, last, "g1", 26)
            .unwrap();
        chain.tip = 108;
        for h in 104..=108 {
            chain.hashes.insert(h, format!("h{h}"));
        }
        chain.hashes.insert(105, "ff".into());
        account(store, &mut chain, &policy, 27).unwrap();
        account(store, &mut chain, &policy, 28).unwrap();
        let _ = block2;
        let batches = store.plan_payouts(&policy, u64::MAX, 4, 29).unwrap();
        assert!(!batches.is_empty());
        store.batch_failed(&batches[0].id, 30).unwrap();
        assert!(store.audit().unwrap().balanced);
        assert!(store.open_batches().unwrap().is_empty());
        // The budget is respected: nothing is planned that the wallet cannot pay.
        assert!(
            store
                .plan_payouts(&policy, policy.payout_fee, 4, 31)
                .unwrap()
                .is_empty()
        );
        // The ledger refuses another network or activation height.
        assert!(store.bind(&PoolPolicy::defaults(Network::Testnet)).is_err());
        assert!(
            store
                .bind(&PoolPolicy {
                    activation_height: 7,
                    ..policy.clone()
                })
                .is_err()
        );
        assert!(store.has_state().unwrap());
    }

    #[test]
    fn sqlite_ledger_follows_blocks_to_maturity_payouts_and_reorgs() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("pool.sqlite");
        let mut store = Store::open_sqlite(&path).unwrap();
        exercise(&mut store);
        drop(store);
        // Reopened after a restart: same state, still balanced.
        let mut reopened = Store::open_sqlite(&path).unwrap();
        reopened.bind(&policy()).unwrap();
        assert!(reopened.audit().unwrap().balanced);
    }

    #[test]
    fn below_activation_blocks_are_never_shared() {
        let directory = tempfile::tempdir().unwrap();
        let mut store = Store::open_sqlite(&directory.path().join("p.sqlite")).unwrap();
        let policy = PoolPolicy {
            activation_height: 50,
            ..policy()
        };
        store.bind(&policy).unwrap();
        let miner = store.miner("gnrd1m", 0).unwrap();
        store.record_work("g", &[(miner, 10, 1)], 0).unwrap();
        store.record_block(49, "aa", 100, 10, 1, "g", 0).unwrap();
        let mut chain = FakeChain {
            tip: 60,
            hashes: [(49, "aa".to_owned())].into(),
        };
        assert_eq!(
            account(&mut store, &mut chain, &policy, 1)
                .unwrap()
                .ineligible,
            vec![49]
        );
        assert_eq!(
            store
                .miner_view("gnrd1m", 10)
                .unwrap()
                .unwrap()
                .immature_fixed,
            "0"
        );
    }

    /// Runs against a real PostgreSQL when GENORYN_POOL_POSTGRES names one
    /// (an empty database). Without it, the test says so and checks nothing.
    #[test]
    fn postgresql_ledger_matches_sqlite() {
        let Ok(url) = std::env::var("GENORYN_POOL_POSTGRES") else {
            eprintln!("GENORYN_POOL_POSTGRES not set: PostgreSQL ledger test not run");
            return;
        };
        let mut store = Store::open_postgres(&url).unwrap();
        store
            .tx(|tx| {
                for table in [
                    "payouts", "batches", "balances", "credits", "blocks", "work", "miners", "meta",
                ] {
                    tx.exec(&format!("DELETE FROM {table}"), &[])?;
                }
                tx.exec(
                    "UPDATE operator SET fees = '0', dust = '0', deficit = '0', network_fees = 0",
                    &[],
                )?;
                Ok(())
            })
            .unwrap();
        assert_eq!(store.driver(), "postgresql");
        exercise(&mut store);
    }
}
