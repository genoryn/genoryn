//! The trade history on disk (1.0.53): every completed swap the board has recorded, in a
//! database of its own next to the board's files, with figures per day kept up to date as
//! swaps are written. Pages and figures are read from it without loading the whole
//! history, so it can hold millions of swaps.

use anyhow::{Context, Result};
use redb::{Database, ReadableTable, ReadableTableMetadata, TableDefinition};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// (time, big-endian) ++ hash -> record.
const TRADES: TableDefinition<&[u8], &[u8]> = TableDefinition::new("trades");
/// hash -> time.
const BY_HASH: TableDefinition<&[u8], u64> = TableDefinition::new("by_hash");
/// Start of a UTC day -> its figures (JSON).
const DAYS: TableDefinition<u64, &[u8]> = TableDefinition::new("days");

const DAY: u64 = 86_400;
const REC_LEN: usize = 8 + 8 + 1 + 1 + 8 + 8 + 8 + 32 + 32;

/// What the GNR chain shows of a swap's claim.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClaimRec {
    pub height: u64,
    pub time: u64,
    pub refund_height: u64,
    pub txid: [u8; 32],
    pub secret: [u8; 32],
}

/// A completed swap as the history keeps it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rec {
    /// When the board recorded it.
    pub at: u64,
    pub hash: [u8; 32],
    pub gnr: u64,
    pub sats: u64,
    /// Its bitcoin was found on the Bitcoin chain.
    pub verified: bool,
    /// Its two wallets reported different amounts: shown, never counted.
    pub disputed: bool,
    pub claim: Option<ClaimRec>,
}

impl Rec {
    pub fn price(&self) -> f64 {
        self.sats as f64 / (self.gnr as f64 / 1e8)
    }

    fn key(&self) -> [u8; 40] {
        let mut k = [0u8; 40];
        k[..8].copy_from_slice(&self.at.to_be_bytes());
        k[8..].copy_from_slice(&self.hash);
        k
    }

    fn encode(&self) -> Vec<u8> {
        let mut v = Vec::with_capacity(REC_LEN);
        v.extend_from_slice(&self.gnr.to_le_bytes());
        v.extend_from_slice(&self.sats.to_le_bytes());
        v.push(u8::from(self.verified) | (u8::from(self.disputed) << 1));
        let c = self.claim.clone();
        v.push(u8::from(c.is_some()));
        let c = c.unwrap_or(ClaimRec {
            height: 0,
            time: 0,
            refund_height: 0,
            txid: [0; 32],
            secret: [0; 32],
        });
        v.extend_from_slice(&c.height.to_le_bytes());
        v.extend_from_slice(&c.time.to_le_bytes());
        v.extend_from_slice(&c.refund_height.to_le_bytes());
        v.extend_from_slice(&c.txid);
        v.extend_from_slice(&c.secret);
        v
    }

    fn decode(key: &[u8], v: &[u8]) -> Option<Rec> {
        if key.len() != 40 || v.len() != REC_LEN {
            return None;
        }
        let u = |b: &[u8]| u64::from_le_bytes(b.try_into().unwrap_or([0; 8]));
        let flags = v[16];
        let claim = (v[17] == 1).then(|| ClaimRec {
            height: u(&v[18..26]),
            time: u(&v[26..34]),
            refund_height: u(&v[34..42]),
            txid: v[42..74].try_into().unwrap_or([0; 32]),
            secret: v[74..106].try_into().unwrap_or([0; 32]),
        });
        Some(Rec {
            at: u64::from_be_bytes(key[..8].try_into().ok()?),
            hash: key[8..].try_into().ok()?,
            gnr: u(&v[0..8]),
            sats: u(&v[8..16]),
            verified: flags & 1 == 1,
            disputed: flags & 2 == 2,
            claim,
        })
    }
}

/// A swap's status as it may change after it was recorded.
#[derive(Clone, Debug)]
pub struct Status {
    pub verified: bool,
    pub disputed: bool,
    pub claim: Option<ClaimRec>,
    /// New amounts (GNR, sat): a proven report stands in for an unproven one.
    pub amounts: Option<(u64, u64)>,
}

/// The figures of one day: swaps that count (not disputed), and all of them for paging.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct DayFig {
    pub day: u64,
    /// Every swap of the day, disputed ones included.
    pub all: u64,
    pub swaps: u64,
    pub verified: u64,
    pub gnr: u64,
    pub sats: u64,
    pub low: Option<f64>,
    pub high: Option<f64>,
    pub median: Option<f64>,
    pub vwap: Option<f64>,
}

/// The median of sorted values.
pub fn median(sorted: &[f64]) -> Option<f64> {
    match sorted.len() {
        0 => None,
        n if n % 2 == 1 => Some(sorted[n / 2]),
        n => Some((sorted[n / 2 - 1] + sorted[n / 2]) / 2.0),
    }
}

fn figures(day: u64, recs: &[Rec]) -> DayFig {
    let counted: Vec<&Rec> = recs.iter().filter(|r| !r.disputed && r.gnr > 0).collect();
    let mut prices: Vec<f64> = counted.iter().map(|r| r.price()).collect();
    prices.sort_by(f64::total_cmp);
    let gnr = counted
        .iter()
        .map(|r| r.gnr)
        .fold(0u64, u64::saturating_add);
    let sats = counted
        .iter()
        .map(|r| r.sats)
        .fold(0u64, u64::saturating_add);
    DayFig {
        day,
        all: recs.len() as u64,
        swaps: counted.len() as u64,
        verified: counted.iter().filter(|r| r.verified).count() as u64,
        gnr,
        sats,
        low: prices.first().copied(),
        high: prices.last().copied(),
        median: median(&prices),
        vwap: (gnr > 0).then(|| sats as f64 / (gnr as f64 / 1e8)),
    }
}

pub struct History {
    db: Database,
}

impl History {
    pub fn open(path: &Path) -> Result<Self> {
        let db = Database::create(path).context("open the trade history")?;
        let w = db.begin_write()?;
        {
            w.open_table(TRADES)?;
            w.open_table(BY_HASH)?;
            w.open_table(DAYS)?;
        }
        w.commit()?;
        Ok(Self { db })
    }

    pub fn count(&self) -> Result<u64> {
        let r = self.db.begin_read()?;
        Ok(r.open_table(TRADES)?.len()?)
    }

    pub fn contains(&self, hash: &[u8; 32]) -> Result<bool> {
        let r = self.db.begin_read()?;
        Ok(r.open_table(BY_HASH)?.get(hash.as_slice())?.is_some())
    }

    /// Records new swaps (a hash already recorded is skipped) and the new status of
    /// recorded ones, in one transaction; the figures of every day touched are worked out
    /// again from that day's swaps.
    pub fn write(&self, new: &[Rec], updates: &[([u8; 32], Status)]) -> Result<()> {
        if new.is_empty() && updates.is_empty() {
            return Ok(());
        }
        let w = self.db.begin_write()?;
        {
            let mut trades = w.open_table(TRADES)?;
            let mut by_hash = w.open_table(BY_HASH)?;
            let mut touched = std::collections::BTreeSet::new();
            for r in new {
                if by_hash.get(r.hash.as_slice())?.is_some() {
                    continue;
                }
                trades.insert(r.key().as_slice(), r.encode().as_slice())?;
                by_hash.insert(r.hash.as_slice(), r.at)?;
                touched.insert(r.at - r.at % DAY);
            }
            for (hash, st) in updates {
                let Some(at) = by_hash.get(hash.as_slice())?.map(|v| v.value()) else {
                    continue;
                };
                let mut k = [0u8; 40];
                k[..8].copy_from_slice(&at.to_be_bytes());
                k[8..].copy_from_slice(hash);
                let old = trades
                    .get(k.as_slice())?
                    .and_then(|v| Rec::decode(&k, v.value()));
                let Some(mut r) = old else {
                    continue;
                };
                let claim = st.claim.clone().or(r.claim.clone());
                let (gnr, sats) = st.amounts.unwrap_or((r.gnr, r.sats));
                if r.verified == st.verified
                    && r.disputed == st.disputed
                    && r.claim == claim
                    && (r.gnr, r.sats) == (gnr, sats)
                {
                    continue;
                }
                r.verified = st.verified;
                r.disputed = st.disputed;
                r.claim = claim;
                r.gnr = gnr;
                r.sats = sats;
                trades.insert(k.as_slice(), r.encode().as_slice())?;
                touched.insert(at - at % DAY);
            }
            let mut days = w.open_table(DAYS)?;
            for day in touched {
                let lo = day.to_be_bytes();
                let hi = (day + DAY).to_be_bytes();
                let mut recs = Vec::new();
                for e in trades.range(lo.as_slice()..hi.as_slice())? {
                    let (k, v) = e?;
                    if let Some(r) = Rec::decode(k.value(), v.value()) {
                        recs.push(r);
                    }
                }
                let f = figures(day, &recs);
                days.insert(day, serde_json::to_vec(&f)?.as_slice())?;
            }
        }
        w.commit()?;
        Ok(())
    }

    /// Swaps newest first: those recorded at or after `since`, only verified ones if asked,
    /// `limit` of them after skipping `offset`; and how many match in all.
    pub fn page(
        &self,
        offset: usize,
        limit: usize,
        since: u64,
        verified: bool,
    ) -> Result<(Vec<Rec>, u64)> {
        let r = self.db.begin_read()?;
        let trades = r.open_table(TRADES)?;
        let lo = since.to_be_bytes();
        let mut out = Vec::with_capacity(limit);
        let mut seen = 0usize;
        for e in trades.range(lo.as_slice()..)?.rev() {
            if out.len() >= limit {
                break;
            }
            let (k, v) = e?;
            let Some(rec) = Rec::decode(k.value(), v.value()) else {
                continue;
            };
            if verified && !(rec.verified && !rec.disputed) {
                continue;
            }
            if seen >= offset {
                out.push(rec);
            }
            seen += 1;
        }
        Ok((out, self.total(since, verified)?))
    }

    /// How many swaps match: from the days' figures, with the first, partial day counted
    /// swap by swap.
    fn total(&self, since: u64, verified: bool) -> Result<u64> {
        let r = self.db.begin_read()?;
        if since == 0 && !verified {
            return Ok(r.open_table(TRADES)?.len()?);
        }
        let first = since - since % DAY;
        let mut n = 0u64;
        let days = r.open_table(DAYS)?;
        let start = if since.is_multiple_of(DAY) { first } else { first + DAY };
        for e in days.range(start..)? {
            let (_, v) = e?;
            let f: DayFig = serde_json::from_slice(v.value()).unwrap_or_default();
            n += if verified { f.verified } else { f.all };
        }
        if !since.is_multiple_of(DAY) {
            let trades = r.open_table(TRADES)?;
            let lo = since.to_be_bytes();
            let hi = start.to_be_bytes();
            for e in trades.range(lo.as_slice()..hi.as_slice())? {
                let (k, v) = e?;
                if let Some(rec) = Rec::decode(k.value(), v.value())
                    && (!verified || (rec.verified && !rec.disputed))
                {
                    n += 1;
                }
            }
        }
        Ok(n)
    }

    /// Every day's figures, oldest first.
    pub fn days(&self) -> Result<Vec<DayFig>> {
        let r = self.db.begin_read()?;
        let days = r.open_table(DAYS)?;
        let mut out = Vec::new();
        for e in days.iter()? {
            let (_, v) = e?;
            if let Ok(f) = serde_json::from_slice::<DayFig>(v.value()) {
                out.push(f);
            }
        }
        Ok(out)
    }

    /// The median price of every swap that counts (reads the whole history: called seldom).
    pub fn median_all(&self) -> Result<Option<f64>> {
        let r = self.db.begin_read()?;
        let trades = r.open_table(TRADES)?;
        let mut prices = Vec::new();
        for e in trades.iter()? {
            let (k, v) = e?;
            if let Some(rec) = Rec::decode(k.value(), v.value())
                && !rec.disputed
                && rec.gnr > 0
            {
                prices.push(rec.price());
            }
        }
        prices.sort_by(f64::total_cmp);
        Ok(median(&prices))
    }

    /// The first and last recording times.
    pub fn span(&self) -> Result<Option<(u64, u64)>> {
        let r = self.db.begin_read()?;
        let trades = r.open_table(TRADES)?;
        let first = trades.first()?.map(|(k, _)| k.value()[..8].to_vec());
        let last = trades.last()?.map(|(k, _)| k.value()[..8].to_vec());
        let at = |b: Vec<u8>| u64::from_be_bytes(b.try_into().unwrap_or([0; 8]));
        Ok(first.zip(last).map(|(a, b)| (at(a), at(b))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(at: u64, n: u8, gnr: u64, sats: u64) -> Rec {
        Rec {
            at,
            hash: [n; 32],
            gnr,
            sats,
            verified: false,
            disputed: false,
            claim: None,
        }
    }

    #[test]
    fn records_pages_days_and_status() {
        let d = tempfile::tempdir().unwrap();
        let h = History::open(&d.path().join("h.redb")).unwrap();
        let t0 = 100 * DAY;
        let new: Vec<Rec> = (0..10u8)
            .map(|k| {
                rec(
                    t0 + u64::from(k) * 3_600 * 6,
                    k,
                    1e8 as u64,
                    1_000 + u64::from(k) * 10,
                )
            })
            .collect();
        h.write(&new, &[]).unwrap();
        // Recorded once: a hash seen before is skipped.
        h.write(&[rec(t0 + 5, 3, 9, 9)], &[]).unwrap();
        assert_eq!(h.count().unwrap(), 10);
        assert!(h.contains(&[3; 32]).unwrap() && !h.contains(&[42; 32]).unwrap());
        let (p, total) = h.page(0, 4, 0, false).unwrap();
        assert_eq!(total, 10);
        assert_eq!(
            p.iter().map(|r| r.hash[0]).collect::<Vec<_>>(),
            vec![9, 8, 7, 6]
        );
        let (p, _) = h.page(8, 4, 0, false).unwrap();
        assert_eq!(p.iter().map(|r| r.hash[0]).collect::<Vec<_>>(), vec![1, 0]);
        // Since a time inside a day: the partial day is counted swap by swap.
        let since = t0 + 3_600 * 6 * 5;
        let (p, total) = h.page(0, 100, since, false).unwrap();
        assert_eq!((p.len(), total), (5, 5));
        let days = h.days().unwrap();
        assert_eq!(days.iter().map(|d| d.swaps).sum::<u64>(), 10);
        assert_eq!(days[0].median, Some(1015.0));
        // A swap found on the Bitcoin chain later, and one disputed.
        let claim = ClaimRec {
            height: 7,
            time: t0,
            refund_height: 99,
            txid: [1; 32],
            secret: [2; 32],
        };
        h.write(
            &[],
            &[
                (
                    [2; 32],
                    Status {
                        verified: true,
                        disputed: false,
                        claim: Some(claim.clone()),
                        amounts: None,
                    },
                ),
                (
                    [1; 32],
                    Status {
                        verified: false,
                        disputed: true,
                        claim: None,
                        amounts: Some((2e8 as u64, 5)),
                    },
                ),
            ],
        )
        .unwrap();
        let (p, total) = h.page(0, 10, 0, true).unwrap();
        assert_eq!((p.len(), total), (1, 1));
        assert_eq!(p[0].claim, Some(claim));
        let days = h.days().unwrap();
        assert_eq!(
            days.iter().map(|d| d.swaps).sum::<u64>(),
            9,
            "the disputed one does not count"
        );
        assert_eq!(days.iter().map(|d| d.all).sum::<u64>(), 10);
        // It all survives a restart.
        drop(h);
        let h = History::open(&d.path().join("h.redb")).unwrap();
        assert_eq!(h.count().unwrap(), 10);
        assert_eq!(h.median_all().unwrap(), Some(1050.0));
        assert_eq!(h.span().unwrap(), Some((t0, t0 + 9 * 3_600 * 6)));
    }

    /// A million swaps: a page and the figures stay fast (run with --release --ignored).
    #[test]
    #[ignore = "writes a million swaps: cargo test --release -p kryptond -- --ignored"]
    fn a_million_swaps() {
        let d = tempfile::tempdir().unwrap();
        let h = History::open(&d.path().join("h.redb")).unwrap();
        let start = std::time::Instant::now();
        let t0 = 1_700_000_000u64;
        for chunk in 0..100u64 {
            let new: Vec<Rec> = (0..10_000u64)
                .map(|k| {
                    let n = chunk * 10_000 + k;
                    let mut hash = [0u8; 32];
                    hash[..8].copy_from_slice(&n.to_le_bytes());
                    Rec {
                        at: t0 + n * 30,
                        hash,
                        gnr: 1e8 as u64 + n % 977,
                        sats: 1_000 + n % 313,
                        verified: n % 3 == 0,
                        disputed: false,
                        claim: None,
                    }
                })
                .collect();
            h.write(&new, &[]).unwrap();
        }
        let written = start.elapsed();
        assert_eq!(h.count().unwrap(), 1_000_000);
        let t = std::time::Instant::now();
        let (p, total) = h.page(0, 50, 0, false).unwrap();
        let first_page = t.elapsed();
        let t = std::time::Instant::now();
        let (_, vt) = h.page(0, 50, t0 + 20 * DAY, true).unwrap();
        let filtered = t.elapsed();
        let t = std::time::Instant::now();
        let days = h.days().unwrap();
        let figures = t.elapsed();
        let t = std::time::Instant::now();
        let deep = h.page(100_000, 50, 0, false).unwrap().0.len();
        let deep_page = t.elapsed();
        println!(
            "1,000,000 swaps written in {written:?}; first page {first_page:?} ({} of {total}); verified since day 20: {filtered:?} ({vt}); {} days of figures {figures:?}; page at 100,000: {deep_page:?} ({deep})",
            p.len(),
            days.len()
        );
        assert!(first_page.as_millis() < 100 && figures.as_millis() < 500);
    }
}
