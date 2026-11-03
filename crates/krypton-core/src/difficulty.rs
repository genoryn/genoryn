//! LWMA-1 difficulty adjustment (linearly weighted moving average), retargeted
//! every block over the last 90 blocks, integer arithmetic only.

pub const TARGET_SPACING: u64 = 60;
pub const WINDOW: usize = 90;
pub const MIN_DIFFICULTY: u128 = 1;

/// `history` holds (timestamp, difficulty) of the most recent blocks, oldest first,
/// ending with the parent of the block being created.
pub fn next_difficulty(history: &[(u64, u128)], initial: u128) -> u128 {
    if history.len() < 7 {
        return initial;
    }
    let n = (history.len() - 1).min(WINDOW);
    let h = &history[history.len() - n - 1..];
    let t = TARGET_SPACING as u128;
    let mut weighted: u128 = 0;
    let mut sum_d: u128 = 0;
    let mut prev_ts = h[0].0;
    for (i, &(ts, d)) in h.iter().enumerate().skip(1) {
        let this_ts = ts.max(prev_ts + 1);
        let solve = (this_ts - prev_ts).min(6 * TARGET_SPACING) as u128;
        prev_ts = this_ts;
        weighted += solve * i as u128;
        sum_d = sum_d.saturating_add(d);
    }
    let n = n as u128;
    let floor = n * n * t / 20;
    let weighted = weighted.max(floor).max(1);
    // next = avg(D) * T * n(n+1)/2 / weighted  =  sum_d * T * (n+1) / (2 * weighted)
    let next = sum_d.saturating_mul(t).saturating_mul(n + 1) / (2 * weighted);
    next.max(MIN_DIFFICULTY)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn simulate(hashrate: u128, start: u128, blocks: usize) -> Vec<(u64, u128)> {
        let mut hist = vec![(0u64, start)];
        let mut ts = 0u64;
        for _ in 0..blocks {
            let d = next_difficulty(&hist[hist.len().saturating_sub(WINDOW + 1)..], start);
            ts += ((d / hashrate) as u64).max(1);
            hist.push((ts, d));
        }
        hist
    }
    #[test]
    fn converges_to_target_spacing() {
        let hist = simulate(1_000_000, 1_000, 600);
        let last = hist.last().unwrap().1;
        let ideal = 60_000_000u128;
        assert!(
            last > ideal * 8 / 10 && last < ideal * 12 / 10,
            "difficulty {last} vs {ideal}"
        );
    }
    #[test]
    fn stable_with_constant_hashrate() {
        let hist = simulate(500, 30_000, 400);
        for w in hist[300..].windows(2) {
            let st = w[1].0 - w[0].0;
            assert!((40..=80).contains(&st), "solve time {st}");
        }
    }
    #[test]
    fn future_and_out_of_order_timestamps_are_bounded() {
        let mut hist: Vec<(u64, u128)> = (0..100).map(|i| (i * 60, 1000)).collect();
        hist.push((10_000_000, 1000));
        let d = next_difficulty(&hist, 1000);
        assert!(
            d > 850,
            "one far-future timestamp must not collapse difficulty: {d}"
        );
        let mut hist: Vec<(u64, u128)> = (0..100).map(|i| (i * 60, 1000)).collect();
        hist.push((0, 1000));
        assert!(next_difficulty(&hist, 1000) <= 1100);
    }
}
