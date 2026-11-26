//! The trade history (1.0.53): every completed swap the board has recorded, with its
//! figures day by day, and this wallet's own completed swaps (exportable as CSV). What the
//! board sends is rebuilt from typed fields before the page sees it.

use crate::{S, blocking, check, check_open, err};
use anyhow::{Context, Result};
use axum::Json;
use axum::extract::Query;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use krypton_swap::board::MAX_OFFER_SATS;
use krypton_swap::engine::{self, Role, Stage};
use krypton_wallet_core::Wallet;
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

const PAGE_MAX: usize = 200;
/// Days of figures passed on (more than 50 years of daily swaps).
const MAX_DAYS: usize = 20_000;

fn hex64(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|c| c.is_ascii_hexdigit())
}

/// One completed swap as the board reports it, keeping only what the page shows.
fn swap_view(t: &Value) -> Option<Value> {
    let hash = t["hash"].as_str().filter(|h| hex64(h))?;
    let gnr = t["gnr"].as_u64().filter(|g| *g > 0)?;
    let sats = t["sats"]
        .as_u64()
        .filter(|s| *s > 0 && *s <= MAX_OFFER_SATS)?;
    let at = t["at"].as_u64()?;
    let claim = t["claim"].as_object().and_then(|c| {
        Some(json!({
            "txid": c.get("txid")?.as_str().filter(|x| hex64(x))?.to_ascii_lowercase(),
            "height": c.get("height")?.as_u64()?,
            "time": c.get("time")?.as_u64()?,
        }))
    });
    Some(json!({
        "at": at,
        "gnr": gnr,
        "sats": sats,
        "price": sats as f64 / (gnr as f64 / 1e8),
        "hash": hash.to_ascii_lowercase(),
        "verified": t["verified"].as_bool().unwrap_or(false),
        "claim": claim,
        "confirmations": t["confirmations"].as_u64(),
    }))
}

#[derive(Deserialize)]
pub struct PageQ {
    #[serde(default)]
    offset: usize,
    limit: Option<usize>,
    since: Option<u64>,
    #[serde(default)]
    verified: bool,
}

/// A page of every completed swap, newest first. A board before 1.0.53 only shows its last
/// 50 swaps (on its market): then those, marked `fallback`.
pub async fn page(axum::extract::State(app): S, h: HeaderMap, Query(q): Query<PageQ>) -> Response {
    if let Err(x) = check(&app, &h) {
        return x;
    }
    let a = app.clone();
    match blocking(move || {
        let base = crate::board::board_url(&a);
        let limit = q.limit.unwrap_or(50).clamp(1, PAGE_MAX);
        let mut path = format!("/v1/swap/history?offset={}&limit={limit}", q.offset);
        if let Some(t) = q.since {
            path += &format!("&since={t}");
        }
        if q.verified {
            path += "&verified=true";
        }
        if let Ok(v) = crate::board::get_json(&base, &path)
            && let Some(list) = v["swaps"].as_array()
        {
            let swaps: Vec<Value> = list.iter().filter_map(swap_view).collect();
            return Ok(json!({
                "swaps": swaps,
                "total": v["total"].as_u64().unwrap_or(0),
                "more": v["more"].as_bool().unwrap_or(false),
                "offset": q.offset,
            }));
        }
        let m = crate::board::get_json(&base, "/v1/swap/market")
            .unwrap_or_else(|_| crate::board::cached_market());
        let since = q.since.unwrap_or(0);
        let all: Vec<Value> = m["recent"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(swap_view)
            .filter(|s| s["at"].as_u64().unwrap_or(0) >= since)
            .filter(|s| !q.verified || s["verified"] == true)
            .collect();
        let total = all.len();
        let swaps: Vec<Value> = all.into_iter().skip(q.offset).take(limit).collect();
        let more = q.offset + swaps.len() < total;
        Ok(json!({"swaps": swaps, "total": total, "more": more, "offset": q.offset, "fallback": true}))
    })
    .await
    {
        Ok(v) => Json(v).into_response(),
        Err(e) => err(e),
    }
}

/// Figures of a set of swaps (all of them, the last 24 hours, or one day), numbers only.
fn figures(v: &Value) -> Value {
    let f = |k: &str| v[k].as_f64().filter(|x| x.is_finite() && *x >= 0.0);
    let n = |k: &str| v[k].as_u64();
    json!({
        "swaps": n("swaps").unwrap_or(0),
        "verified": n("verified").unwrap_or(0),
        "gnr": n("gnr").unwrap_or(0),
        "sats": n("sats").unwrap_or(0),
        "low": f("low"),
        "high": f("high"),
        "median": f("median"),
        "vwap": f("vwap"),
        "day": n("day"),
        "first": n("first"),
        "last": n("last"),
    })
}

/// The whole history in figures: totals, the last 24 hours, and day by day. `fallback`
/// from a board before 1.0.53 (the page then works it out from the swaps it has).
pub async fn summary(axum::extract::State(app): S, h: HeaderMap) -> Response {
    if let Err(x) = check(&app, &h) {
        return x;
    }
    let a = app.clone();
    match blocking(move || {
        let base = crate::board::board_url(&a);
        Ok(match crate::board::get_json(&base, "/v1/swap/history/summary") {
            Ok(v) if v["all"].is_object() => json!({
                "all": figures(&v["all"]),
                "day": figures(&v["day"]),
                "days": v["days"].as_array().into_iter().flatten().take(MAX_DAYS).map(figures).collect::<Vec<_>>(),
                "now": v["now"].as_u64(),
            }),
            _ => json!({"fallback": true}),
        })
    })
    .await
    {
        Ok(v) => Json(v).into_response(),
        Err(e) => err(e),
    }
}

/// This wallet's completed swaps, newest first: what it bought and sold, at what price.
fn mine_list(w: &Wallet) -> Vec<Value> {
    let mut v: Vec<(u64, Value)> = engine::load(w)
        .iter()
        .filter(|s| s.stage == Stage::Done && s.gnr > 0)
        .map(|s| {
            let at = s.log.last().map(|l| l.0).unwrap_or(s.created);
            let side = if s.role == Role::BtcSeller {
                "buy"
            } else {
                "sell"
            };
            (
                at,
                json!({
                    "side": side,
                    "gnr": s.gnr,
                    "sats": s.sats,
                    "price": s.sats as f64 / (s.gnr as f64 / 1e8),
                    "at": at,
                    "hash": hex::encode(s.hash),
                }),
            )
        })
        .collect();
    v.sort_by_key(|x| std::cmp::Reverse(x.0));
    v.into_iter().map(|x| x.1).collect()
}

pub async fn mine(axum::extract::State(app): S, h: HeaderMap) -> Response {
    if let Err(x) = check_open(&app, &h) {
        return x;
    }
    let list = app
        .wallet
        .lock()
        .as_ref()
        .map(mine_list)
        .unwrap_or_default();
    Json(json!({"trades": list})).into_response()
}

/// A date and time in UTC, `2026-11-26 02:30:55`.
fn utc(t: u64) -> String {
    let days = (t / 86_400) as i64;
    let secs = t % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}",
        secs / 3600,
        secs / 60 % 60,
        secs % 60
    )
}

fn units(v: u64) -> String {
    format!("{}.{:08}", v / 100_000_000, v % 100_000_000)
}

fn csv(list: &[Value]) -> String {
    let mut s =
        String::from("date_utc,side,gnr,btc,price_btc_per_gnr,price_sat_per_gnr,swap_hash\n");
    for t in list {
        let (gnr, sats) = (
            t["gnr"].as_u64().unwrap_or(0),
            t["sats"].as_u64().unwrap_or(0),
        );
        let price = t["price"].as_f64().unwrap_or(0.0);
        s += &format!(
            "{},{},{},{},{:.8},{:.2},{}\n",
            utc(t["at"].as_u64().unwrap_or(0)),
            t["side"].as_str().unwrap_or(""),
            units(gnr),
            units(sats),
            price / 1e8,
            price,
            t["hash"].as_str().unwrap_or(""),
        );
    }
    s
}

/// The user's Downloads folder, or `fallback` when there is none.
fn downloads(fallback: &Path) -> PathBuf {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
        .map(|h| PathBuf::from(h).join("Downloads"))
        .filter(|d| d.is_dir())
        .unwrap_or_else(|| fallback.to_path_buf())
}

/// Writes this wallet's completed swaps as a CSV file into Downloads; returns its path.
pub async fn export(axum::extract::State(app): S, h: HeaderMap) -> Response {
    if let Err(x) = check_open(&app, &h) {
        return x;
    }
    let list = app
        .wallet
        .lock()
        .as_ref()
        .map(mine_list)
        .unwrap_or_default();
    if list.is_empty() {
        return err("no completed swaps to export yet");
    }
    let dir = downloads(&app.dir);
    match blocking(move || -> Result<String> {
        use std::io::Write;
        let name = format!(
            "genoryn-trades-{}.csv",
            utc(krypton_core::now()).replace([' ', ':'], "-")
        );
        let path = dir.join(name);
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .context("cannot write the file")?;
        f.write_all(csv(&list).as_bytes())?;
        Ok(path.display().to_string())
    })
    .await
    {
        Ok(p) => Json(json!({"path": p})).into_response(),
        Err(e) => err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_are_utc() {
        assert_eq!(utc(0), "1970-01-01 00:00:00");
        assert_eq!(utc(951_782_400), "2000-02-29 00:00:00");
        assert_eq!(utc(1_793_738_591), "2026-11-03 20:43:11");
    }

    #[test]
    fn board_swaps_are_rebuilt_from_typed_fields() {
        let ok = json!({"at": 5, "gnr": 200_000_000u64, "sats": 2_000, "hash": "ab".repeat(32),
            "verified": true, "claim": {"txid": "CD".repeat(32), "height": 9, "time": 7, "extra": "<b>"},
            "note": "<script>"});
        let v = swap_view(&ok).unwrap();
        assert_eq!(v["price"], 1000.0);
        assert_eq!(v["claim"]["txid"], "cd".repeat(32));
        assert!(v.get("note").is_none() && v["claim"].get("extra").is_none());
        for bad in [
            json!({"at": 5, "gnr": 0, "sats": 2, "hash": "ab".repeat(32)}),
            json!({"at": 5, "gnr": 1, "sats": 2, "hash": "<img>"}),
            json!({"at": 5, "gnr": 1, "sats": MAX_OFFER_SATS + 1, "hash": "ab".repeat(32)}),
            json!({"gnr": 1, "sats": 2, "hash": "ab".repeat(32)}),
        ] {
            assert!(swap_view(&bad).is_none(), "{bad}");
        }
        let csv = csv(&[
            json!({"at": 0, "side": "buy", "gnr": 150_000_000u64, "sats": 1_500, "price": 1000.0, "hash": "ab"}),
        ]);
        assert!(
            csv.ends_with("1970-01-01 00:00:00,buy,1.50000000,0.00001500,0.00001000,1000.00,ab\n"),
            "{csv}"
        );
    }
}
