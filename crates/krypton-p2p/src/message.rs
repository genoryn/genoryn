//! Peer protocol messages (binary, bounded).

use anyhow::{Result, bail, ensure};
use krypton_core::Hash32;
use krypton_core::codec::{Reader, Writer};

pub const MAX_HASHES: usize = 2_000;
pub const MAX_ADDRS: usize = 250;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hello {
    pub version: u32,
    pub chain_id: String,
    pub genesis: Hash32,
    pub height: u64,
    pub best: Hash32,
    pub user_agent: String,
    pub listen_port: u16,
    pub nonce: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Message {
    Hello(Hello),
    Ping(u64),
    Pong(u64),
    /// Main-chain hashes after the first locator entry the peer knows.
    GetHashes {
        locator: Vec<Hash32>,
        max: u32,
    },
    Hashes(Vec<Hash32>),
    GetBlocks(Vec<Hash32>),
    Block(Vec<u8>),
    Inv {
        blocks: Vec<Hash32>,
        txs: Vec<Hash32>,
    },
    GetTxs(Vec<Hash32>),
    Tx(Vec<u8>),
    GetAddr,
    Addr(Vec<String>),
    /// Mining shares (encoded headers), pushed as they are found.
    Shares(Vec<Vec<u8>>),
}

pub const MAX_SHARES_PER_MESSAGE: usize = 256;

fn hashes(w: &mut Writer, v: &[Hash32]) {
    w.u32(v.len() as u32);
    for h in v {
        w.raw(h);
    }
}

fn read_hashes(r: &mut Reader) -> Result<Vec<Hash32>> {
    let n = r.u32()? as usize;
    ensure!(n <= MAX_HASHES, "too many hashes");
    Ok((0..n)
        .map(|_| r.array())
        .collect::<krypton_core::Result<_>>()?)
}

impl Message {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::default();
        match self {
            Message::Hello(h) => {
                w.u8(0)
                    .u32(h.version)
                    .bytes(h.chain_id.as_bytes())
                    .raw(&h.genesis)
                    .u64(h.height)
                    .raw(&h.best);
                w.bytes(h.user_agent.as_bytes())
                    .u16(h.listen_port)
                    .u64(h.nonce);
            }
            Message::Ping(n) => {
                w.u8(1).u64(*n);
            }
            Message::Pong(n) => {
                w.u8(2).u64(*n);
            }
            Message::GetHashes { locator, max } => {
                w.u8(3);
                hashes(&mut w, locator);
                w.u32(*max);
            }
            Message::Hashes(v) => {
                w.u8(4);
                hashes(&mut w, v);
            }
            Message::GetBlocks(v) => {
                w.u8(5);
                hashes(&mut w, v);
            }
            Message::Block(b) => {
                w.u8(6).bytes(b);
            }
            Message::Inv { blocks, txs } => {
                w.u8(7);
                hashes(&mut w, blocks);
                hashes(&mut w, txs);
            }
            Message::GetTxs(v) => {
                w.u8(8);
                hashes(&mut w, v);
            }
            Message::Tx(b) => {
                w.u8(9).bytes(b);
            }
            Message::GetAddr => {
                w.u8(10);
            }
            Message::Addr(v) => {
                w.u8(11).u32(v.len() as u32);
                for a in v {
                    w.bytes(a.as_bytes());
                }
            }
            Message::Shares(v) => {
                w.u8(12).u32(v.len() as u32);
                for s in v {
                    w.bytes(s);
                }
            }
        }
        w.0
    }

    pub fn decode(b: &[u8]) -> Result<Self> {
        let mut r = Reader::new(b);
        let s = |r: &mut Reader, max| -> Result<String> { Ok(String::from_utf8(r.bytes(max)?)?) };
        let m = match r.u8()? {
            0 => Message::Hello(Hello {
                version: r.u32()?,
                chain_id: s(&mut r, 64)?,
                genesis: r.array()?,
                height: r.u64()?,
                best: r.array()?,
                user_agent: s(&mut r, 128)?,
                listen_port: r.u16()?,
                nonce: r.u64()?,
            }),
            1 => Message::Ping(r.u64()?),
            2 => Message::Pong(r.u64()?),
            3 => Message::GetHashes {
                locator: read_hashes(&mut r)?,
                max: r.u32()?,
            },
            4 => Message::Hashes(read_hashes(&mut r)?),
            5 => Message::GetBlocks(read_hashes(&mut r)?),
            6 => Message::Block(r.bytes(krypton_core::params::MAX_BLOCK_BYTES + 1024)?),
            7 => Message::Inv {
                blocks: read_hashes(&mut r)?,
                txs: read_hashes(&mut r)?,
            },
            8 => Message::GetTxs(read_hashes(&mut r)?),
            9 => Message::Tx(r.bytes(krypton_core::params::MAX_TX_BYTES)?),
            10 => Message::GetAddr,
            11 => {
                let n = r.u32()? as usize;
                ensure!(n <= MAX_ADDRS, "too many addresses");
                Message::Addr((0..n).map(|_| s(&mut r, 100)).collect::<Result<_>>()?)
            }
            12 => {
                let n = r.u32()? as usize;
                ensure!(n <= MAX_SHARES_PER_MESSAGE, "too many shares");
                Message::Shares(
                    (0..n)
                        .map(|_| r.bytes(krypton_core::types::HEADER_LEN))
                        .collect::<krypton_core::Result<_>>()?,
                )
            }
            t => bail!("unknown message type {t}"),
        };
        r.finish()?;
        Ok(m)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn roundtrip() {
        let msgs = vec![
            Message::Hello(Hello {
                version: 1,
                chain_id: "x".into(),
                genesis: [1; 32],
                height: 9,
                best: [2; 32],
                user_agent: "u".into(),
                listen_port: 5,
                nonce: 7,
            }),
            Message::GetHashes {
                locator: vec![[3; 32]],
                max: 10,
            },
            Message::Inv {
                blocks: vec![[4; 32]],
                txs: vec![],
            },
            Message::Addr(vec!["1.2.3.4:5".into()]),
            Message::Block(vec![1, 2, 3]),
            Message::Shares(vec![vec![7; 10], vec![]]),
        ];
        for m in msgs {
            assert_eq!(Message::decode(&m.encode()).unwrap(), m);
        }
        assert!(Message::decode(&[99]).is_err());
        let mut b = Message::Ping(1).encode();
        b.push(0);
        assert!(Message::decode(&b).is_err());
    }
}
