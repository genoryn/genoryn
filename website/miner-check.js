/* "Check your miner" on /mining: public, read-only node data for one address. */
(function () {
  var form = document.getElementById("mchk"), input = document.getElementById("mchkAddr"), out = document.getElementById("mchkOut");
  if (!form || !window.gk) return;
  var KEY = "gk-miner-address", timer = null, current = "";
  function nm(v) { var n = Number(v); return isFinite(n) ? n : 0; }
  function kv(rows) { return '<div class="kv">' + rows.map(function (r) { return "<div>" + r[0] + "</div><div>" + r[1] + "</div>"; }).join("") + "</div>"; }
  function api(p) {
    return fetch(p, { headers: { accept: "application/json" } }).then(function (r) {
      return r.json().catch(function () { return {}; }).then(function (j) { if (!r.ok) throw new Error(j.error || String(r.status)); return j; });
    });
  }
  function block(h) { return h ? '<a href="/explorer#/block/' + nm(h) + '">block ' + nm(h).toLocaleString("en-US") + "</a>" : "—"; }
  function minutes(n) { n = Math.max(0, Math.round(n)); return n < 90 ? n + " min" : (n / 60).toFixed(1) + " h"; }

  function render(addr, a, m, info) {
    var height = nm(a.height), act = (a.mining || {}).activity || {}, pays = act.payouts || [];
    var blocks = Math.max(1, nm(m.shares_recent_blocks)), recent = nm(m.shares_recent), pending = nm(m.shares_pending);
    var perBlock = nm(m.network_shares_per_block) || 32;
    var frac = Math.min(1, recent / (perBlock * blocks));
    var last = nm(act.last_seen), since = last ? height - last : Infinity;
    var status = pending > 0 || since <= 5
      ? '<span class="ok"><span class="dot live ok"></span> <b>Mining</b>: its shares are reaching the network</span>'
      : last
        ? '<span class="warn"><b>No recent shares</b>: the last was ' + block(last) + " (" + minutes(since) + " ago)</span>"
        : '<span class="warn"><b>No shares yet</b> from this address</span>';
    var nph = nm(m.next_payout_height), paid = pays.reduce(function (s, p) { return s + nm(p.value); }, 0);
    var perDay = frac * 1440 * nm(info.next_subsidy);
    out.innerHTML = '<div class="glass table-card" style="margin-top:14px">' + kv([
      ["Status", status],
      ["Shares in the last " + blocks + " blocks", "<b>" + recent.toLocaleString("en-US") + "</b> of about " + (perBlock * blocks).toLocaleString("en-US") + " on the network"],
      ["Waiting at this node", pending + " share" + (pending === 1 ? "" : "s") + " (credited by the next block)"],
      ["Hashrate (estimate)", recent ? "≈ " + gk.rate(frac * nm(info.hashrate)) + " · " + (frac * 100).toFixed(frac < 0.01 ? 3 : 2) + " % of the network" : "—"],
      ["GNR per day (estimate)", recent ? "≈ " + gk.gnr(perDay, perDay >= 1e10 ? 0 : 2) + " GNR at the current rate" : "—"],
      ["Rewards accruing", "<b>" + gk.gnr(nm((a.mining || {}).unpaid)) + " GNR</b>"],
      ["Next payout", nph ? block(nph) + (nph > height ? " (in about " + minutes(nph - height) + ")" : " (due now)") : "after its first credited share"],
      ["Paid so far", gk.gnr(paid) + " GNR in " + pays.length + " payout" + (pays.length === 1 ? "" : "s")],
      ["Last share", block(last)]
    ]) + '<p class="muted" style="padding:0 18px 16px;margin:12px 0 0">Estimates come from the shares this address found in recent blocks; only the payouts in your wallet are final. ' +
      '<a href="/explorer#/address/' + encodeURIComponent(addr) + '">Full history in the explorer</a>. Updates every minute.</p></div>';
  }

  function check(addr, quiet) {
    current = addr;
    if (!quiet) out.innerHTML = '<p class="muted" style="margin-top:14px"><span class="mchk-spin" aria-hidden="true"></span> Looking up…</p>';
    api("/v1/address-info/" + encodeURIComponent(addr)).then(function (a) {
      if (a.kind === "short") throw new Error("This short address has not been published, so it cannot be resolved. Publish it in the wallet (Receive → Publish short form) or enter the full address.");
      if (!/^[0-9a-f]{64}$/.test(a.owner || "")) throw new Error("Unexpected answer from the node.");
      return Promise.all([api("/v1/miner/" + a.owner), api("/v1/info")]).then(function (r) { if (addr === current) render(addr, a, r[0], r[1]); });
    }).catch(function (e) {
      if (quiet || addr !== current) return;
      var msg = /^\d+$/.test(e.message) ? "The node could not be reached (" + e.message + "). Try again in a minute." : e.message;
      out.innerHTML = '<p class="bad" style="margin-top:14px">' + gk.esc(msg) + "</p>";
    });
  }

  form.addEventListener("submit", function (e) {
    e.preventDefault();
    var addr = input.value.trim().replace(/\s+/g, "");
    if (!/^[a-z]*gnrs?1[a-z2-7]+$/.test(addr)) { out.innerHTML = '<p class="bad" style="margin-top:14px">Enter a Genoryn Krypton address: gnrs1… or gnr1…</p>'; return; }
    try { localStorage.setItem(KEY, addr); } catch (x) {}
    check(addr);
    clearInterval(timer);
    timer = setInterval(function () { if (!document.hidden) check(addr, true); }, 60000);
  });
  try { var saved = localStorage.getItem(KEY); if (saved) input.value = saved; } catch (x) {}
})();
