//! Post-quantum hybrid handshake and encrypted framing.
//!
//! initiator -> responder: magic | version | X25519 ephemeral | ML-KEM-768 encapsulation key
//! responder -> initiator: magic | version | X25519 ephemeral | ML-KEM-768 ciphertext
//!
//! Both sides derive directional ChaCha20-Poly1305 keys from SHA3 over both shared
//! secrets and the full transcript, so traffic recorded today stays confidential
//! even against a future quantum computer (and against a break of either primitive).

use anyhow::{Result, bail, ensure};
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Nonce};
use fips203::ml_kem_768;
use fips203::traits::{Decaps, Encaps, KeyGen, SerDes};
use rand_core::{OsRng, RngCore};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use zeroize::Zeroizing;

pub const PROTOCOL_VERSION: u16 = 1;
pub const MAX_FRAME: usize = 8 * 1024 * 1024;

pub struct SendHalf {
    w: OwnedWriteHalf,
    cipher: ChaCha20Poly1305,
    counter: u64,
}

pub struct RecvHalf {
    r: OwnedReadHalf,
    cipher: ChaCha20Poly1305,
    counter: u64,
}

fn nonce(counter: u64) -> Nonce {
    let mut n = [0u8; 12];
    n[4..].copy_from_slice(&counter.to_le_bytes());
    Nonce::from(n)
}

impl SendHalf {
    pub async fn send(&mut self, payload: &[u8]) -> Result<()> {
        ensure!(payload.len() <= MAX_FRAME, "frame too large");
        let ct = self
            .cipher
            .encrypt(&nonce(self.counter), payload)
            .map_err(|_| anyhow::anyhow!("encrypt"))?;
        self.counter += 1;
        self.w.write_all(&(ct.len() as u32).to_le_bytes()).await?;
        self.w.write_all(&ct).await?;
        Ok(())
    }
}

impl RecvHalf {
    pub async fn recv(&mut self) -> Result<Vec<u8>> {
        let mut len = [0u8; 4];
        self.r.read_exact(&mut len).await?;
        let n = u32::from_le_bytes(len) as usize;
        ensure!(n <= MAX_FRAME + 16, "frame too large");
        let mut buf = vec![0u8; n];
        self.r.read_exact(&mut buf).await?;
        let pt = self
            .cipher
            .decrypt(&nonce(self.counter), buf.as_slice())
            .map_err(|_| anyhow::anyhow!("frame authentication failed"))?;
        self.counter += 1;
        Ok(pt)
    }
}

fn derive(
    ss_m: &[u8],
    ss_x: &[u8],
    transcript: &[u8],
) -> (Zeroizing<[u8; 32]>, Zeroizing<[u8; 32]>) {
    let base = Zeroizing::new(krypton_crypto::hash::tagged_parts(
        "Krypton/p2p/X25519-MLKEM768/v1",
        &[ss_m, ss_x, transcript],
    ));
    let i2r = Zeroizing::new(krypton_crypto::hash::tagged_parts(
        "Krypton/p2p/i2r",
        &[base.as_ref()],
    ));
    let r2i = Zeroizing::new(krypton_crypto::hash::tagged_parts(
        "Krypton/p2p/r2i",
        &[base.as_ref()],
    ));
    (i2r, r2i)
}

async fn read_exact_vec(s: &mut TcpStream, n: usize) -> Result<Vec<u8>> {
    let mut v = vec![0u8; n];
    s.read_exact(&mut v).await?;
    Ok(v)
}

pub async fn initiate(mut s: TcpStream, magic: [u8; 4]) -> Result<(SendHalf, RecvHalf)> {
    let mut e = Zeroizing::new([0u8; 32]);
    OsRng.fill_bytes(e.as_mut());
    let eph = x25519_dalek::StaticSecret::from(*e);
    let eph_pub = x25519_dalek::PublicKey::from(&eph).to_bytes();
    let (ek, dk) = ml_kem_768::KG::try_keygen().map_err(|_| anyhow::anyhow!("keygen"))?;
    let ek_b = ek.into_bytes();
    let mut hello = Vec::with_capacity(6 + 32 + ek_b.len());
    hello.extend_from_slice(&magic);
    hello.extend_from_slice(&PROTOCOL_VERSION.to_le_bytes());
    hello.extend_from_slice(&eph_pub);
    hello.extend_from_slice(&ek_b);
    s.write_all(&hello).await?;
    let reply = read_exact_vec(&mut s, 6 + 32 + ml_kem_768::CT_LEN).await?;
    if reply[..4] != magic {
        bail!("peer is on another network");
    }
    let peer_x: [u8; 32] = reply[6..38].try_into().expect("32");
    let ct = ml_kem_768::CipherText::try_from_bytes(reply[38..].try_into().expect("ct"))
        .map_err(|_| anyhow::anyhow!("ct"))?;
    let ss_m = Zeroizing::new(
        dk.try_decaps(&ct)
            .map_err(|_| anyhow::anyhow!("decaps"))?
            .into_bytes(),
    );
    let ss_x = Zeroizing::new(
        eph.diffie_hellman(&x25519_dalek::PublicKey::from(peer_x))
            .to_bytes(),
    );
    let mut transcript = hello;
    transcript.extend_from_slice(&reply);
    let (i2r, r2i) = derive(ss_m.as_ref(), ss_x.as_ref(), &transcript);
    let (r, w) = s.into_split();
    Ok((
        SendHalf {
            w,
            cipher: ChaCha20Poly1305::new(i2r.as_ref().into()),
            counter: 0,
        },
        RecvHalf {
            r,
            cipher: ChaCha20Poly1305::new(r2i.as_ref().into()),
            counter: 0,
        },
    ))
}

pub async fn respond(mut s: TcpStream, magic: [u8; 4]) -> Result<(SendHalf, RecvHalf)> {
    let hello = read_exact_vec(&mut s, 6 + 32 + ml_kem_768::EK_LEN).await?;
    if hello[..4] != magic {
        bail!("peer is on another network");
    }
    let peer_x: [u8; 32] = hello[6..38].try_into().expect("32");
    let ek = ml_kem_768::EncapsKey::try_from_bytes(hello[38..].try_into().expect("ek"))
        .map_err(|_| anyhow::anyhow!("ek"))?;
    let (ss_m, ct) = ek.try_encaps().map_err(|_| anyhow::anyhow!("encaps"))?;
    let ss_m = Zeroizing::new(ss_m.into_bytes());
    let mut e = Zeroizing::new([0u8; 32]);
    OsRng.fill_bytes(e.as_mut());
    let eph = x25519_dalek::StaticSecret::from(*e);
    let eph_pub = x25519_dalek::PublicKey::from(&eph).to_bytes();
    let ss_x = Zeroizing::new(
        eph.diffie_hellman(&x25519_dalek::PublicKey::from(peer_x))
            .to_bytes(),
    );
    let mut reply = Vec::with_capacity(6 + 32 + ml_kem_768::CT_LEN);
    reply.extend_from_slice(&magic);
    reply.extend_from_slice(&PROTOCOL_VERSION.to_le_bytes());
    reply.extend_from_slice(&eph_pub);
    reply.extend_from_slice(&ct.into_bytes());
    s.write_all(&reply).await?;
    let mut transcript = hello;
    transcript.extend_from_slice(&reply);
    let (i2r, r2i) = derive(ss_m.as_ref(), ss_x.as_ref(), &transcript);
    let (r, w) = s.into_split();
    Ok((
        SendHalf {
            w,
            cipher: ChaCha20Poly1305::new(r2i.as_ref().into()),
            counter: 0,
        },
        RecvHalf {
            r,
            cipher: ChaCha20Poly1305::new(i2r.as_ref().into()),
            counter: 0,
        },
    ))
}
