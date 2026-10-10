//! User-entered display values. Never read by transaction construction or consensus.
use anyhow::{Context, Result, ensure};

pub(super) fn format_usd(balance_atomic: &str, price: &str) -> Result<String> {
    let rate = super::protocol::parse_amount(price)?;
    ensure!(
        rate > 0 && rate <= 1_000_000_000_000_000_000,
        "Reference price must be positive and at most 1,000,000 USD/GNR."
    );
    let balance: u128 = balance_atomic.parse().context("Invalid local balance")?;
    let cents = balance
        .checked_mul(u128::from(rate))
        .context("Reference display overflow")?
        / 10_000_000_000_000_000_000_000u128;
    Ok(format!("${}.{:02} USD", cents / 100, cents % 100))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reference_display_is_exact_checked_and_rounds_down_to_cents() {
        assert_eq!(format_usd("1000000000000", "1").unwrap(), "$1.00 USD");
        assert_eq!(format_usd("1500000000000", "2.50").unwrap(), "$3.75 USD");
        assert_eq!(format_usd("1", "1").unwrap(), "$0.00 USD");
        assert_eq!(
            format_usd("21000000000000000000", "1000000").unwrap(),
            "$21000000000000.00 USD"
        );
        for price in ["", "0", "-1", "1e5", "1000001"] {
            assert!(format_usd("1000000000000", price).is_err());
        }
        assert!(format_usd(&u128::MAX.to_string(), "1000000").is_err());
    }
}
