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
  var explorer = GKI18N.path("/explorer");
  function block(h) { return h ? '<a href="' + explorer + "#/block/" + nm(h) + '">' + gk.esc(T("block {h}", { h: GKI18N.num(nm(h)) })) + "</a>" : "—"; }
  function minutes(n) {
    n = Math.max(0, Math.round(n));
    return n < 90 ? T("{n} min", { n: n }) : T("{n} h", { n: GKI18N.num(n / 60, { maximumFractionDigits: 1 }) });
  }

  function render(addr, a, m, info) {
    var height = nm(a.height), act = (a.mining || {}).activity || {}, pays = act.payouts || [];
    var blocks = Math.max(1, nm(m.shares_recent_blocks)), recent = nm(m.shares_recent), pending = nm(m.shares_pending);
    var perBlock = nm(m.network_shares_per_block) || 32;
    var frac = Math.min(1, recent / (perBlock * blocks));
    var last = nm(act.last_seen), since = last ? height - last : Infinity;
    var status = pending > 0 || since <= 5
      ? '<span class="ok"><span class="dot live ok"></span> <b>' + gk.esc(T("Mining")) + "</b>: " + gk.esc(T("its shares are reaching the network")) + "</span>"
      : last
        ? '<span class="warn"><b>' + gk.esc(T("No recent shares")) + "</b>: " + gk.esc(T("the last was {block} ({ago} ago)", { block: "{block}", ago: minutes(since) })).replace("{block}", block(last)) + "</span>"
        : '<span class="warn"><b>' + gk.esc(T("No shares yet")) + "</b> " + gk.esc(T("from this address")) + "</span>";
    var nph = nm(m.next_payout_height), paid = pays.reduce(function (s, p) { return s + nm(p.value); }, 0);
    var perDay = frac * 1440 * nm(info.next_subsidy);
    var pct = GKI18N.num(frac * 100, { maximumFractionDigits: frac < 0.01 ? 3 : 2 });
    var e = gk.esc;
    out.innerHTML = '<div class="glass table-card" style="margin-top:14px">' + kv([
      [e(T("Status")), status],
      [e(T("Shares in the last {n} blocks", { n: blocks })), e(T("{mine} of about {all} on the network", { mine: GKI18N.num(recent), all: GKI18N.num(perBlock * blocks) }))],
      [e(T("Waiting at this node")), e(Tn(pending, "{n} share (credited by the next block)", "{n} shares (credited by the next block)"))],
      [e(T("Hashrate (estimate)")), recent ? e("≈ " + gk.rate(frac * nm(info.hashrate)) + " · " + T("{pct} % of the network", { pct: pct })) : "—"],
      [e(T("GNR per day (estimate)")), recent ? e("≈ " + T("{gnr} GNR at the current rate", { gnr: gk.gnr(perDay, perDay >= 1e10 ? 0 : 2) })) : "—"],
      [e(T("Rewards accruing")), "<b>" + e(gk.gnr(nm((a.mining || {}).unpaid))) + " GNR</b>"],
      [e(T("Next payout")), nph ? block(nph) + " " + e(nph > height ? T("(in about {t})", { t: minutes(nph - height) }) : T("(due now)")) : e(T("after its first credited share"))],
      [e(T("Paid so far")), e(Tn(pays.length, "{gnr} GNR in {n} payout", "{gnr} GNR in {n} payouts", { gnr: gk.gnr(paid) }))],
      [e(T("Last share")), block(last)]
    ]) + '<p class="muted" style="padding:0 18px 16px;margin:12px 0 0">' + e(T("Estimates come from the shares this address found in recent blocks; only the payouts in your wallet are final.")) + " " +
      '<a href="' + explorer + "#/address/" + encodeURIComponent(addr) + '">' + e(T("Full history in the explorer")) + "</a>. " + e(T("Updates every minute.")) + "</p></div>";
  }

  function check(addr, quiet) {
    current = addr;
    if (!quiet) out.innerHTML = '<p class="muted" style="margin-top:14px"><span class="mchk-spin" aria-hidden="true"></span> ' + gk.esc(T("Looking up…")) + "</p>";
    api("/v1/address-info/" + encodeURIComponent(addr)).then(function (a) {
      if (a.kind === "short") throw new Error(T("This short address has not been published, so it cannot be resolved. Publish it in the wallet (Receive → Publish short form) or enter the full address."));
      if (!/^[0-9a-f]{64}$/.test(a.owner || "")) throw new Error(T("Unexpected answer from the node."));
      return Promise.all([api("/v1/miner/" + a.owner), api("/v1/info")]).then(function (r) { if (addr === current) render(addr, a, r[0], r[1]); });
    }).catch(function (e) {
      if (quiet || addr !== current) return;
      var msg = /^\d+$/.test(e.message) ? T("The node could not be reached ({code}). Try again in a minute.", { code: e.message }) : e.message;
      out.innerHTML = '<p class="bad" style="margin-top:14px">' + gk.esc(msg) + "</p>";
    });
  }

  form.addEventListener("submit", function (e) {
    e.preventDefault();
    var addr = input.value.trim().replace(/\s+/g, "");
    if (!/^[a-z]*gnrs?1[a-z2-7]+$/.test(addr)) { out.innerHTML = '<p class="bad" style="margin-top:14px">' + gk.esc(T("Enter a Genoryn Krypton address: gnrs1… or gnr1…")) + "</p>"; return; }
    try { localStorage.setItem(KEY, addr); } catch (x) {}
    check(addr);
    clearInterval(timer);
    timer = setInterval(function () { if (!document.hidden) check(addr, true); }, 60000);
  });
  try { var saved = localStorage.getItem(KEY); if (saved) input.value = saved; } catch (x) {}
})();
