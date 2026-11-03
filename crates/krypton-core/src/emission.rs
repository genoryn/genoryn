//! Bitcoin-style issuance: 5 GNR per 60-second block, halving every 2,100,000
//! blocks (about four years), at most 21,000,000 GNR. No premine, no developer
//! fund, no early bonus: the genesis block issues nothing.

pub const COIN: u64 = 100_000_000;
pub const INITIAL_SUBSIDY: u64 = 5 * COIN;
pub const HALVING_INTERVAL: u64 = 2_100_000;
pub const MAX_SUPPLY: u64 = 21_000_000 * COIN;

/// Block subsidy at `height` (height 0 is genesis and issues nothing).
pub fn subsidy(height: u64) -> u64 {
    if height == 0 {
        return 0;
    }
    let halvings = (height - 1) / HALVING_INTERVAL;
    if halvings >= 64 {
        0
    } else {
        INITIAL_SUBSIDY >> halvings
    }
}

/// Total issued up to and including `height`, computed in closed form.
pub fn issued_through(height: u64) -> u64 {
    if height == 0 {
        return 0;
    }
    let mut total: u128 = 0;
    let mut remaining = height;
    let mut epoch = 0u32;
    while remaining > 0 && epoch < 64 {
        let n = remaining.min(HALVING_INTERVAL);
        total += n as u128 * (INITIAL_SUBSIDY >> epoch) as u128;
        remaining -= n;
        epoch += 1;
    }
    total as u64
}

/// Height after which no new coins are issued.
pub fn last_subsidy_height() -> u64 {
    let mut epoch = 0u64;
    while INITIAL_SUBSIDY >> epoch > 0 {
        epoch += 1;
    }
    epoch * HALVING_INTERVAL
}

pub fn format_amount(gori: u64) -> String {
    format!("{}.{:08}", gori / COIN, gori % COIN)
}

pub fn parse_amount(s: &str) -> Option<u64> {
    let s = s.trim();
    let (int, frac) = s.split_once('.').unwrap_or((s, ""));
    if frac.len() > 8 || int.is_empty() && frac.is_empty() {
        return None;
    }
    let int: u64 = if int.is_empty() { 0 } else { int.parse().ok()? };
    let mut f = frac.to_string();
    while f.len() < 8 {
        f.push('0');
    }
    let frac: u64 = f.parse().ok()?;
    int.checked_mul(COIN)?.checked_add(frac)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn schedule() {
        assert_eq!(subsidy(0), 0);
        assert_eq!(subsidy(1), 5 * COIN);
        assert_eq!(subsidy(HALVING_INTERVAL), 5 * COIN);
        assert_eq!(subsidy(HALVING_INTERVAL + 1), 250_000_000);
        assert_eq!(subsidy(2 * HALVING_INTERVAL + 1), 125_000_000);
        assert_eq!(subsidy(last_subsidy_height() + 1), 0);
        assert_eq!(issued_through(HALVING_INTERVAL), 10_500_000 * COIN);
    }
    #[test]
    fn total_supply_below_cap() {
        let total = issued_through(u64::MAX / 2);
        assert!(total <= MAX_SUPPLY);
        // Integer halving leaves less than one coin of dust below 21 million.
        assert!(MAX_SUPPLY - total < COIN, "dust {}", MAX_SUPPLY - total);
        assert_eq!(total, issued_through(last_subsidy_height()));
        println!("final supply {}", format_amount(total));
    }
    #[test]
    fn closed_form_matches_sum() {
        let mut s = 0u64;
        for h in 0..=5000 {
            s += subsidy(h);
            assert_eq!(s, issued_through(h));
        }
    }
    #[test]
    fn amounts() {
        assert_eq!(parse_amount("1.5"), Some(150_000_000));
        assert_eq!(parse_amount("0.00000001"), Some(1));
        assert_eq!(parse_amount("1.000000001"), None);
        assert_eq!(format_amount(150_000_001), "1.50000001");
    }
}
