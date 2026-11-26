#!/usr/bin/env python3
"""What a Genoryn Krypton transaction still shows on chain, measured.

Reads blocks from a node's public API (read-only, standard library only) and reports the
metadata every observer sees: transaction kinds, fees, sizes, input/output shape, how
transactions are spread over blocks and over the day, and how many transactions carry a
combination of these that no other transaction has (a fingerprint that shrinks the
anonymity set). Amounts, senders and recipients are not on chain and are not measured.

    python chain_metadata.py [--node https://genoryn.io] [--from 0] [--to TIP]
                             [--cache blocks.jsonl] [--json out.json] [--delay 0.25]

The cache keeps fetched transactions, so a second run only fetches new blocks.
"""

import argparse
import collections
import json
import math
import os
import statistics
import sys
import time
import urllib.request


def get(node, path, delay, tries=5):
    for i in range(tries):
        try:
            req = urllib.request.Request(node + path, headers={"User-Agent": "chain-metadata/1"})
            with urllib.request.urlopen(req, timeout=30) as r:
                body = r.read()
            time.sleep(delay)
            return json.loads(body)
        except Exception as e:  # rate limits and timeouts: back off and retry
            if i == tries - 1:
                raise
            time.sleep(delay * 8 * (i + 1))
            print(f"retry {path}: {e}", file=sys.stderr)


def entropy(counter):
    n = sum(counter.values())
    return -sum(c / n * math.log2(c / n) for c in counter.values()) if n else 0.0


def collect(node, lo, hi, cache, delay):
    """Transactions in heights lo..=hi as dicts, from the cache where possible."""
    have = {}
    blocks = {}
    if cache and os.path.exists(cache):
        for line in open(cache, encoding="utf-8"):
            r = json.loads(line)
            if r.get("block"):
                blocks[r["height"]] = r
            else:
                have.setdefault(r["height"], []).append(r)
    out = open(cache, "a", encoding="utf-8") if cache else None
    h = hi
    while h >= lo:
        need = [x for x in range(max(lo, h - 99), h + 1) if x not in blocks]
        if need:
            page = get(node, f"/v1/blocks?from={h}&count=100", delay)
            for b in page:
                if b["height"] < lo or b["height"] in blocks:
                    continue
                rec = {"block": True, "height": b["height"], "timestamp": b["timestamp"],
                       "txs": b["txs"], "size": b["size"]}
                if b["txs"]:
                    full = get(node, f"/v1/block/{b['height']}", delay)
                    for t in full["txs"]:
                        tx = {"height": b["height"], "timestamp": b["timestamp"],
                              "kind": t.get("kind"), "fee": t.get("fee"), "size": t.get("size"),
                              "proof_bytes": t.get("proof_bytes"),
                              "inputs": len(t.get("nullifiers") or []),
                              "outputs": len(t.get("commitments") or []),
                              "coin": t.get("coin")}
                        have.setdefault(b["height"], []).append(tx)
                        if out:
                            out.write(json.dumps(tx) + "\n")
                blocks[b["height"]] = rec
                if out:
                    out.write(json.dumps(rec) + "\n")
            print(f"  blocks down to {h - 99}", file=sys.stderr)
        h -= 100
    if out:
        out.close()
    txs = [t for x in range(lo, hi + 1) for t in have.get(x, [])]
    return txs, {x: blocks[x] for x in range(lo, hi + 1) if x in blocks}


def analyse(txs, blocks):
    n = len(txs)
    r = {"blocks": len(blocks), "transactions": n}
    if not blocks:
        return r
    per_block = collections.Counter(b["txs"] for b in blocks.values())
    r["blocks_with_transactions"] = sum(c for k, c in per_block.items() if k)
    r["transactions_per_block"] = dict(sorted(per_block.items()))
    if not n:
        return r
    kinds = collections.Counter(t["kind"] for t in txs)
    fees = collections.Counter(t["fee"] for t in txs)
    sizes = collections.Counter(t["size"] for t in txs)
    shapes = collections.Counter(f"{t['inputs']}-in/{t['outputs']}-out" for t in txs)
    hours = collections.Counter(time.gmtime(t["timestamp"]).tm_hour for t in txs)
    print_ = collections.Counter((t["kind"], t["fee"], t["size"], t["inputs"], t["outputs"], t["coin"]) for t in txs)
    unique = sum(1 for t in txs if print_[(t["kind"], t["fee"], t["size"], t["inputs"], t["outputs"], t["coin"])] == 1)
    sets = sorted(print_[(t["kind"], t["fee"], t["size"], t["inputs"], t["outputs"], t["coin"])] for t in txs)
    by_block = collections.Counter(t["height"] for t in txs)
    shared = sum(1 for t in txs if by_block[t["height"]] > 1)
    times = sorted(t["timestamp"] for t in txs)
    gaps = [b - a for a, b in zip(times, times[1:])]
    r.update({
        "kinds": dict(kinds.most_common()),
        "shapes": dict(shapes.most_common()),
        "fee_values": len(fees),
        "fee_top": [[f, c] for f, c in fees.most_common(8)],
        "fee_entropy_bits": round(entropy(fees), 3),
        "size_values": len(sizes),
        "size_top": [[s, c] for s, c in sizes.most_common(8)],
        "size_entropy_bits": round(entropy(sizes), 3),
        "size_range": [min(sizes), max(sizes)],
        "proof_bytes_values": len(collections.Counter(t["proof_bytes"] for t in txs)),
        "fingerprint_unique_share": round(unique / n, 4),
        "fingerprint_set_median": statistics.median(sets),
        "same_block_share": round(shared / n, 4),
        "gap_median_secs": statistics.median(gaps) if gaps else None,
        "hour_entropy_bits": round(entropy(hours), 3),
        "hour_entropy_max_bits": round(math.log2(24), 3),
        "first_tx_height": min(t["height"] for t in txs),
        "last_tx_height": max(t["height"] for t in txs),
    })
    return r


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--node", default="https://genoryn.io")
    ap.add_argument("--from", dest="lo", type=int, default=0)
    ap.add_argument("--to", dest="hi", type=int)
    ap.add_argument("--cache", default="chain-metadata-cache.jsonl")
    ap.add_argument("--json")
    ap.add_argument("--delay", type=float, default=0.25)
    a = ap.parse_args()
    node = a.node.rstrip("/")
    hi = a.hi if a.hi is not None else get(node, "/v1/info", a.delay)["height"]
    txs, blocks = collect(node, a.lo, hi, a.cache, a.delay)
    r = analyse(txs, blocks)
    r.update({"node": node, "from": a.lo, "to": hi, "measured_at": int(time.time())})
    text = json.dumps(r, indent=2)
    if a.json:
        open(a.json, "w", encoding="utf-8").write(text + "\n")
    print(text)


if __name__ == "__main__":
    main()
