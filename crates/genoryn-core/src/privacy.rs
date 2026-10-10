//! Vetted Orchard 0.16 / Ironwood v3 integration. No locally authored circuit.
use crate::{
    codec::{self, Hash, MAX_ACTIONS, MAX_TX_BYTES},
    params::Network,
};
use anyhow::{Context, Result, ensure};
use orchard::{
    Action, Address, Anchor, Bundle, Note, Proof,
    builder::{Builder, BundleType},
    bundle::{Authorization, Authorized, BundleVersion, Flags, TxVersion},
    circuit::{OrchardCircuitVersion, ProvingKey, VerifyingKey},
    keys::{FullViewingKey, OutgoingViewingKey, Scope, SpendAuthorizingKey, SpendingKey},
    note::{ExtractedNoteCommitment, Nullifier, TransmittedNoteCiphertext},
    primitives::redpallas,
    value::{NoteValue, ValueCommitment},
};
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

pub const BUNDLE_VERSION: BundleVersion = BundleVersion::ironwood_v3();
pub const CIRCUIT_VERSION: OrchardCircuitVersion = OrchardCircuitVersion::PostNu6_3;
pub type AuthorizedBundle = Bundle<Authorized, i64>;
static PROVING_KEY: OnceLock<ProvingKey> = OnceLock::new();
static VERIFYING_KEY: OnceLock<VerifyingKey> = OnceLock::new();
pub fn proving_key() -> &'static ProvingKey {
    PROVING_KEY.get_or_init(|| ProvingKey::build(CIRCUIT_VERSION))
}
pub fn verifying_key() -> &'static VerifyingKey {
    VERIFYING_KEY.get_or_init(|| VerifyingKey::build(CIRCUIT_VERSION))
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct TxContext {
    pub protocol: u16,
    pub network: Network,
    /// 0 transfer, 1 miner issuance, 2 treasury issuance. No other classes exist.
    pub kind: u8,
    pub height: u64,
    pub expires: u64,
    /// Random for transfers; previous block hash for coinbase bundles.
    pub nonce: Hash,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ActionWire {
    pub nullifier: Hash,
    pub rk: Hash,
    pub cmx: Hash,
    pub ephemeral_key: Hash,
    pub encrypted_note: Vec<u8>,
    pub outgoing_ciphertext: Vec<u8>,
    pub value_commitment: Hash,
    pub signature: Vec<u8>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct BundleWire {
    pub actions: Vec<ActionWire>,
    pub flags: u8,
    /// Public fee for transfers, negative deterministic issuance for coinbase.
    pub value_balance: i64,
    pub anchor: Hash,
    pub proof: Vec<u8>,
    pub binding_signature: Vec<u8>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ShieldedTx {
    pub context: TxContext,
    pub bundle: BundleWire,
}

pub fn sighash<A: Authorization>(context: &TxContext, bundle: &Bundle<A, i64>) -> Result<Hash> {
    let mut bytes = codec::encode(context)?;
    bytes.extend_from_slice(&context.network.genesis());
    bytes.extend_from_slice(bundle.commitment(TxVersion::V6)?.0.as_bytes());
    bytes.extend_from_slice(&bundle.anchor().to_bytes());
    bytes.extend_from_slice(&bundle.value_balance().to_le_bytes());
    bytes.push(bundle.flag_byte());
    Ok(codec::hash(b"GENORYN/shielded/signature/v1", &bytes))
}

impl BundleWire {
    pub fn from_bundle(bundle: &AuthorizedBundle) -> Result<Self> {
        let actions = bundle
            .actions()
            .iter()
            .map(|a| ActionWire {
                nullifier: a.nullifier().to_bytes(),
                rk: a.rk().into(),
                cmx: a.cmx().to_bytes(),
                ephemeral_key: a.encrypted_note().epk_bytes,
                encrypted_note: a.encrypted_note().enc_ciphertext.as_ref().to_vec(),
                outgoing_ciphertext: a.encrypted_note().out_ciphertext.to_vec(),
                value_commitment: a.cv_net().to_bytes(),
                signature: <[u8; 64]>::from(a.authorization()).to_vec(),
            })
            .collect();
        Ok(Self {
            actions,
            flags: bundle.flag_byte(),
            value_balance: *bundle.value_balance(),
            anchor: bundle.anchor().to_bytes(),
            proof: bundle.authorization().proof().as_ref().to_vec(),
            binding_signature: <[u8; 64]>::from(bundle.authorization().binding_signature())
                .to_vec(),
        })
    }
    pub fn to_bundle(&self) -> Result<AuthorizedBundle> {
        ensure!(
            !self.actions.is_empty() && self.actions.len() <= MAX_ACTIONS,
            "action count out of bounds"
        );
        ensure!(
            self.proof.len() == Proof::expected_proof_size(self.actions.len()),
            "noncanonical proof length"
        );
        let actions = self
            .actions
            .iter()
            .map(|a| -> Result<_> {
                let nf = Option::<Nullifier>::from(Nullifier::from_bytes(&a.nullifier))
                    .context("invalid nullifier")?;
                let rk = redpallas::VerificationKey::try_from(a.rk)?;
                let cmx = Option::<ExtractedNoteCommitment>::from(
                    ExtractedNoteCommitment::from_bytes(&a.cmx),
                )
                .context("invalid commitment")?;
                let cv = Option::<ValueCommitment>::from(ValueCommitment::from_bytes(
                    &a.value_commitment,
                ))
                .context("invalid value commitment")?;
                let ciphertext = TransmittedNoteCiphertext {
                    epk_bytes: a.ephemeral_key,
                    enc_ciphertext: orchard::note_encryption::NoteBytesData(
                        a.encrypted_note
                            .as_slice()
                            .try_into()
                            .context("note ciphertext length")?,
                    ),
                    out_ciphertext: a
                        .outgoing_ciphertext
                        .as_slice()
                        .try_into()
                        .context("outgoing ciphertext length")?,
                };
                let sig = redpallas::Signature::from(
                    <[u8; 64]>::try_from(a.signature.as_slice()).context("signature length")?,
                );
                Ok(Action::from_parts(nf, rk, cmx, ciphertext, cv, sig)?)
            })
            .collect::<Result<Vec<_>>>()?;
        let flags = Flags::from_byte(self.flags, BUNDLE_VERSION).context("invalid flags")?;
        let anchor =
            Option::<Anchor>::from(Anchor::from_bytes(self.anchor)).context("invalid anchor")?;
        let binding = redpallas::Signature::from(
            <[u8; 64]>::try_from(self.binding_signature.as_slice())
                .context("binding signature length")?,
        );
        Ok(Bundle::try_from_parts(
            nonempty::NonEmpty::from_vec(actions).context("empty bundle")?,
            flags,
            self.value_balance,
            anchor,
            Authorized::from_parts(Proof::new(self.proof.clone()), binding),
            BUNDLE_VERSION,
        )?)
    }
}

impl ShieldedTx {
    pub fn id(&self) -> Result<Hash> {
        Ok(codec::hash(
            b"GENORYN/transaction/v1",
            &codec::encode(self)?,
        ))
    }
    pub fn verify_crypto(&self) -> Result<AuthorizedBundle> {
        ensure!(
            codec::encode(self)?.len() <= MAX_TX_BYTES,
            "transaction too large"
        );
        ensure!(
            self.context.protocol == 1 && self.context.kind <= 2,
            "unsupported transaction version/type"
        );
        let bundle = self.bundle.to_bundle()?;
        let digest = sighash(&self.context, &bundle)?;
        for action in bundle.actions() {
            action.rk().verify(&digest, action.authorization())?;
        }
        bundle
            .binding_validating_key()
            .verify(&digest, bundle.authorization().binding_signature())?;
        bundle
            .verify_proof(verifying_key())
            .context("shielded proof invalid")?;
        Ok(bundle)
    }
}

pub fn build_bundle(
    context: TxContext,
    builder: Builder,
    keys: &[SpendAuthorizingKey],
) -> Result<ShieldedTx> {
    let mut rng = rand::rng();
    let (bundle, _) = builder
        .build::<i64>(&mut rng)?
        .context("empty transaction")?;
    let digest = sighash(&context, &bundle)?;
    let bundle = bundle
        .create_proof(proving_key(), &mut rng)?
        .apply_signatures(&mut rng, digest, keys)?;
    Ok(ShieldedTx {
        context,
        bundle: BundleWire::from_bundle(&bundle)?,
    })
}

/// Public, reproducible test key for devnet and testnet treasury experiments. Mainnet
/// never uses it: its treasury pays the provably unspendable burn address.
pub fn treasury_key(network: Network) -> SpendingKey {
    let mut counter = 0u64;
    loop {
        let seed = codec::hash(
            b"GENORYN/dev-only-treasury-key",
            &[
                network.id().to_le_bytes().as_slice(),
                &counter.to_le_bytes(),
            ]
            .concat(),
        );
        if let Some(key) = Option::<SpendingKey>::from(SpendingKey::from_bytes(seed)) {
            return key;
        }
        counter += 1;
    }
}
/// Mainnet burns the treasury allocation: nobody, the founder included, can ever spend
/// it. Test networks keep the public test key so treasury code paths stay exercisable.
pub fn treasury_address(network: Network) -> Address {
    match network {
        Network::Mainnet => crate::names::burn_address(network),
        _ => FullViewingKey::from(&treasury_key(network)).address_at(0u32, Scope::External),
    }
}
pub fn treasury_ovk() -> OutgoingViewingKey {
    OutgoingViewingKey::from([0u8; 32])
}

pub fn mint(
    context: TxContext,
    recipient: Address,
    amount: u64,
    treasury: bool,
) -> Result<ShieldedTx> {
    ensure!(amount <= i64::MAX as u64, "mint amount overflow");
    let mut builder = Builder::new(
        BundleType::Coinbase,
        BUNDLE_VERSION,
        Flags::SPENDS_DISABLED,
        Anchor::empty_tree(),
    )?;
    builder.add_output(
        if treasury { Some(treasury_ovk()) } else { None },
        recipient,
        NoteValue::from_raw(amount),
        [0; 512],
    )?;
    build_bundle(context, builder, &[])
}

pub fn treasury_note(tx: &ShieldedTx) -> Result<Note> {
    let bundle = tx.bundle.to_bundle()?;
    ensure!(
        bundle.actions().len() == 1,
        "treasury must contain exactly one action"
    );
    let (note, recipient, memo) = bundle
        .recover_output_with_ovk(0, &treasury_ovk())
        .context("treasury output is not publicly auditable")?;
    ensure!(
        recipient == treasury_address(tx.context.network),
        "incorrect treasury receiver"
    );
    ensure!(memo == [0; 512], "treasury memo must be empty");
    Ok(note)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn malformed_bundle_fails_closed() {
        let malformed = BundleWire {
            actions: vec![],
            flags: 255,
            value_balance: -1,
            anchor: [0; 32],
            proof: vec![],
            binding_signature: vec![],
        };
        assert!(malformed.to_bundle().is_err());
    }
    #[test]
    fn mainnet_treasury_is_burned() {
        let treasury = treasury_address(Network::Mainnet);
        assert!(crate::names::is_burn_address(Network::Mainnet, &treasury));
        let public_test_key =
            FullViewingKey::from(&treasury_key(Network::Mainnet)).address_at(0u32, Scope::External);
        assert_ne!(treasury, public_test_key);
        assert!(!crate::names::is_burn_address(
            Network::Devnet,
            &treasury_address(Network::Devnet)
        ));
    }
    #[test]
    fn patched_circuit_is_pinned() {
        assert_eq!(
            BUNDLE_VERSION.circuit_version(),
            OrchardCircuitVersion::PostNu6_3
        );
    }
    #[test]
    #[ignore = "real Halo2 proof integration; run scripts/test-all"]
    fn genuine_mint_proof_and_tamper_rejection() {
        let context = TxContext {
            protocol: 1,
            network: Network::Devnet,
            kind: 2,
            height: 1,
            expires: 0,
            nonce: Network::Devnet.genesis(),
        };
        let tx = mint(context, treasury_address(Network::Devnet), 42, true).unwrap();
        tx.verify_crypto().unwrap();
        assert_eq!(treasury_note(&tx).unwrap().value().inner(), 42);
        let mut changed = tx.clone();
        changed.bundle.value_balance -= 1;
        assert!(changed.verify_crypto().is_err());
        let mut wrong_network = tx.clone();
        wrong_network.context.network = Network::Testnet;
        assert!(wrong_network.verify_crypto().is_err());
        let mut invalid_proof = tx.clone();
        invalid_proof.bundle.proof[0] ^= 1;
        assert!(invalid_proof.verify_crypto().is_err());
    }
}
