//! Bounded integer LWMA-style per-block retarget. See ADR 0005.
pub const WINDOW: usize = 60;
pub const TARGET: i128 = 60;
pub const MAX_DIFFICULTY: u64 = 1_000_000_000_000_000;

/// Samples are (timestamp, difficulty), oldest first. Need one extra timestamp.
pub fn next(samples: &[(u64, u64)], initial: u64) -> u64 {
    if samples.len() < 11 {
        return initial;
    }
    let samples = &samples[samples.len().saturating_sub(WINDOW + 1)..];
    let n = samples.len() - 1;
    let mut weighted = 0i128;
    let mut sum_difficulty = 0u128;
    for i in 1..=n {
        let elapsed =
            (samples[i].0 as i128 - samples[i - 1].0 as i128).clamp(-6 * TARGET, 6 * TARGET);
        weighted += elapsed * i as i128;
        sum_difficulty += samples[i].1 as u128;
    }
    let weight_sum = (n * (n + 1) / 2) as i128;
    let weighted = weighted.max(weight_sum * TARGET / 10) as u128;
    let raw = sum_difficulty * TARGET as u128 * weight_sum as u128 / (n as u128 * weighted);
    let previous = samples[n].1;
    // At most 25% in either direction per block; never zero.
    (raw.min(MAX_DIFFICULTY as u128) as u64)
        .clamp(
            (previous * 3 / 4).max(1),
            (previous.saturating_mul(5) / 4).max(previous + 1),
        )
        .max(1)
}
pub fn median_time(timestamps: &[u64]) -> u64 {
    let mut times: Vec<_> = timestamps.iter().rev().take(11).copied().collect();
    times.sort_unstable();
    times.get(times.len() / 2).copied().unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn simulate(factor: u64) -> Vec<u64> {
        let mut history = vec![(0, 10_000)];
        let mut results = Vec::new();
        for i in 0..800 {
            let d = next(&history, 10_000);
            let rate = if i < 100 { 10_000 } else { factor };
            let dt = (60u128 * d as u128 / rate as u128).max(1) as u64;
            history.push((history.last().unwrap().0 + dt, d));
            if i > 700 {
                results.push(dt);
            }
        }
        results
    }
    #[test]
    fn stable_spike_and_drop_recover() {
        for hash in [1_000, 10_000, 100_000] {
            let intervals = simulate(hash);
            let mean = intervals.iter().sum::<u64>() / intervals.len() as u64;
            assert!((50..=70).contains(&mean), "hashrate {hash}: {mean}s");
        }
    }
    #[test]
    fn malicious_time_and_partition_stay_bounded() {
        for delta in [0, 1, 60, 600, 3600] {
            let samples: Vec<_> = (0..61).map(|n| (n * delta, 10_000)).collect();
            assert!((7500..=12500).contains(&next(&samples, 10_000)));
        }
        let mut reverse: Vec<_> = (0..61).map(|n| (10_000 - n * 100, 10_000)).collect();
        assert_eq!(next(&reverse, 10_000), 12_500);
        reverse.reverse();
        assert!(next(&reverse, 10_000) > 0);
    }
    #[test]
    fn oscillating_miners_cannot_escape_bounds() {
        let mut s = vec![(0, 1000)];
        for i in 1..1000 {
            let d = next(&s, 1000);
            let prev = s.last().unwrap().1;
            if s.len() > 10 {
                assert!(d >= (prev * 3 / 4).max(1) && d <= (prev * 5 / 4).max(prev + 1));
            }
            s.push((s.last().unwrap().0 + if i % 20 < 10 { 6 } else { 600 }, d));
        }
    }
}
