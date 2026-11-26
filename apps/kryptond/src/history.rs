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
/// What this file was written with: "figures" -> the version of the days' figures.
const META: TableDefinition<&str, u64> = TableDefinition::new("meta");
/// 2 (1.0.55): each day also carries the figures of its verified swaps alone.
const FIGURES_VERSION: u64 = 2;

const DAY: u64 = 86_400;
const REC_LEN: usize = 8 + 8 + 1 + 1 + 8 + 8 + 8 + 32 + 32 + 8;

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
    /// Its bitcoin was found on the Bitcoin chain, and how much (sat).
    pub verified: bool,
    pub paid: u64,
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
        v.extend_from_slice(&self.paid.to_le_bytes());
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
            paid: u(&v[106..114]),
            disputed: flags & 2 == 2,
            claim,
        })
    }
}

/// A swap's status as it may change after it was recorded.
#[derive(Clone, Debug)]
pub struct Status {
    pub verified: bool,
    pub paid: u64,
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
    /// The same figures over the day's verified swaps alone (1.0.55): the ones whose bitcoin
    /// the board found on the Bitcoin chain. `verified` above is how many they are.
    #[serde(default)]
    pub v_gnr: u64,
    #[serde(default)]
    pub v_sats: u64,
    #[serde(default)]
    pub v_low: Option<f64>,
    #[serde(default)]
    pub v_high: Option<f64>,
    #[serde(default)]
    pub v_median: Option<f64>,
    #[serde(default)]
    pub v_vwap: Option<f64>,
}

/// The median of sorted values.
pub fn median(sorted: &[f64]) -> Option<f64> {
    match sorted.len() {
        0 => None,
        n if n % 2 == 1 => Some(sorted[n / 2]),
        n => Some((sorted[n / 2 - 1] + sorted[n / 2]) / 2.0),
    }
}

/// GNR, bitcoin and prices of a set of swaps: (GNR, sat, low, high, median, average weighed
/// by the bitcoin moved).
type Sums = (u64, u64, Option<f64>, Option<f64>, Option<f64>, Option<f64>);

fn sums(recs: &[&Rec]) -> Sums {
    let mut prices: Vec<f64> = recs.iter().map(|r| r.price()).collect();
    prices.sort_by(f64::total_cmp);
    let gnr = recs.iter().map(|r| r.gnr).fold(0u64, u64::saturating_add);
    let sats = recs.iter().map(|r| r.sats).fold(0u64, u64::saturating_add);
    (
        gnr,
        sats,
        prices.first().copied(),
        prices.last().copied(),
        median(&prices),
        (gnr > 0).then(|| sats as f64 / (gnr as f64 / 1e8)),
    )
}

fn figures(day: u64, recs: &[Rec]) -> DayFig {
    let counted: Vec<&Rec> = recs.iter().filter(|r| !r.disputed && r.gnr > 0).collect();
    let verified: Vec<&Rec> = counted.iter().copied().filter(|r| r.verified).collect();
    let (gnr, sats, low, high, median, vwap) = sums(&counted);
    let (v_gnr, v_sats, v_low, v_high, v_median, v_vwap) = sums(&verified);
    DayFig {
        day,
        all: recs.len() as u64,
        swaps: counted.len() as u64,
        verified: verified.len() as u64,
        gnr,
        sats,
        low,
        high,
        median,
        vwap,
        v_gnr,
        v_sats,
        v_low,
        v_high,
        v_median,
        v_vwap,
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
            let trades = w.open_table(TRADES)?;
            w.open_table(BY_HASH)?;
            let mut days = w.open_table(DAYS)?;
            let mut meta = w.open_table(META)?;
            // Days written by an older version lack the newer figures: every day is worked
            // out again from its swaps, once.
            if meta.get("figures")?.map(|v| v.value()) != Some(FIGURES_VERSION) {
                let mut day: Option<u64> = None;
                let mut recs: Vec<Rec> = Vec::new();
                let mut put = |day: u64, recs: &[Rec]| -> Result<()> {
                    days.insert(day, serde_json::to_vec(&figures(day, recs))?.as_slice())?;
                    Ok(())
                };
                for e in trades.iter()? {
                    let (k, v) = e?;
                    let Some(r) = Rec::decode(k.value(), v.value()) else {
                        continue;
                    };
                    let d = r.at - r.at % DAY;
                    if day != Some(d) {
                        if let Some(old) = day {
                            put(old, &recs)?;
                        }
                        day = Some(d);
                        recs.clear();
                    }
                    recs.push(r);
                }
                if let Some(old) = day {
                    put(old, &recs)?;
                }
                meta.insert("figures", FIGURES_VERSION)?;
            }
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
                    && r.paid == st.paid
                    && r.disputed == st.disputed
                    && r.claim == claim
                    && (r.gnr, r.sats) == (gnr, sats)
                {
                    continue;
                }
                r.verified = st.verified;
                r.paid = st.paid;
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
        let start = if since.is_multiple_of(DAY) {
            first
        } else {
            first + DAY
        };
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

    /// The median price of every swap that counts, and of the verified ones alone (reads
    /// the whole history: called seldom).
    pub fn median_all(&self) -> Result<(Option<f64>, Option<f64>)> {
        let r = self.db.begin_read()?;
        let trades = r.open_table(TRADES)?;
        let mut prices = Vec::new();
        let mut verified = Vec::new();
        for e in trades.iter()? {
            let (k, v) = e?;
            if let Some(rec) = Rec::decode(k.value(), v.value())
                && !rec.disputed
                && rec.gnr > 0
            {
                prices.push(rec.price());
                if rec.verified {
                    verified.push(rec.price());
                }
            }
        }
        prices.sort_by(f64::total_cmp);
        verified.sort_by(f64::total_cmp);
        Ok((median(&prices), median(&verified)))
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
            paid: 0,
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
                        paid: 1_020,
                        disputed: false,
                        claim: Some(claim.clone()),
                        amounts: None,
                    },
                ),
                (
                    [1; 32],
                    Status {
                        verified: false,
                        paid: 0,
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
        assert_eq!((p[0].claim.clone(), p[0].paid), (Some(claim), 1_020));
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
        assert_eq!(h.median_all().unwrap(), (Some(1050.0), Some(1020.0)));
        assert_eq!(h.span().unwrap(), Some((t0, t0 + 9 * 3_600 * 6)));
    }

    /// A history like the early mainnet's: 13 swaps, 7 of them with their bitcoin found, at
    /// prices close to each other, and 6 reported ones at prices far from those. The figures
    /// of the verified swaps are not moved by the others.
    #[test]
    fn verified_figures_stand_apart_from_odd_reports() {
        let d = tempfile::tempdir().unwrap();
        let h = History::open(&d.path().join("h.redb")).unwrap();
        let t0 = 20_000 * DAY;
        let g = 1e8 as u64;
        // (day, GNR, sat per GNR, verified)
        let swaps: [(u64, u64, u64, bool); 13] = [
            (0, 10, 1_000, false),
            (0, 2, 5_000, false),
            (1, 1, 10_000, false),
            (3, 50, 20, false),
            (3, 5, 1_000, false),
            (4, 3, 167, false),
            (19, 4_000, 120, true),
            (21, 100, 120, true),
            (21, 750, 115, true),
            (21, 1_100, 218, true),
            (21, 500, 100, true),
            (21, 100, 120, true),
            (22, 250, 130, true),
        ];
        let new: Vec<Rec> = swaps
            .iter()
            .enumerate()
            .map(|(k, (day, gnr, px, ok))| Rec {
                at: t0 + day * DAY + 600 * k as u64,
                hash: [k as u8 + 1; 32],
                gnr: gnr * g,
                sats: gnr * px,
                verified: *ok,
                paid: if *ok { gnr * px } else { 0 },
                disputed: false,
                claim: None,
            })
            .collect();
        h.write(&new, &[]).unwrap();
        let days = h.days().unwrap();
        assert_eq!(days.len(), 7, "seven days saw a swap");
        assert_eq!(days.iter().map(|d| d.swaps).sum::<u64>(), 13);
        assert_eq!(days.iter().map(|d| d.verified).sum::<u64>(), 7);
        // Every reported swap: from 20 to 10,000 sat.
        let low = days.iter().filter_map(|d| d.low).fold(f64::MAX, f64::min);
        let high = days.iter().filter_map(|d| d.high).fold(0.0, f64::max);
        assert_eq!((low, high), (20.0, 10_000.0));
        // The verified ones alone: from 100 to 218 sat.
        let v_low = days.iter().filter_map(|d| d.v_low).fold(f64::MAX, f64::min);
        let v_high = days.iter().filter_map(|d| d.v_high).fold(0.0, f64::max);
        assert_eq!((v_low, v_high), (100.0, 218.0));
        let (all, verified) = h.median_all().unwrap();
        assert_eq!(verified, Some(120.0));
        assert_eq!(all, Some(130.0));
        // A day with only unverified swaps has no verified figures at all.
        let first = &days[0];
        assert_eq!((first.swaps, first.verified, first.v_gnr), (2, 0, 0));
        assert_eq!(
            (first.v_low, first.v_median, first.v_vwap),
            (None, None, None)
        );
        // The busiest day: five verified swaps.
        let busy = days.iter().find(|d| d.day == t0 + 21 * DAY).unwrap();
        assert_eq!((busy.swaps, busy.verified), (5, 5));
        assert_eq!(busy.v_gnr, 2_550 * g);
        assert_eq!(busy.v_median, Some(120.0));
        assert_eq!((busy.v_low, busy.v_high), (Some(100.0), Some(218.0)));
        // A report proven later joins the verified figures of its day.
        h.write(
            &[],
            &[(
                [6; 32],
                Status {
                    verified: true,
                    paid: 3 * 167,
                    disputed: false,
                    claim: None,
                    amounts: None,
                },
            )],
        )
        .unwrap();
        let d4 = h
            .days()
            .unwrap()
            .into_iter()
            .find(|d| d.day == t0 + 4 * DAY)
            .unwrap();
        assert_eq!((d4.verified, d4.v_low, d4.v_gnr), (1, Some(167.0), 3 * g));
    }

    /// Days written before 1.0.55 have no figures of the verified swaps: opening the history
    /// works every day out again, once.
    #[test]
    fn old_days_are_worked_out_again() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("h.redb");
        let t0 = 300 * DAY;
        {
            let h = History::open(&path).unwrap();
            let mut a = rec(t0 + 10, 1, 1e8 as u64, 120);
            a.verified = true;
            let b = rec(t0 + 20, 2, 1e8 as u64, 9_000);
            let mut c = rec(t0 + DAY + 5, 3, 2e8 as u64, 260);
            c.verified = true;
            h.write(&[a, b, c], &[]).unwrap();
        }
        // What an older version left behind: days without the newer fields, no version.
        {
            let db = Database::create(&path).unwrap();
            let w = db.begin_write().unwrap();
            {
                let mut days = w.open_table(DAYS).unwrap();
                for day in [t0, t0 + DAY] {
                    let old = serde_json::json!({"day": day, "all": 9, "swaps": 9, "verified": 9,
                        "gnr": 1, "sats": 1, "low": 1.0, "high": 1.0, "median": 1.0, "vwap": 1.0});
                    days.insert(day, serde_json::to_vec(&old).unwrap().as_slice())
                        .unwrap();
                }
                w.open_table(META).unwrap().remove("figures").unwrap();
            }
            w.commit().unwrap();
        }
        let h = History::open(&path).unwrap();
        let days = h.days().unwrap();
        assert_eq!(days.len(), 2);
        assert_eq!((days[0].swaps, days[0].verified), (2, 1));
        assert_eq!((days[0].low, days[0].high), (Some(120.0), Some(9_000.0)));
        assert_eq!(
            (days[0].v_low, days[0].v_high, days[0].v_median),
            (Some(120.0), Some(120.0), Some(120.0))
        );
        assert_eq!(
            (days[1].verified, days[1].v_gnr, days[1].v_vwap),
            (1, 2e8 as u64, Some(130.0))
        );
        // Opening again changes nothing.
        drop(h);
        assert_eq!(History::open(&path).unwrap().days().unwrap(), days);
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
                        paid: 0,
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
