//! The miner protocol: one JSON object per line over TCP (plain on loopback
//! and onion services, TLS elsewhere).
//!
//! Miner → pool: `login`, `submit`, `keepalive`. Pool → miner: replies with
//! the request `id`, and `job` notifications. Lines are at most
//! [`MAX_LINE_BYTES`]; anything else closes the connection.
//!
//! A job carries the canonical header bytes with nonce zero, the RandomX seed,
//! the session's share difficulty and its nonce range. The miner sets the
//! header's `nonce` (bytes 94..102, little endian), hashes, and submits nonces
//! whose hash meets the share difficulty. The pool hashes every submission
//! again itself before crediting it.
use serde::{Deserialize, Serialize};

pub const MAX_LINE_BYTES: usize = 4_096;
pub const PROTOCOL: &str = "genoryn-pool/1";
/// Offset of the 8-byte little-endian nonce in the canonical header encoding.
pub const NONCE_OFFSET: usize = 94;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "method", content = "params", rename_all = "snake_case")]
pub enum Request {
    Login {
        /// The shielded address payouts go to.
        address: String,
        #[serde(default)]
        worker: String,
        #[serde(default)]
        agent: String,
    },
    Submit {
        job_id: String,
        /// Hex of the 8-byte little-endian nonce.
        nonce: String,
    },
    Keepalive,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientMessage {
    pub id: u64,
    #[serde(flatten)]
    pub request: Request,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Job {
    pub job_id: String,
    pub height: u64,
    /// Hex of the canonical header encoding with nonce zero.
    pub header: String,
    /// Hex RandomX seed (the header's `pow_seed`).
    pub seed: String,
    pub share_difficulty: u64,
    pub network_difficulty: u64,
    /// First nonce of this session's range, hex little endian.
    pub nonce_start: String,
    /// The range holds 2^nonce_bits nonces.
    pub nonce_bits: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Receipt {
    pub job_id: String,
    pub height: u64,
    pub nonce: String,
    /// The share's RandomX hash as the pool computed it.
    pub hash: String,
    /// Work credited: the share difficulty (expected hashes).
    pub work: u64,
    /// The durable work row the share was recorded in.
    pub sequence: i64,
    pub block_candidate: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reject {
    NotLoggedIn,
    InvalidAddress,
    UnknownJob,
    Stale,
    Duplicate,
    OutOfRange,
    LowDifficulty,
    Malformed,
    Busy,
    RateLimited,
    NotOpenYet,
}
impl Reject {
    pub fn message(&self) -> &'static str {
        match self {
            Self::NotLoggedIn => "log in with a payout address first",
            Self::InvalidAddress => "not a GENORYN address of this network",
            Self::UnknownJob => "unknown job",
            Self::Stale => "stale: the chain moved past this job",
            Self::Duplicate => "duplicate share",
            Self::OutOfRange => "nonce outside this session's range",
            Self::LowDifficulty => "the hash does not meet the share difficulty",
            Self::Malformed => "malformed request",
            Self::Busy => "pool busy; retry shortly",
            Self::RateLimited => "too many requests",
            Self::NotOpenYet => "pool accounting has not reached its activation height",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ServerMessage {
    Reply {
        id: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        result: Option<serde_json::Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<RejectBody>,
    },
    Notify {
        method: String,
        params: Job,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RejectBody {
    pub code: Reject,
    pub message: String,
}
impl ServerMessage {
    pub fn ok(id: u64, result: serde_json::Value) -> Self {
        Self::Reply {
            id,
            result: Some(result),
            error: None,
        }
    }
    pub fn reject(id: u64, code: Reject) -> Self {
        Self::Reply {
            id,
            result: None,
            error: Some(RejectBody {
                message: code.message().into(),
                code,
            }),
        }
    }
    pub fn job(job: Job) -> Self {
        Self::Notify {
            method: "job".into(),
            params: job,
        }
    }
}

/// One client line, bounded and strictly typed.
pub fn parse_client(line: &str) -> Result<ClientMessage, Reject> {
    if line.len() > MAX_LINE_BYTES {
        return Err(Reject::Malformed);
    }
    serde_json::from_str(line).map_err(|_| Reject::Malformed)
}

pub fn parse_nonce(text: &str) -> Option<u64> {
    let bytes: [u8; 8] = hex::decode(text).ok()?.try_into().ok()?;
    Some(u64::from_le_bytes(bytes))
}
pub fn format_nonce(nonce: u64) -> String {
    hex::encode(nonce.to_le_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_round_trip_and_hostile_lines_are_refused() {
        let login = r#"{"id":1,"method":"login","params":{"address":"gnr1x","worker":"rig"}}"#;
        let parsed = parse_client(login).unwrap();
        assert_eq!(parsed.id, 1);
        assert!(matches!(parsed.request, Request::Login { ref worker, .. } if worker == "rig"));
        let submit =
            r#"{"id":2,"method":"submit","params":{"job_id":"j","nonce":"0100000000000000"}}"#;
        assert!(matches!(
            parse_client(submit).unwrap().request,
            Request::Submit { .. }
        ));
        assert!(matches!(
            parse_client(r#"{"id":3,"method":"keepalive"}"#)
                .unwrap()
                .request,
            Request::Keepalive
        ));
        for bad in [
            "",
            "{}",
            r#"{"id":1,"method":"mine_for_me"}"#,
            r#"{"id":-1,"method":"keepalive"}"#,
            &"x".repeat(MAX_LINE_BYTES + 1),
        ] {
            assert_eq!(parse_client(bad), Err(Reject::Malformed), "{bad}");
        }
        assert_eq!(
            parse_nonce(&format_nonce(0x0102_0304_0506_0708)),
            Some(0x0102_0304_0506_0708)
        );
        assert_eq!(parse_nonce("01"), None);
        assert_eq!(parse_nonce("zz00000000000000"), None);
        let reply = serde_json::to_string(&ServerMessage::reject(9, Reject::Stale)).unwrap();
        assert!(reply.contains("\"code\":\"stale\""), "{reply}");
    }
}
