//! Bootstrap hosts, resolved like operator DNS seeds on the network's base port:
//! a built-in list plus the list published in the main repository, so seed
//! operators can be added without a new release. Seeds only help a new node find
//! its first peers; no seed is a consensus authority and every block received from
//! one is fully validated.
use genoryn_core::params::Network;
use std::time::Duration;

const MAINNET: &[&str] = &["187.7.72.137"];
const DEVNET: &[&str] = &[];
const TESTNET: &[&str] = &[];
/// v3 onion seeds, reached onion-to-onion without Tor exits. Kept apart from
/// the lists above because nodes before 0.6 read those and would hand an onion
/// name to the system resolver.
const MAINNET_ONION: &[&str] = &[];
const PUBLISHED: &str = "https://raw.githubusercontent.com/genoryn/genoryn/main/seeds";
const MAX_PUBLISHED_BYTES: usize = 16 * 1024;
const MAX_PUBLISHED_SEEDS: usize = 16;
/// Through Tor a request right after bootstrap also waits for a circuit, which
/// often takes longer than a whole direct fetch.
const DIRECT_FETCH: (Duration, usize) = (Duration::from_secs(5), 1);
const TOR_FETCH: (Duration, usize) = (Duration::from_secs(30), 2);

pub fn published_url(network: Network) -> Option<String> {
    // Named per 1.0 network: nodes of the retired networks read `mainnet.txt`.
    Some(format!("{PUBLISHED}/{}.txt", network.data_dir_name()))
}

/// The published onion seed list, read only by hybrid and tor-only nodes.
pub fn published_onion_url(network: Network) -> Option<String> {
    published_url(network).map(|url| url.replace(".txt", "-onion.txt"))
}

/// One seed name per line; blank lines, `#` comments and invalid names are skipped.
pub fn parse_published(text: &str) -> Vec<String> {
    parse_list(text, valid_seed_name)
}
/// Like `parse_published`, keeping only valid v3 onion names.
pub fn parse_published_onion(text: &str) -> Vec<String> {
    parse_list(text, |name| crate::net::OnionV3::parse(name).is_ok())
}
fn parse_list(text: &str, accept: impl Fn(&str) -> bool) -> Vec<String> {
    let mut seeds = Vec::new();
    for line in text.lines() {
        let name = line.split('#').next().unwrap_or_default().trim();
        if valid_seed_name(name) && accept(name) && !seeds.iter().any(|seed| seed == name) {
            seeds.push(name.to_owned());
        }
        if seeds.len() == MAX_PUBLISHED_SEEDS {
            break;
        }
    }
    seeds
}

/// Best effort: an unreachable or malformed list leaves the other discovery sources.
/// With a `socks5h://` proxy the request goes only through Tor (remote DNS); a
/// Tor failure is reported, never retried directly.
pub async fn fetch_published(network: Network, tor_proxy: Option<&str>) -> Vec<String> {
    match published_url(network) {
        Some(url) => fetch_list(&url, tor_proxy, parse_published).await,
        None => Vec::new(),
    }
}
/// The published onion seeds, fetched the same way.
pub async fn fetch_published_onion(network: Network, tor_proxy: Option<&str>) -> Vec<String> {
    match published_onion_url(network) {
        Some(url) => fetch_list(&url, tor_proxy, parse_published_onion).await,
        None => Vec::new(),
    }
}
async fn fetch_list(
    url: &str,
    tor_proxy: Option<&str>,
    parse: fn(&str) -> Vec<String>,
) -> Vec<String> {
    let (timeout, attempts) = if tor_proxy.is_some() {
        TOR_FETCH
    } else {
        DIRECT_FETCH
    };
    let fetch = || async {
        let client = crate::net::http_client_builder(tor_proxy)?
            .user_agent(concat!("genorynd/", env!("CARGO_PKG_VERSION")))
            .timeout(timeout)
            .redirect(reqwest::redirect::Policy::limited(2))
            .https_only(true)
            .build()?;
        let response = client.get(url).send().await?.error_for_status()?;
        anyhow::ensure!(
            response
                .content_length()
                .is_none_or(|length| length <= MAX_PUBLISHED_BYTES as u64),
            "published seed list is too large"
        );
        let body = response.bytes().await?;
        anyhow::ensure!(
            body.len() <= MAX_PUBLISHED_BYTES,
            "published seed list is too large"
        );
        Ok::<_, anyhow::Error>(parse(&String::from_utf8_lossy(&body)))
    };
    for attempt in 1..=attempts {
        match fetch().await {
            Ok(seeds) => return seeds,
            Err(error) if attempt == attempts => {
                tracing::warn!(%error, "published seed list unavailable; other discovery continues")
            }
            Err(error) => tracing::debug!(%error, "published seed list fetch failed; retrying"),
        }
    }
    Vec::new()
}

pub fn default_seeds(network: Network) -> &'static [&'static str] {
    match network {
        Network::Devnet => DEVNET,
        Network::Testnet => TESTNET,
        Network::Mainnet => MAINNET,
    }
}
pub fn default_onion_seeds(network: Network) -> &'static [&'static str] {
    match network {
        Network::Mainnet => MAINNET_ONION,
        Network::Devnet | Network::Testnet => &[],
    }
}

/// DNS names, onion names or IP literals only; no ports, schemes or paths.
pub fn valid_seed_name(seed: &str) -> bool {
    !seed.is_empty()
        && seed.len() <= 253
        && seed
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_in_seeds_are_valid_names() {
        for network in [Network::Devnet, Network::Testnet, Network::Mainnet] {
            for seed in default_seeds(network) {
                assert!(valid_seed_name(seed), "{seed}");
            }
        }
        assert!(
            !default_seeds(Network::Mainnet).is_empty(),
            "mainnet has a bootstrap seed"
        );
    }

    #[test]
    fn published_lists_are_bounded_and_ignore_noise() {
        let text = "# GENORYN devnet seeds\n\n203.0.113.7\nseed.example.org # operator A\nbad:host\n203.0.113.7\n";
        assert_eq!(
            parse_published(text),
            vec!["203.0.113.7".to_owned(), "seed.example.org".to_owned()]
        );
        let many = (0..40)
            .map(|n| format!("seed{n}.example.org\n"))
            .collect::<String>();
        assert_eq!(parse_published(&many).len(), MAX_PUBLISHED_SEEDS);
        assert!(
            published_url(Network::Mainnet)
                .unwrap()
                .ends_with("/seeds/mainnet-1.0.txt")
        );
        assert!(
            published_url(Network::Devnet)
                .unwrap()
                .starts_with("https://")
        );
    }

    #[test]
    fn onion_seeds_are_published_apart_and_must_be_valid_v3_names() {
        let onion = crate::net::addr::tests::test_onion(3).hostname();
        let text = format!(
            "# onion seeds\n{onion} # operator A\n203.0.113.7\nseed.example.org\nabc.onion\n{onion}\n"
        );
        assert_eq!(parse_published_onion(&text), vec![onion.clone()]);
        assert!(
            parse_published(&format!("{onion}\n")).contains(&onion),
            "the regular list format would accept it, hence the separate file"
        );
        assert!(
            published_onion_url(Network::Mainnet)
                .unwrap()
                .ends_with("/seeds/mainnet-1.0-onion.txt")
        );
        for network in [Network::Devnet, Network::Testnet, Network::Mainnet] {
            for seed in default_onion_seeds(network) {
                assert_eq!(parse_published_onion(seed), vec![seed.to_string()]);
            }
        }
    }

    #[test]
    fn seed_names_reject_ports_and_urls() {
        assert!(valid_seed_name("seed.example.org"));
        assert!(valid_seed_name("203.0.113.7"));
        for bad in ["", "seed.example.org:29410", "http://seed", "a/b", "a b"] {
            assert!(!valid_seed_name(bad), "{bad}");
        }
    }
}
