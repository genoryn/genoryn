//! The GNR market price, version 2 (1.0.53): `docs/spec/market-price.md`.
//!
//! Only completed swaps whose bitcoin the board found on the Bitcoin chain count, each
//! weighed by that bitcoin. Every quantity is an integer, so anyone with the same swaps
//! computes the same price on any computer:
//!
//! * a price is sat per GNR × [`SCALE`]; a weight is satoshi (times a decay factor);
//! * the window is the last 24 hours when they hold enough swaps spread over the day
//!   ([`FAST_SWAPS`] in [`FAST_BLOCKS`] six-hour blocks), otherwise 72 hours, widened to 7
//!   and then 30 days while it holds fewer than [`MIN_SWAPS`] (the price then shows as
//!   stale, never as fresh);
//! * a swap weighs the bitcoin it moved, less for older swaps (half at the window's end),
//!   never more than its GNR is worth at the median, never more than a fifth of all, and
//!   the swaps of one six-hour block never more than a quarter of all: a burst of swaps,
//!   large or small, cannot outweigh the rest of the window, a lasting move can;
//! * swaps more than 4× from the median, or further from it than five median absolute
//!   deviations (and more than half), are left out;
//! * the price is the weighted median of what remains, rebuilt hour by hour over the last
//!   seven days and moving at most 10 % an hour toward it.
//!
//! The confidence (0 to 100) multiplies four factors: the bitcoin behind the price, the
//! effective number of swaps, how far apart their prices are, and how fresh the newest
//! is. The status names the weakest of them.

/// Prices are sat per GNR times this.
pub const SCALE: u128 = 1_000_000;
pub const ALGORITHM: &str = "gnr-price-2";
/// The least bitcoin a swap must have moved to count (1.0.48): many tiny swaps between
/// one's own wallets cannot carry the price.
pub const MIN_SATS: u64 = 100_000;
/// Swaps needed in a window for a price.
pub const MIN_SWAPS: usize = 3;
/// The windows, narrowest first (the first only for a busy market, see [`FAST_SWAPS`]).
pub const WINDOWS: [u64; 4] = [24 * 3600, 72 * 3600, 7 * 24 * 3600, 30 * 24 * 3600];
/// The 24-hour window needs this many swaps, in at least this many six-hour blocks.
pub const FAST_SWAPS: usize = 8;
pub const FAST_BLOCKS: usize = 3;
/// Swaps are grouped in blocks of this many seconds; one block weighs at most
/// 1/[`MAX_BLOCK_SHARE_DIV`] of all.
pub const BLOCK_SECS: u64 = 6 * 3600;
pub const MAX_BLOCK_SHARE_DIV: u128 = 4;
/// How far back the price is rebuilt, hour by hour.
pub const REBUILD_HOURS: u64 = 7 * 24;
/// The price moves at most this much an hour (per mille).
pub const MOVE_PER_HOUR_PERMILLE: u128 = 100;
/// No swap weighs more than this share of all (one fifth).
pub const MAX_SHARE_DIV: u128 = 5;
/// Swaps further than this factor from the median are left out.
pub const OUTLIER_FACTOR: u128 = 4;
/// ... and so are swaps further from it than this many median absolute deviations, if
/// also more than [`MAD_FLOOR_PPM`] away.
pub const MAD_K: u128 = 5;
pub const MAD_FLOOR_PPM: u128 = 500_000;
/// The bitcoin behind the price at which its liquidity factor is one half (0.01 BTC).
pub const LIQUIDITY_HALF_SATS: u128 = 1_000_000;
/// The effective number of swaps at which their factor is one half.
pub const ESS_HALF: u128 = 3;
/// The price spread (median absolute deviation) at which its factor is one half: 15 %.
pub const SPREAD_HALF_PPM: u128 = 150_000;
/// The newest swap is fresh this long; then the factor falls to a fifth at 72 hours.
pub const FRESH_SECS: u64 = 6 * 3600;
pub const STALE_SECS: u64 = 72 * 3600;

const PPM: u128 = 1_000_000;

/// A completed swap: when the board recorded it, its GNR (krypton), its bitcoin (sat) and
/// the bitcoin the board found taken out of its contract (0: not found).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Swap {
    pub at: u64,
    pub gnr: u64,
    pub sats: u64,
    pub paid: u64,
}

impl Swap {
    /// Sat per GNR × [`SCALE`].
    pub fn price(&self) -> u128 {
        if self.gnr == 0 {
            return 0;
        }
        u128::from(self.sats) * 100_000_000 * SCALE / u128::from(self.gnr)
    }
    /// The bitcoin it counts for: what was found on the Bitcoin chain, never more than
    /// was reported.
    fn moved(&self) -> u64 {
        self.paid.min(self.sats)
    }
}

/// What a window of swaps says the price is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub price: u128,
    pub window_secs: u64,
    /// Swaps behind it, their bitcoin, the effective number of them (× 100), their median
    /// absolute deviation from the price (ppm) and when the newest was recorded.
    pub swaps: usize,
    pub sats: u64,
    pub ess_x100: u128,
    pub mad_ppm: u128,
    pub newest: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Reliable,
    Moderate,
    LowLiquidity,
    LowConfidence,
    Stale,
    Insufficient,
    Unavailable,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Reliable => "reliable",
            Status::Moderate => "moderate",
            Status::LowLiquidity => "low_liquidity",
            Status::LowConfidence => "low_confidence",
            Status::Stale => "stale",
            Status::Insufficient => "insufficient",
            Status::Unavailable => "unavailable",
        }
    }
}

/// The market price now, and what is behind it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Assessment {
    /// The published price (sat per GNR × [`SCALE`]).
    pub price: Option<u128>,
    /// What the swaps say right now (the price moves toward it).
    pub target: Option<Target>,
    pub confidence: u8,
    pub status: Status,
    /// The published price 24 hours ago, for the change.
    pub price_24h_ago: Option<u128>,
    /// Bitcoin of the verified swaps of the last 24 hours (any size).
    pub verified_sats_24h: u64,
    /// Seconds since the newest swap that counts.
    pub freshness_secs: Option<u64>,
}

/// The weighted median of (price, weight), sorted by price: the first price where half
/// the weight is reached; exactly half: the midpoint with the next.
pub fn weighted_median(v: &mut [(u128, u128)]) -> Option<u128> {
    v.sort_by_key(|x| x.0);
    let total: u128 = v.iter().map(|x| x.1).sum();
    if total == 0 {
        return None;
    }
    let mut acc = 0u128;
    for i in 0..v.len() {
        acc += v[i].1;
        if acc * 2 == total
            && let Some(next) = v.get(i + 1)
        {
            return Some((v[i].0 + next.0) / 2);
        }
        if acc * 2 >= total {
            return Some(v[i].0);
        }
    }
    v.last().map(|x| x.0)
}

/// Caps every weight at one fifth of the total (with five swaps or fewer: all the least).
fn cap_shares(w: &mut [u128]) {
    if w.is_empty() {
        return;
    }
    let least = *w.iter().min().unwrap_or(&0);
    if (w.len() as u128) <= MAX_SHARE_DIV {
        w.iter_mut().for_each(|x| *x = least);
        return;
    }
    let mut c = *w.iter().max().unwrap_or(&0);
    for _ in 0..64 {
        let next = w.iter().map(|x| (*x).min(c)).sum::<u128>() / MAX_SHARE_DIV;
        if next >= c {
            break;
        }
        c = next.max(least);
    }
    w.iter_mut().for_each(|x| *x = (*x).min(c));
}

/// Caps the total weight of each group (members sharing a key) at 1/`div` of all, by
/// scaling its members down (with `div` groups or fewer: every group to the least one).
fn cap_groups(w: &mut [u128], key: &[u64], div: u128) {
    let mut totals: std::collections::BTreeMap<u64, u128> = Default::default();
    for (w, k) in w.iter().zip(key) {
        *totals.entry(*k).or_default() += *w;
    }
    let g: Vec<u128> = totals.values().copied().collect();
    if g.len() < 2 {
        return;
    }
    let least = *g.iter().min().unwrap_or(&0);
    let cap = if (g.len() as u128) <= div {
        least
    } else {
        let mut c = *g.iter().max().unwrap_or(&0);
        for _ in 0..64 {
            let next = g.iter().map(|x| (*x).min(c)).sum::<u128>() / div;
            if next >= c {
                break;
            }
            c = next.max(least);
        }
        c
    };
    for (w, k) in w.iter_mut().zip(key) {
        let t = totals[k];
        if t > cap && t > 0 {
            *w = *w * cap / t;
        }
    }
}

fn abs_diff(a: u128, b: u128) -> u128 {
    a.max(b) - a.min(b)
}

/// The target of the swaps eligible at `tau` within `window` seconds before it.
fn target_in(swaps: &[Swap], tau: u64, window: u64) -> Option<Target> {
    let v: Vec<&Swap> = swaps
        .iter()
        .filter(|s| s.at <= tau && s.at + window > tau && s.moved() >= MIN_SATS && s.gnr > 0)
        .collect();
    if v.len() < MIN_SWAPS {
        return None;
    }
    // Bitcoin moved, less for older swaps: half at the window's end.
    let base: Vec<u128> = v
        .iter()
        .map(|s| {
            u128::from(s.moved()) * u128::from(2 * window - (tau - s.at)) / u128::from(2 * window)
        })
        .collect();
    let mut keep: Vec<(&Swap, u128)> = v.iter().copied().zip(base).collect();
    let median_of = |k: &[(&Swap, u128)]| {
        let mut w: Vec<u128> = k.iter().map(|x| x.1).collect();
        cap_shares(&mut w);
        let mut pw: Vec<(u128, u128)> = k.iter().zip(w).map(|(x, w)| (x.0.price(), w)).collect();
        weighted_median(&mut pw)
    };
    // Odd prices out: 4× from the median, then far beyond the usual spread.
    let m = median_of(&keep)?;
    keep.retain(|(s, _)| {
        let p = s.price();
        p * OUTLIER_FACTOR >= m && p <= m * OUTLIER_FACTOR
    });
    if keep.len() >= MIN_SWAPS {
        let m = median_of(&keep)?;
        let mut dev: Vec<(u128, u128)> = keep
            .iter()
            .map(|(s, w)| (abs_diff(s.price(), m) * PPM / m.max(1), *w))
            .collect();
        let mad = weighted_median(&mut dev).unwrap_or(0);
        let limit = (mad * MAD_K).max(MAD_FLOOR_PPM);
        let kept: Vec<(&Swap, u128)> = keep
            .iter()
            .copied()
            .filter(|(s, _)| abs_diff(s.price(), m) * PPM / m.max(1) <= limit)
            .collect();
        if kept.len() >= MIN_SWAPS {
            keep = kept;
        }
    }
    if keep.len() < MIN_SWAPS {
        return None;
    }
    // A swap weighs no more than its GNR is worth at the median.
    let m = median_of(&keep)?;
    for (s, w) in keep.iter_mut() {
        let worth = u128::from(s.gnr) * m / (100_000_000 * SCALE);
        *w = (*w).min(worth.max(1));
    }
    let mut w: Vec<u128> = keep.iter().map(|x| x.1).collect();
    cap_shares(&mut w);
    let blocks: Vec<u64> = keep.iter().map(|(s, _)| s.at / BLOCK_SECS).collect();
    cap_groups(&mut w, &blocks, MAX_BLOCK_SHARE_DIV);
    let mut pw: Vec<(u128, u128)> = keep
        .iter()
        .zip(w.iter())
        .map(|(x, w)| (x.0.price(), *w))
        .collect();
    let price = weighted_median(&mut pw)?;
    let sum: u128 = w.iter().sum();
    let sq: u128 = w.iter().map(|x| x * x).sum();
    let ess_x100 = (sum * sum * 100).checked_div(sq).unwrap_or(0);
    let mut dev: Vec<(u128, u128)> = keep
        .iter()
        .zip(w.iter())
        .map(|((s, _), w)| (abs_diff(s.price(), price) * PPM / price.max(1), *w))
        .collect();
    Some(Target {
        price,
        window_secs: window,
        swaps: keep.len(),
        sats: keep
            .iter()
            .map(|(s, _)| s.moved())
            .fold(0u64, u64::saturating_add),
        ess_x100,
        mad_ppm: weighted_median(&mut dev).unwrap_or(0),
        newest: keep.iter().map(|(s, _)| s.at).max().unwrap_or(0),
    })
}

/// The target at `tau`: the narrowest window with enough swaps (the 24-hour one only with
/// [`FAST_SWAPS`] spread over [`FAST_BLOCKS`] blocks).
pub fn target_at(swaps: &[Swap], tau: u64) -> Option<Target> {
    let day: Vec<&Swap> = swaps
        .iter()
        .filter(|s| s.at <= tau && s.at + WINDOWS[0] > tau && s.moved() >= MIN_SATS && s.gnr > 0)
        .collect();
    let blocks: std::collections::BTreeSet<u64> = day.iter().map(|s| s.at / BLOCK_SECS).collect();
    let busy = day.len() >= FAST_SWAPS && blocks.len() >= FAST_BLOCKS;
    WINDOWS
        .iter()
        .skip(usize::from(!busy))
        .find_map(|w| target_in(swaps, tau, *w))
}

/// The published price at `now`: rebuilt hour by hour over [`REBUILD_HOURS`], each hour
/// moving toward that hour's target by at most [`MOVE_PER_HOUR_PERMILLE`] (in the last,
/// partial hour by its share of that). `None` until some hour has a target.
pub fn published(swaps: &[Swap], now: u64) -> Option<u128> {
    let start = (now / 3600).saturating_sub(REBUILD_HOURS) * 3600;
    let mut price: Option<u128> = None;
    let mut last = start;
    let mut tau = start;
    loop {
        if let Some(t) = target_at(swaps, tau) {
            price = Some(match price {
                None => t.price,
                Some(p) => {
                    let step = u128::from(tau - last);
                    let room = p * MOVE_PER_HOUR_PERMILLE * step / (1000 * 3600);
                    t.price.clamp(p.saturating_sub(room), p + room)
                }
            });
        }
        last = tau;
        if tau == now {
            break;
        }
        tau = (tau + 3600).min(now);
    }
    price
}

/// Confidence 0 to 100: liquidity × effective swaps × spread × freshness.
pub fn confidence(t: &Target, now: u64) -> u8 {
    let sats = u128::from(t.sats);
    let liquidity = sats * PPM / (sats + LIQUIDITY_HALF_SATS);
    let ess = t.ess_x100;
    let sample = ess * PPM / (ess + ESS_HALF * 100);
    let spread = SPREAD_HALF_PPM * SPREAD_HALF_PPM * PPM
        / (SPREAD_HALF_PPM * SPREAD_HALF_PPM + t.mad_ppm * t.mad_ppm);
    let fresh = freshness(now.saturating_sub(t.newest));
    let score = liquidity * sample / PPM * spread / PPM * fresh / PPM;
    (score * 100 / PPM).min(100) as u8
}

/// 1 (ppm) while the newest swap is fresh, then down to a fifth at [`STALE_SECS`].
fn freshness(age: u64) -> u128 {
    if age <= FRESH_SECS {
        return PPM;
    }
    let past = u128::from(age.min(STALE_SECS) - FRESH_SECS);
    let span = u128::from(STALE_SECS - FRESH_SECS);
    PPM - past * (PPM - PPM / 5) / span
}

/// The market price now, with its confidence and status.
pub fn assess(swaps: &[Swap], now: u64) -> Assessment {
    let price = published(swaps, now);
    let target = target_at(swaps, now);
    let verified_sats_24h = swaps
        .iter()
        .filter(|s| s.at <= now && s.at + 86_400 > now && s.paid > 0)
        .map(|s| s.moved())
        .fold(0u64, u64::saturating_add);
    let newest = swaps
        .iter()
        .filter(|s| s.at <= now && s.moved() >= MIN_SATS)
        .map(|s| s.at)
        .max();
    let any_verified = swaps.iter().any(|s| s.paid > 0);
    let conf = target.as_ref().map_or(0, |t| confidence(t, now));
    let status = match (&target, newest) {
        _ if !any_verified => Status::Unavailable,
        (None, _) => Status::Insufficient,
        (Some(_), Some(n)) if now.saturating_sub(n) > STALE_SECS => Status::Stale,
        (Some(t), _) if t.sats < 500_000 || t.ess_x100 < 300 => Status::LowLiquidity,
        _ if conf < 40 => Status::LowConfidence,
        _ if conf < 70 => Status::Moderate,
        _ => Status::Reliable,
    };
    Assessment {
        price,
        price_24h_ago: if now >= 86_400 {
            published(swaps, now - 86_400)
        } else {
            None
        },
        confidence: conf,
        status,
        target,
        verified_sats_24h,
        freshness_secs: newest.map(|n| now.saturating_sub(n)),
    }
}

/// A price as sat per GNR (display only).
pub fn to_f64(p: u128) -> f64 {
    p as f64 / SCALE as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(at: u64, price_sat: u64, sats: u64) -> Swap {
        Swap {
            at,
            gnr: sats * 100_000_000 / price_sat,
            sats,
            paid: sats,
        }
    }

    #[test]
    fn nothing_one_or_two_swaps_give_no_price() {
        let now = 1_000_000;
        assert_eq!(assess(&[], now).status, Status::Unavailable);
        let one = [s(now - 60, 5_000, 200_000)];
        let a = assess(&one, now);
        assert_eq!((a.price, a.status), (None, Status::Insufficient));
        let unverified = [Swap { paid: 0, ..one[0] }; 3];
        assert_eq!(assess(&unverified, now).status, Status::Unavailable);
    }

    #[test]
    fn the_same_swaps_give_the_same_price_bit_for_bit() {
        let now = 2_000 * 3600;
        let v: Vec<Swap> = (0..40)
            .map(|i| s(now - i * 3_000, 4_000 + (i * 37 % 500), 150_000 + i * 1_000))
            .collect();
        let a = assess(&v, now);
        let mut shuffled = v.clone();
        shuffled.reverse();
        assert_eq!(assess(&shuffled, now), a, "order does not matter");
        assert!(a.price.is_some() && a.confidence > 0);
    }

    #[test]
    fn a_quiet_market_shows_its_last_price_as_stale() {
        let now = 3_000 * 3600;
        let v: Vec<Swap> = (0..6)
            .map(|i| s(now - 5 * 86_400 - i * 3_600, 5_000, 300_000))
            .collect();
        let a = assess(&v, now);
        assert_eq!(a.status, Status::Stale);
        assert_eq!(a.price.map(to_f64), Some(5_000.0));
        assert_eq!(a.target.unwrap().window_secs, WINDOWS[2]);
    }

    #[test]
    fn micro_swaps_odd_prices_and_one_whale_do_not_set_the_price() {
        let now = 4_000 * 3600;
        let mut v: Vec<Swap> = (0..24)
            .map(|i| s(now - i * 10_800, 5_000, 400_000))
            .collect();
        // A thousand tiny swaps at ten times the price: below the minimum, they weigh nothing.
        v.extend((0..1_000).map(|i| s(now - i * 60, 50_000, 50_000)));
        // Prices far off.
        v.extend((0..3).map(|i| s(now - i * 600, 100_000, 200_000)));
        // One swap with more bitcoin than all the others together, at twice the price.
        v.push(s(now - 30, 10_000, 50_000_000));
        let p = to_f64(assess(&v, now).price.unwrap());
        assert!((p - 5_000.0).abs() < 1.0, "{p}");
    }

    #[test]
    fn a_real_move_shows_within_hours_and_never_jumps() {
        let now = 5_000 * 3600;
        let mut v: Vec<Swap> = (0..36)
            .map(|i| s(now - 30 * 3_600 - i * 3_600, 5_000, 300_000))
            .collect();
        v.extend((0..30).map(|i| s(now - i * 3_600, 8_000, 300_000)));
        let p = to_f64(published(&v, now).unwrap());
        assert!(
            p > 7_900.0 && p <= 8_000.0,
            "the move shows within a day: {p}"
        );
        let earlier = to_f64(published(&v, now - 28 * 3_600).unwrap());
        let next = to_f64(published(&v, now - 27 * 3_600).unwrap());
        assert!(next <= earlier * 1.1 + 1.0, "{earlier} -> {next}");
    }

    #[test]
    fn integer_arithmetic_does_not_overflow_at_the_limits() {
        let now = u64::from(u32::MAX);
        let big = 100_000_000_000u64; // 1,000 BTC, the most an offer may hold
        let v: Vec<Swap> = (0..2_000)
            .map(|i| Swap {
                at: now - i * 100,
                gnr: 1 + i,
                sats: big,
                paid: big,
            })
            .collect();
        let a = assess(&v, now);
        assert!(a.price.is_some());
        let tiny: Vec<Swap> = (0..5)
            .map(|i| Swap {
                at: now - i,
                gnr: u64::MAX,
                sats: MIN_SATS,
                paid: MIN_SATS,
            })
            .collect();
        assert!(assess(&tiny, now).price.is_some());
    }

    #[test]
    fn confidence_grows_with_bitcoin_swaps_and_agreement() {
        let now = 6_000 * 3600;
        let few: Vec<Swap> = (0..3).map(|i| s(now - i * 600, 5_000, 150_000)).collect();
        let many: Vec<Swap> = (0..40)
            .map(|i| s(now - i * 1_800, 5_000, 2_000_000))
            .collect();
        let noisy: Vec<Swap> = (0..40)
            .map(|i| s(now - i * 1_800, 3_000 + (i % 5) * 1_000, 2_000_000))
            .collect();
        let (cf, cm, cn) = (
            assess(&few, now).confidence,
            assess(&many, now).confidence,
            assess(&noisy, now).confidence,
        );
        assert!(cf < cn && cn < cm, "{cf} {cn} {cm}");
        assert_eq!(assess(&many, now).status, Status::Reliable);
        assert_eq!(assess(&few, now).status, Status::LowLiquidity);
    }

    /// The board's real swaps (a `/v1/swap/market` answer saved to the file `GK_MARKET`):
    /// what v1 and v2 say (cargo test -p krypton-swap real_market -- --ignored --nocapture).
    #[test]
    #[ignore = "reads a saved market answer from GK_MARKET"]
    fn real_market() {
        let v: serde_json::Value =
            serde_json::from_slice(&std::fs::read(std::env::var("GK_MARKET").unwrap()).unwrap())
                .unwrap();
        let now = v["now"].as_u64().unwrap();
        let recent = v["recent"].as_array().unwrap();
        let swaps: Vec<Swap> = recent
            .iter()
            .map(|x| {
                let sats = x["sats"].as_u64().unwrap();
                Swap {
                    at: x["at"].as_u64().unwrap(),
                    gnr: x["gnr"].as_u64().unwrap(),
                    sats,
                    paid: if x["verified"] == true { sats } else { 0 },
                }
            })
            .collect();
        let trades: Vec<(crate::board::Trade, u64)> = swaps
            .iter()
            .enumerate()
            .map(|(i, s)| {
                (
                    crate::board::Trade {
                        network: "mainnet".into(),
                        hash: format!("{i:064x}"),
                        gnr: s.gnr,
                        sats: s.sats,
                        nonce: 0,
                    },
                    s.at,
                )
            })
            .collect();
        let seen = swaps
            .iter()
            .enumerate()
            .filter(|(_, s)| s.paid > 0)
            .map(|(i, s)| (format!("{i:064x}"), s.paid))
            .collect();
        let m = crate::board::market(&trades, &seen, &[], &[], now);
        println!(
            "board now (1.0.52): price {:?} status {} confidence {} depth {}",
            v["market"]["price"],
            v["market"]["status"],
            v["market"]["confidence"],
            v["market"]["depth"]
        );
        println!(
            "1.0.53 on the same swaps: price {:?} reference {:?} status {} market_status {} confidence {} ({}) depth {} priced {} window {:?} ess {:?}",
            m.price,
            m.reference_price,
            m.status,
            m.market_status,
            m.confidence,
            m.confidence_score,
            m.depth,
            m.priced,
            m.window_secs,
            m.ess
        );
    }

    /// Simulations comparing candidate methods (run with --nocapture to see the table):
    /// a known true price, honest swaps around it, and attacks. See docs/spec/market-price.md.
    #[test]
    fn simulations() {
        sim::run(false);
    }
}

/// The simulator behind `docs/spec/market-price.md`: deterministic random markets with a
/// known true price, honest swaps around it, and attacks; each method's error and how much
/// bitcoin an attacker must move.
#[cfg(test)]
pub mod sim {
    use super::*;

    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
        fn unit(&mut self) -> f64 {
            (self.next() >> 11) as f64 / (1u64 << 53) as f64
        }
        /// About normal (sum of uniforms), mean 0, deviation 1.
        fn normal(&mut self) -> f64 {
            (0..12).map(|_| self.unit()).sum::<f64>() - 6.0
        }
    }

    /// The candidate methods, each a price (sat per GNR) from the swaps at `now`.
    /// A pricing method: its name and the price it gives for swaps at a time.
    pub type Method = (&'static str, fn(&[Swap], u64) -> Option<f64>);

    pub fn methods() -> Vec<Method> {
        fn eligible(v: &[Swap], now: u64) -> Vec<Swap> {
            v.iter()
                .filter(|s| s.at <= now && s.at + WINDOWS[1] > now && s.moved() >= MIN_SATS)
                .copied()
                .collect()
        }
        fn vwap(v: &[Swap], now: u64) -> Option<f64> {
            let e = eligible(v, now);
            let (s, g) = e.iter().fold((0f64, 0f64), |a, x| {
                (a.0 + x.moved() as f64, a.1 + x.gnr as f64 / 1e8)
            });
            (g > 0.0).then(|| s / g)
        }
        fn twap(v: &[Swap], now: u64) -> Option<f64> {
            let e = eligible(v, now);
            (!e.is_empty())
                .then(|| e.iter().map(|x| to_f64(x.price())).sum::<f64>() / e.len() as f64)
        }
        fn median(v: &[Swap], now: u64) -> Option<f64> {
            let mut p: Vec<f64> = eligible(v, now).iter().map(|x| to_f64(x.price())).collect();
            p.sort_by(f64::total_cmp);
            (!p.is_empty()).then(|| p[p.len() / 2])
        }
        fn vw_median(v: &[Swap], now: u64) -> Option<f64> {
            let mut pw: Vec<(u128, u128)> = eligible(v, now)
                .iter()
                .map(|x| (x.price(), u128::from(x.moved())))
                .collect();
            weighted_median(&mut pw).map(to_f64)
        }
        fn v1(v: &[Swap], now: u64) -> Option<f64> {
            let trades: Vec<(crate::board::Trade, u64)> = v
                .iter()
                .enumerate()
                .map(|(i, s)| {
                    (
                        crate::board::Trade {
                            network: "mainnet".into(),
                            hash: format!("{i:064x}"),
                            gnr: s.gnr,
                            sats: s.sats,
                            nonce: 0,
                        },
                        s.at,
                    )
                })
                .collect();
            let seen = v
                .iter()
                .enumerate()
                .filter(|(_, s)| s.paid > 0)
                .map(|(i, s)| (format!("{i:064x}"), s.paid))
                .collect();
            crate::board::reference(&trades, &seen, now).map(|r| r.price)
        }
        fn v2(v: &[Swap], now: u64) -> Option<f64> {
            published(v, now).map(to_f64)
        }
        vec![
            ("VWAP 72h", vwap),
            ("TWAP 72h", twap),
            ("median 72h", median),
            ("vw median 72h", vw_median),
            ("v1 (1.0.48)", v1),
            ("v2 (1.0.53)", v2),
        ]
    }

    /// Honest swaps over `hours` ending at `now`, `per_hour` on average, around a true
    /// price that `truth(hour)` gives, with `noise` relative spread.
    fn honest(
        r: &mut Rng,
        now: u64,
        hours: u64,
        per_hour: f64,
        truth: &dyn Fn(u64) -> f64,
        noise: f64,
    ) -> Vec<Swap> {
        let mut v = Vec::new();
        for h in 0..hours {
            let n = (per_hour + r.normal() * per_hour.sqrt() * 0.5)
                .round()
                .max(0.0) as u64;
            for _ in 0..n {
                let at = now - (hours - h) * 3600 + (r.unit() * 3600.0) as u64;
                let p = truth(h) * (1.0 + noise * r.normal());
                let sats = (200_000.0 * (1.0 + 4.0 * r.unit())) as u64;
                v.push(Swap {
                    at,
                    gnr: (sats as f64 * 1e8 / p) as u64,
                    sats,
                    paid: sats,
                });
            }
        }
        v
    }

    /// Swaps of an attacker trading with himself at `factor` × the true price.
    fn attack(r: &mut Rng, now: u64, n: u64, sats: u64, price: f64) -> Vec<Swap> {
        (0..n)
            .map(|_| {
                let at = now - (r.unit() * 6.0 * 3600.0) as u64;
                Swap {
                    at,
                    gnr: (sats as f64 * 1e8 / price) as u64,
                    sats,
                    paid: sats,
                }
            })
            .collect()
    }

    pub fn run(print: bool) {
        let now = 10_000 * 3600;
        let ms = methods();
        let mut out = String::new();
        let mut row = |name: &str, cells: Vec<String>| {
            out += &format!("| {name} | {} |\n", cells.join(" | "));
        };
        // 1. Honest market, 1 swap an hour, 3 % noise: error against the truth.
        let flat = |_h: u64| 5_000.0;
        let mut err = vec![0f64; ms.len()];
        for seed in 1..=20u64 {
            let mut r = Rng(seed * 7919);
            let v = honest(&mut r, now, 96, 1.0, &flat, 0.03);
            for (k, (_, f)) in ms.iter().enumerate() {
                err[k] += f(&v, now).map_or(1.0, |p| (p / 5_000.0 - 1.0).abs()) / 20.0;
            }
        }
        row(
            "Honest market: mean error",
            err.iter().map(|e| format!("{:.2} %", e * 100.0)).collect(),
        );
        // 2. A real move from 5,000 to 8,000 thirty hours ago: where each method is now.
        let step = |h: u64| if h >= 66 { 8_000.0 } else { 5_000.0 };
        let mut r = Rng(42);
        let v = honest(&mut r, now, 96, 1.0, &step, 0.02);
        let moved_v2 = published(&v, now).map_or(0.0, to_f64);
        row(
            "Real move 5,000 → 8,000, 30 h later",
            ms.iter()
                .map(|(_, f)| f(&v, now).map_or("—".into(), |p| format!("{p:.0}")))
                .collect(),
        );
        let mut attacked_v2 = Vec::new();
        // 3. Attacks on a market of 0.36 BTC over 72 h: the price after the attack.
        for (label, n, sats, factor) in [
            (
                "Wash trading: 300 × 0.001 BTC at 2×",
                300u64,
                100_000u64,
                2.0,
            ),
            (
                "Micro-trades: 3,000 × 0.0009 BTC at 10×",
                3_000,
                90_000,
                10.0,
            ),
            ("One whale: 1 × 0.5 BTC at 2×", 1, 50_000_000, 2.0),
            ("Ten swaps: 10 × 0.05 BTC at 1.5×", 10, 5_000_000, 1.5),
            ("Outliers: 5 × 0.002 BTC at 100×", 5, 200_000, 100.0),
        ] {
            let mut r = Rng(99);
            let mut v = honest(&mut r, now, 72, 1.0, &flat, 0.02);
            v.extend(attack(&mut r, now, n, sats, 5_000.0 * factor));
            attacked_v2.push(published(&v, now).map_or(1.0, |p| to_f64(p) / 5_000.0 - 1.0));
            row(
                label,
                ms.iter()
                    .map(|(_, f)| {
                        f(&v, now).map_or("—".into(), |p| {
                            format!("{:+.1} %", (p / 5_000.0 - 1.0) * 100.0)
                        })
                    })
                    .collect(),
            );
        }
        // 4. Calibration of the confidence: error by confidence band over random markets.
        let mut bands = [(0f64, 0u32); 5];
        for seed in 1..=400u64 {
            let mut r = Rng(seed * 104_729);
            let per_hour = [0.05, 0.1, 0.3, 1.0, 3.0][(seed % 5) as usize];
            let noise = [0.01, 0.05, 0.15, 0.3][(seed % 4) as usize];
            let mut v = honest(&mut r, now, 72, per_hour, &flat, noise);
            v.retain(|s| s.at < now - (seed % 3) * 30 * 3600);
            let a = assess(&v, now);
            if let (Some(t), true) = (a.target.as_ref(), a.confidence > 0) {
                let e = (to_f64(t.price) / 5_000.0 - 1.0).abs();
                let b = (a.confidence as usize / 20).min(4);
                bands[b].0 += e;
                bands[b].1 += 1;
            }
        }
        let cal: Vec<String> = bands
            .iter()
            .map(|(e, n)| {
                if *n > 0 {
                    format!("{:.1} % ({n})", e / *n as f64 * 100.0)
                } else {
                    "—".into()
                }
            })
            .collect();
        if print {
            println!(
                "| Scenario | {} |",
                ms.iter().map(|m| m.0).collect::<Vec<_>>().join(" | ")
            );
            println!("|---|{}", "---|".repeat(ms.len()));
            print!("{out}");
            println!(
                "\nConfidence calibration, mean error of the target by confidence band 0–19, 20–39, 40–59, 60–79, 80–100 (markets):\n{}",
                cal.join(" · ")
            );
        }
        // What the method must achieve: v2 is at least as accurate as v1 on an honest
        // market, never moved by the attacks beyond one hour's step, and its confidence
        // means something (more confidence, less error).
        assert!(
            err[5] <= err[4] + 0.005,
            "honest error v2 {} v1 {}",
            err[5],
            err[4]
        );
        assert!(
            attacked_v2.iter().all(|x| x.abs() < 0.10),
            "attacks moved v2: {attacked_v2:?}"
        );
        assert!(
            moved_v2 > 7_900.0,
            "a real move shows within a day: {moved_v2}"
        );
        let means: Vec<f64> = bands
            .iter()
            .filter(|(_, n)| *n >= 5)
            .map(|(e, n)| e / *n as f64)
            .collect();
        assert!(
            means.windows(2).all(|w| w[1] <= w[0] * 1.15 + 0.002),
            "calibration {cal:?}"
        );
    }

    #[test]
    #[ignore = "prints the simulation table: cargo test -p krypton-swap sim_table -- --ignored --nocapture"]
    fn sim_table() {
        run(true);
    }
}
