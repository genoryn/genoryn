//! Transaction-origin privacy within the existing peer protocol.
//!
//! Relayed transactions go to each outbound peer after an independent exponential
//! delay; all inbound peers share one per-transaction delay, so an observer that
//! opens many inbound connections gains nothing over opening one. A transaction
//! created by this node is first sent to a single relay peer (preferring outbound
//! Tor links) and is broadcast more widely only if it is not seen coming back from
//! the network within a random embargo. This is a one-hop "stem" in the spirit of
//! Dandelion++, chosen because it needs no new message type: unupgraded peers
//! simply relay it. It reduces, but does not eliminate, origin inference; the
//! chosen relay peer, timing analysis and network observers remain risks.
//!
//! Delivery never depends on the stem peer. When the transaction comes back
//! through any other peer it is relayed onward once, as a node that did not
//! create it would, so a dropping stem plus an echoing accomplice cannot
//! suppress it; otherwise it is broadcast once when the embargo ends. Peers
//! that connect later receive it with the rest of the mempool.
use crate::net::{Link, NetworkMode};
use genoryn_core::codec::Hash;
use rand08::{Rng, RngCore};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    sync::Mutex,
    time::{Duration, Instant},
};

pub const OUTBOUND_MEAN: Duration = Duration::from_secs(2);
pub const INBOUND_MEAN: Duration = Duration::from_secs(5);
pub const STEM_MEAN: Duration = Duration::from_secs(2);
pub const EMBARGO_SECONDS: std::ops::RangeInclusive<u64> = 20..=45;
pub const MAX_LOCAL: usize = 64;
/// Pending relays queued per connection; beyond this a relay is dropped for
/// that peer, which other peers still deliver.
pub const MAX_QUEUED_PER_PEER: usize = 256;

/// Exponential delay with the given mean from a uniform sample in (0, 1],
/// capped at six means so a queue entry never waits unboundedly.
pub fn exponential(mean: Duration, uniform: f64) -> Duration {
    let uniform = uniform.clamp(f64::MIN_POSITIVE, 1.0);
    let seconds = (-uniform.ln() * mean.as_secs_f64()).min(6.0 * mean.as_secs_f64());
    Duration::from_secs_f64(seconds.max(0.0))
}
fn uniform_from(bytes: [u8; 8]) -> f64 {
    ((u64::from_le_bytes(bytes) >> 11) as f64 + 1.0) / (1u64 << 53) as f64
}
pub fn random_delay(mean: Duration) -> Duration {
    let mut bytes = [0u8; 8];
    rand08::rngs::OsRng.fill_bytes(&mut bytes);
    exponential(mean, uniform_from(bytes))
}

/// Which links a broadcast may use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RelayScope {
    All,
    /// Only Tor-routed links (outbound through Tor or inbound via the onion service).
    TorLinks,
}
impl RelayScope {
    pub fn allows(self, link: Link) -> bool {
        self == Self::All || link.via_tor()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub connection: u64,
    pub inbound: bool,
    pub link: Link,
}

/// The single peer that first receives a transaction created here, or `None`
/// when no peer is acceptable yet. Outbound peers are preferred because this
/// node chose them; inbound slots can be filled by an observer.
pub fn choose_stem(mode: NetworkMode, peers: &[Candidate], rng: &mut impl Rng) -> Option<u64> {
    type Tier = fn(&Candidate) -> bool;
    fn outbound_tor(c: &Candidate) -> bool {
        !c.inbound && c.link.via_tor()
    }
    fn any_tor(c: &Candidate) -> bool {
        c.link.via_tor()
    }
    fn outbound(c: &Candidate) -> bool {
        !c.inbound
    }
    fn any(_: &Candidate) -> bool {
        true
    }
    const TOR_ONLY: &[Tier] = &[outbound_tor, any_tor];
    const HYBRID: &[Tier] = &[outbound_tor, any_tor, outbound, any];
    const DIRECT: &[Tier] = &[outbound, any];
    let tiers = match mode {
        NetworkMode::TorOnly => TOR_ONLY,
        NetworkMode::Hybrid => HYBRID,
        NetworkMode::Direct => DIRECT,
    };
    for tier in tiers {
        let eligible: Vec<_> = peers.iter().filter(|c| tier(c)).collect();
        if !eligible.is_empty() {
            return Some(eligible[rng.gen_range(0..eligible.len())].connection);
        }
    }
    None
}
/// Scope of an embargo-expiry broadcast of a transaction created here. Tor-only
/// never uses another link; hybrid prefers Tor links while any exist.
pub fn fluff_scope(mode: NetworkMode, peers: &[Candidate]) -> RelayScope {
    match mode {
        NetworkMode::TorOnly => RelayScope::TorLinks,
        NetworkMode::Hybrid if peers.iter().any(|c| c.link.via_tor()) => RelayScope::TorLinks,
        _ => RelayScope::All,
    }
}

#[derive(Debug)]
struct LocalTx {
    stem: Option<u64>,
    /// Sent beyond the stem: broadcast after the embargo, or relayed onward
    /// after it came back through another peer.
    forwarded: bool,
}

pub struct RelayState {
    salt: [u8; 32],
    local: Mutex<BTreeMap<Hash, LocalTx>>,
}
impl Default for RelayState {
    fn default() -> Self {
        let mut salt = [0u8; 32];
        rand08::rngs::OsRng.fill_bytes(&mut salt);
        Self {
            salt,
            local: Mutex::new(BTreeMap::new()),
        }
    }
}
impl RelayState {
    /// One delay per transaction for every inbound peer, unpredictable to peers
    /// because the salt is random per process.
    pub fn inbound_delay(&self, txid: &Hash) -> Duration {
        let digest = Sha256::new()
            .chain_update(self.salt)
            .chain_update(txid)
            .finalize();
        exponential(
            INBOUND_MEAN,
            uniform_from(digest[..8].try_into().expect("fixed slice")),
        )
    }
    pub fn embargo(&self) -> Duration {
        Duration::from_secs(rand08::rngs::OsRng.gen_range(EMBARGO_SECONDS))
    }
    /// Tracks a transaction created here; false when too many are pending.
    pub fn register_local(&self, txid: Hash) -> bool {
        let Ok(mut local) = self.local.lock() else {
            return false;
        };
        if local.len() >= MAX_LOCAL && !local.contains_key(&txid) {
            return false;
        }
        local.entry(txid).or_insert(LocalTx {
            stem: None,
            forwarded: false,
        });
        true
    }
    pub fn set_stem(&self, txid: &Hash, connection: u64) {
        if let Ok(mut local) = self.local.lock()
            && let Some(entry) = local.get_mut(txid)
        {
            entry.stem = Some(connection);
        }
    }
    /// A peer sent us a transaction we created. Unless it is the stem peer
    /// echoing it back, the network has it, and it is relayed onward once like
    /// any other transaction. Returns whether to relay it now.
    pub fn note_received(&self, txid: &Hash, from: u64) -> bool {
        let Ok(mut local) = self.local.lock() else {
            return false;
        };
        match local.get_mut(txid) {
            Some(entry) if entry.stem != Some(from) && !entry.forwarded => {
                entry.forwarded = true;
                true
            }
            _ => false,
        }
    }
    /// Claims the embargo broadcast; false if the transaction already went
    /// beyond its stem or is not ours.
    pub fn mark_forwarded(&self, txid: &Hash) -> bool {
        self.local
            .lock()
            .ok()
            .and_then(|mut local| {
                local
                    .get_mut(txid)
                    .map(|entry| !std::mem::replace(&mut entry.forwarded, true))
            })
            .unwrap_or(false)
    }
    /// Ours and not yet beyond its stem peer: withheld from every other peer.
    pub fn in_stem_phase(&self, txid: &Hash) -> bool {
        self.local
            .lock()
            .is_ok_and(|local| local.get(txid).is_some_and(|entry| !entry.forwarded))
    }
    pub fn is_local(&self, txid: &Hash) -> bool {
        self.local
            .lock()
            .is_ok_and(|local| local.contains_key(txid))
    }
    pub fn local_ids(&self) -> Vec<Hash> {
        self.local
            .lock()
            .map(|local| local.keys().copied().collect())
            .unwrap_or_default()
    }
    pub fn forget(&self, txid: &Hash) {
        if let Ok(mut local) = self.local.lock() {
            local.remove(txid);
        }
    }
    pub fn pending(&self) -> usize {
        self.local.lock().map(|local| local.len()).unwrap_or(0)
    }
}

/// Per-connection queue of relays waiting for their delay to elapse.
pub struct RelayQueue<T> {
    entries: BTreeMap<(Instant, u64), (Hash, T)>,
    sequence: u64,
}
impl<T> Default for RelayQueue<T> {
    fn default() -> Self {
        Self {
            entries: BTreeMap::new(),
            sequence: 0,
        }
    }
}
impl<T> RelayQueue<T> {
    pub fn push(&mut self, due: Instant, id: Hash, item: T) -> bool {
        if self.entries.len() >= MAX_QUEUED_PER_PEER {
            return false;
        }
        self.sequence = self.sequence.wrapping_add(1);
        self.entries.insert((due, self.sequence), (id, item));
        true
    }
    pub fn next_due(&self) -> Option<Instant> {
        self.entries.keys().next().map(|(due, _)| *due)
    }
    pub fn pop_due(&mut self, now: Instant) -> Vec<(Hash, T)> {
        let later = self.entries.split_off(&(now, u64::MAX));
        std::mem::replace(&mut self.entries, later)
            .into_values()
            .collect()
    }
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand08::{SeedableRng, rngs::StdRng};

    fn candidate(connection: u64, inbound: bool, link: Link) -> Candidate {
        Candidate {
            connection,
            inbound,
            link,
        }
    }

    #[test]
    fn delays_are_exponential_bounded_and_not_constant() {
        assert_eq!(exponential(OUTBOUND_MEAN, 1.0), Duration::ZERO);
        assert_eq!(exponential(OUTBOUND_MEAN, 0.0), OUTBOUND_MEAN * 6);
        let samples: Vec<_> = (0..2_000).map(|_| random_delay(OUTBOUND_MEAN)).collect();
        let mean = samples.iter().map(Duration::as_secs_f64).sum::<f64>() / samples.len() as f64;
        assert!((1.6..2.4).contains(&mean), "mean {mean}");
        assert!(samples.iter().all(|d| *d <= OUTBOUND_MEAN * 6));
        let distinct: std::collections::BTreeSet<_> =
            samples.iter().map(Duration::as_millis).collect();
        assert!(distinct.len() > 500, "no fixed relay fingerprint");
    }

    #[test]
    fn inbound_delay_is_shared_per_transaction_and_salted_per_process() {
        let state = RelayState::default();
        let other = RelayState::default();
        let ids: Vec<Hash> = (0..64u8).map(|n| [n; 32]).collect();
        for id in &ids {
            assert_eq!(state.inbound_delay(id), state.inbound_delay(id));
        }
        assert!(
            ids.iter()
                .any(|id| state.inbound_delay(id) != other.inbound_delay(id))
        );
    }

    #[test]
    fn stem_prefers_outbound_tor_and_never_leaves_tor_in_tor_only_mode() {
        let mut rng = StdRng::seed_from_u64(1);
        let peers = [
            candidate(1, false, Link::Clearnet),
            candidate(2, true, Link::OnionInbound),
            candidate(3, false, Link::Tor),
            candidate(4, true, Link::Clearnet),
        ];
        for _ in 0..50 {
            assert_eq!(choose_stem(NetworkMode::TorOnly, &peers, &mut rng), Some(3));
            assert_eq!(choose_stem(NetworkMode::Hybrid, &peers, &mut rng), Some(3));
            assert!(matches!(
                choose_stem(NetworkMode::Direct, &peers, &mut rng),
                Some(1 | 3)
            ));
        }
        let clearnet_only = [
            candidate(1, false, Link::Clearnet),
            candidate(4, true, Link::Clearnet),
        ];
        assert_eq!(
            choose_stem(NetworkMode::TorOnly, &clearnet_only, &mut rng),
            None
        );
        assert_eq!(
            choose_stem(NetworkMode::Hybrid, &clearnet_only, &mut rng),
            Some(1)
        );
        assert_eq!(choose_stem(NetworkMode::Direct, &[], &mut rng), None);
        assert_eq!(
            fluff_scope(NetworkMode::TorOnly, &clearnet_only),
            RelayScope::TorLinks
        );
        assert_eq!(
            fluff_scope(NetworkMode::Hybrid, &clearnet_only),
            RelayScope::All
        );
        assert_eq!(
            fluff_scope(NetworkMode::Hybrid, &peers),
            RelayScope::TorLinks
        );
        assert!(!RelayScope::TorLinks.allows(Link::Clearnet));
        assert!(RelayScope::TorLinks.allows(Link::OnionInbound));
    }

    #[test]
    fn echoes_from_other_peers_are_relayed_once_and_end_the_stem_phase() {
        let state = RelayState::default();
        let id = [7; 32];
        assert!(state.register_local(id));
        assert!(state.is_local(&id) && state.in_stem_phase(&id));
        state.set_stem(&id, 10);
        assert!(
            !state.note_received(&id, 10),
            "the stem echoing back proves nothing"
        );
        assert!(state.in_stem_phase(&id));
        assert!(state.note_received(&id, 11), "relay onward like any node");
        assert!(!state.in_stem_phase(&id));
        assert!(!state.note_received(&id, 12), "relayed only once");
        assert!(!state.mark_forwarded(&id), "no second embargo broadcast");
        state.forget(&id);
        assert!(!state.note_received(&id, 11), "forgotten: not ours");

        let quiet = [9; 32];
        assert!(state.register_local(quiet));
        state.set_stem(&quiet, 4);
        assert!(state.mark_forwarded(&quiet), "embargo broadcast once");
        assert!(!state.mark_forwarded(&quiet));
        assert!(!state.note_received(&quiet, 5), "already broadcast");
        assert!(!state.mark_forwarded(&[1; 32]), "not ours");
        assert_eq!(state.local_ids(), vec![quiet]);
        state.forget(&quiet);

        let unstemmed = [8; 32];
        assert!(state.register_local(unstemmed));
        assert!(
            state.note_received(&unstemmed, 3),
            "without a stem any receipt counts"
        );
        state.forget(&unstemmed);
        assert_eq!(state.pending(), 0);
        for n in 0..MAX_LOCAL as u8 {
            assert!(state.register_local([n; 32]));
        }
        assert!(!state.register_local([255; 32]), "bounded");
    }

    #[test]
    fn relay_queue_releases_in_due_order_and_is_bounded() {
        let start = Instant::now();
        let mut queue = RelayQueue::default();
        assert!(queue.push(start + Duration::from_secs(2), [2; 32], "b"));
        assert!(queue.push(start + Duration::from_secs(1), [1; 32], "a"));
        assert!(queue.push(start + Duration::from_secs(1), [3; 32], "c"));
        assert_eq!(queue.next_due(), Some(start + Duration::from_secs(1)));
        let due = queue.pop_due(start + Duration::from_secs(1));
        assert_eq!(
            due.iter().map(|(_, item)| *item).collect::<Vec<_>>(),
            ["a", "c"]
        );
        assert_eq!(queue.len(), 1);
        for n in 0..MAX_QUEUED_PER_PEER {
            queue.push(start, [n as u8; 32], "x");
        }
        assert_eq!(queue.len(), MAX_QUEUED_PER_PEER);
    }
}
