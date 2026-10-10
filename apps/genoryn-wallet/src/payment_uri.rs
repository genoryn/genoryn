//! Experimental v1 payment requests are parsed locally and never auto-submitted.
use crate::protocol::{format_atomic, parse_amount};
use anyhow::{Context, Result, ensure};
use genoryn_core::params::{MAX_MONEY_PER_TX, Network};
use genoryn_wallet_core::keys::decode_address;
use std::collections::BTreeMap;

#[derive(Debug, PartialEq, Eq)]
pub struct PaymentRequest {
    pub address: String,
    pub amount_atomic: Option<u64>,
}

pub fn parse(text: &str, expected_network: Network) -> Result<PaymentRequest> {
    ensure!(
        text.len() <= 2048
            && text.is_ascii()
            && !text
                .bytes()
                .any(|b| b.is_ascii_whitespace() || b == b'#' || b == b'%'),
        "malformed payment URI"
    );
    let body = text
        .strip_prefix("genoryn:")
        .context("expected genoryn: scheme")?;
    let (address, query) = body
        .split_once('?')
        .context("payment URI version and network are required")?;
    ensure!(
        !address.is_empty() && !address.contains('/') && !address.contains('@'),
        "payment URI requires a raw receiving identity"
    );
    decode_address(expected_network, address)
        .context("payment URI receiving identity is invalid or belongs to another network")?;
    let mut params = BTreeMap::new();
    for item in query.split('&') {
        let (key, value) = item
            .split_once('=')
            .context("malformed payment URI parameter")?;
        ensure!(
            ["v", "network", "amount"].contains(&key) && !value.is_empty(),
            "unsupported or empty payment URI parameter"
        );
        ensure!(
            params.insert(key, value).is_none(),
            "duplicate payment URI parameter"
        );
    }
    ensure!(
        params.get("v") == Some(&"1"),
        "unsupported payment URI version"
    );
    ensure!(
        params.get("network") == Some(&expected_network.name()),
        "payment URI belongs to another network"
    );
    let amount_atomic = params
        .get("amount")
        .map(|value| -> Result<u64> {
            let amount = parse_amount(value)?;
            ensure!(
                amount > 0 && amount <= MAX_MONEY_PER_TX,
                "payment URI amount is outside the supported transaction range"
            );
            ensure!(
                format_atomic(&amount.to_string()) == **value,
                "payment URI amount must use canonical decimal notation"
            );
            Ok(amount)
        })
        .transpose()?;
    Ok(PaymentRequest {
        address: address.into(),
        amount_atomic,
    })
}
pub fn build(network: Network, address: &str, amount: Option<&str>) -> Result<String> {
    decode_address(network, address)?;
    let mut uri = format!("genoryn:{address}?v=1&network={}", network.name());
    if let Some(value) = amount {
        let atomic = parse_amount(value)?;
        ensure!(
            atomic > 0 && atomic <= MAX_MONEY_PER_TX,
            "request amount outside transaction range"
        );
        uri.push_str(&format!("&amount={}", format_atomic(&atomic.to_string())));
    }
    parse(&uri, network)?;
    Ok(uri)
}
#[cfg(test)]
mod tests {
    use super::*;
    fn address(network: Network) -> String {
        genoryn_wallet_core::keys::encode_address(
            network,
            genoryn_core::privacy::treasury_address(network),
        )
        .unwrap()
    }
    #[test]
    fn versioned_uri_roundtrips_exact_amount_and_network() {
        let address = address(Network::Devnet);
        let uri = build(Network::Devnet, &address, Some("1.000000000001")).unwrap();
        assert_eq!(
            parse(&uri, Network::Devnet).unwrap(),
            PaymentRequest {
                address: address.clone(),
                amount_atomic: Some(1_000_000_000_001)
            }
        );
        assert_eq!(
            parse(
                &build(Network::Devnet, &address, None).unwrap(),
                Network::Devnet
            )
            .unwrap()
            .amount_atomic,
            None
        );
        assert!(parse(&uri, Network::Testnet).is_err());
    }
    #[test]
    fn rejects_ambiguous_or_malicious_requests() {
        let address = address(Network::Devnet);
        let prefix = format!("genoryn:{address}");
        for query in [
            "v=2&network=devnet",
            "v=1&network=testnet",
            "v=1&network=devnet&amount=0",
            "v=1&network=devnet&amount=01",
            "v=1&network=devnet&amount=1.0",
            "v=1&network=devnet&amount=1e2",
            "v=1&network=devnet&amount=1&amount=2",
            "v=1&network=devnet&amount=1%26amount=2",
            "v=1&network=devnet&amount=-1",
            "v=1&network=devnet&memo=private",
            "v=1&network=devnet#fragment",
            "network=devnet",
            "v=1&network=devnet&v=1",
        ] {
            assert!(
                parse(&format!("{prefix}?{query}"), Network::Devnet).is_err(),
                "{query}"
            );
        }
        assert!(parse("genoryn://evil/?v=1&network=devnet", Network::Devnet).is_err());
        assert!(
            parse(
                &format!(
                    "genoryn:{}{}?v=1&network=devnet",
                    &address[..address.len() - 1],
                    if address.ends_with('q') { 'p' } else { 'q' }
                ),
                Network::Devnet
            )
            .is_err()
        );
    }
}
