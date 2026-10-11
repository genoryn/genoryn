//! Variable share difficulty.
//!
//! Each session gets a share difficulty that makes its miner submit about one
//! share every `target_seconds`, whatever its hashrate. A share is credited
//! with exactly its difficulty (its expected number of hashes), so a miner
//! earns the same per hash at any share difficulty: VarDiff changes how often
//! work is reported, not what it is worth.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VarDiffConfig {
    pub initial: u64,
    pub min: u64,
    pub max: u64,
    pub target_seconds: u64,
    /// Retarget after this many shares, or after `retarget_seconds` without.
    pub retarget_shares: u32,
    pub retarget_seconds: u64,
}
impl Default for VarDiffConfig {
    fn default() -> Self {
        // 100 is about 10 seconds of a 10 H/s computer; each step at most
        // quadruples or quarters the difficulty.
        Self {
            initial: 1_000,
            min: 100,
            max: 1_000_000_000_000,
            target_seconds: 15,
            retarget_shares: 8,
            retarget_seconds: 90,
        }
    }
}
impl VarDiffConfig {
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            1 <= self.min && self.min <= self.initial && self.initial <= self.max,
            "VarDiff needs 1 <= min <= initial <= max"
        );
        anyhow::ensure!(
            (1..=600).contains(&self.target_seconds),
            "VarDiff target must be 1-600 seconds"
        );
        anyhow::ensure!(
            self.retarget_shares >= 1 && self.retarget_seconds >= self.target_seconds,
            "VarDiff retarget settings are inconsistent"
        );
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct SessionDifficulty {
    config: VarDiffConfig,
    difficulty: u64,
    since_ms: u64,
    shares: u32,
}
impl SessionDifficulty {
    pub fn new(config: VarDiffConfig, now_ms: u64) -> Self {
        Self {
            config,
            difficulty: config.initial,
            since_ms: now_ms,
            shares: 0,
        }
    }
    pub fn current(&self) -> u64 {
        self.difficulty
    }
    /// Records an accepted share; returns the new difficulty when it changes.
    pub fn on_share(&mut self, now_ms: u64) -> Option<u64> {
        self.shares = self.shares.saturating_add(1);
        if self.shares < self.config.retarget_shares {
            return None;
        }
        self.retarget(now_ms)
    }
    /// Called periodically; lowers the difficulty of a miner that stopped
    /// finding shares at the current one.
    pub fn on_timer(&mut self, now_ms: u64) -> Option<u64> {
        let elapsed = now_ms.saturating_sub(self.since_ms);
        if elapsed < self.config.retarget_seconds * 1_000 {
            return None;
        }
        self.retarget(now_ms)
    }
    fn retarget(&mut self, now_ms: u64) -> Option<u64> {
        let elapsed = now_ms.saturating_sub(self.since_ms).max(1);
        let observed_work = u128::from(self.difficulty) * u128::from(self.shares);
        // Hashrate estimate × target interval, all in integers.
        let wanted =
            observed_work * u128::from(self.config.target_seconds) * 1_000 / u128::from(elapsed);
        let lower = u128::from((self.difficulty / 4).max(1));
        let upper = u128::from(self.difficulty).saturating_mul(4);
        let next = wanted
            .clamp(lower, upper)
            .clamp(u128::from(self.config.min), u128::from(self.config.max))
            as u64;
        self.since_ms = now_ms;
        self.shares = 0;
        (next != self.difficulty).then(|| {
            self.difficulty = next;
            next
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A miner at `hashrate` finds a share of difficulty d every d/hashrate seconds.
    fn converge(hashrate: u64) -> u64 {
        let config = VarDiffConfig::default();
        let mut session = SessionDifficulty::new(config, 0);
        let mut now = 0u64;
        for _ in 0..400 {
            let interval_ms = (session.current() * 1_000 / hashrate).max(1);
            now += interval_ms;
            session.on_share(now);
        }
        session.current()
    }

    #[test]
    fn every_hashrate_settles_near_the_target_interval() {
        for hashrate in [10u64, 100, 500, 1_000, 10_000, 100_000, 10_000_000] {
            let difficulty = converge(hashrate);
            let expected = (hashrate * 15).max(100);
            assert!(
                difficulty * 10 >= expected * 7 && difficulty * 10 <= expected * 13,
                "{hashrate} H/s settled at {difficulty}, expected about {expected}"
            );
        }
    }

    #[test]
    fn a_silent_miner_is_lowered_and_steps_are_bounded() {
        let config = VarDiffConfig::default();
        let mut session = SessionDifficulty::new(config, 0);
        assert_eq!(session.on_timer(10_000), None, "too early");
        assert_eq!(session.on_timer(91_000), Some(250), "a quarter per step");
        assert_eq!(
            session.on_timer(182_000),
            Some(100),
            "never below the minimum"
        );
        let mut fast = SessionDifficulty::new(config, 0);
        for i in 1..=8 {
            fast.on_share(i);
        }
        assert_eq!(fast.current(), 4_000, "at most four times per step");
        assert!(VarDiffConfig { min: 0, ..config }.validate().is_err());
        assert!(
            VarDiffConfig {
                initial: 10,
                ..config
            }
            .validate()
            .is_err()
        );
        assert!(config.validate().is_ok());
    }
}
