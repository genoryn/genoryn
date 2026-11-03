/* Explorer views on the public node API (/v1). Hash routes: #/ , #/block/<id>, #/tx/<id>, #/nullifier/<hex> */
(function () {
  var view = document.getElementById("view");
  function card(html) { return '<div class="glass table-card">' + html + "</div>"; }
  function short(h) { return h ? h.slice(0, 10) + "…" + h.slice(-8) : ""; }
  function kv(rows) { return '<div class="kv">' + rows.map(function (r) { return "<div>" + r[0] + "</div><div>" + r[1] + "</div>"; }).join("") + "</div>"; }
  function err(m) { view.innerHTML = card('<p style="padding:18px" class="err-site">' + gk.esc(m) + "</p>"); }

  function stats() {
    gk.api("/v1/info").then(function (i) {
      document.getElementById("sHeight").textContent = Number(i.height).toLocaleString("en-US");
      document.getElementById("sSupply").textContent = gk.gnr(i.supply, 0);
      document.getElementById("sRate").textContent = gk.rate(i.hashrate);
      document.getElementById("sPool").textContent = i.mempool;
      gk.miners(i.stats);
    }).catch(function () {});
  }

  var dashEl = document.getElementById("dash"), dash = null;
  if (dashEl && window.GKCharts) {
    var kpi = function (k, v) { var e = dashEl.querySelector('[data-kpi="' + k + '"]'); if (e) e.textContent = v; };
    dash = GKCharts.dashboard(dashEl, function (r) { return gk.api("/v1/stats?range=" + r); }, function (s, pts) {
      var last = pts[pts.length - 1] || {};
      var avg = function (k) { return pts.length ? pts.reduce(function (a, p) { return a + Number(p[k] || 0); }, 0) / pts.length : 0; };
      kpi("hashrate", gk.rate(s.hashrate));
      kpi("difficulty", GKCharts.fmtNum(last.difficulty || 0));
      kpi("block_time", avg("block_time").toFixed(0) + " s avg");
      kpi("shares", avg("shares").toFixed(1) + " avg");
      kpi("miners", String(s.estimated_miners || 0));
      kpi("issued", gk.gnr(s.issued, 0) + " GNR");
      kpi("issued2", gk.gnr(s.issued, 0) + " GNR");
      kpi("remaining", gk.gnr(s.remaining, 0) + " GNR");
      GKCharts.disc(document.getElementById("supplyDisc"), Number(s.issued || 0), Number(s.max_supply || 21e14));
    });
    GKCharts.tilt(".tilt-card");
  }

  function home() {
    gk.api("/v1/blocks?count=25").then(function (bs) {
      var rows = bs.map(function (b) {
        return '<tr><td data-label="Height"><a href="#/block/' + b.height + '">' + b.height + '</a></td><td data-label="Hash" class="mono"><a href="#/block/' + b.hash + '">' + short(b.hash) +
          '</a></td><td data-label="Age">' + gk.ago(b.timestamp) + '</td><td data-label="Shares" class="num">' + (b.shares || 0) + '</td><td data-label="Transactions" class="num">' + b.txs +
          '</td><td data-label="Reward" class="num">' + gk.gnr((b.subsidy || 0) + (b.fees || 0)) +
          '</td><td data-label="Size" class="num">' + (b.size / 1024).toFixed(1) + " KB</td></tr>";
      }).join("");
      view.innerHTML = '<h2 style="font-size:20px;margin:8px 0 12px">Latest blocks</h2>' + card('<table class="data"><thead><tr><th>Height</th><th>Hash</th><th>Age</th><th class="num">Shares</th><th class="num">Txs</th><th class="num">Reward GNR</th><th class="num">Size</th></tr></thead><tbody>' + rows + "</tbody></table>") +
        '<p class="muted" style="margin-top:12px;font-size:13.5px">Each block adds its reward to a shared pool; every share credits its miner an equal slice, so miners earn in proportion to their hashrate.</p>';
    }).catch(function () { err("The node API is not reachable right now."); });
  }

  function block(id) {
    gk.api("/v1/block/" + encodeURIComponent(id)).then(function (b) {
      var h = b.header;
      var txs = b.txs.map(function (t) {
        return '<tr><td data-label="Txid" class="mono"><a href="#/tx/' + t.txid + '">' + short(t.txid) + '</a></td><td data-label="Kind">' + t.kind + '</td><td data-label="Fee" class="num">' + gk.gnr(t.fee) +
          '</td><td data-label="Proof" class="num">' + (t.proof_bytes / 1024).toFixed(1) + " KB</td></tr>";
      }).join("") || '<tr><td colspan="4" class="muted">No transactions</td></tr>';
      var shares = (b.shares || []).map(function (s) {
        return '<tr><td data-label="Share" class="mono">' + short(s.hash) + '</td><td data-label="Mined on block" class="num">' + s.height + '</td><td data-label="Miner" class="mono">' + short(s.miner) + "</td></tr>";
      }).join("") || '<tr><td colspan="3" class="muted">Only the block itself was credited</td></tr>';
      var payouts = (b.payouts || []).map(function (p) {
        return '<tr><td data-label="Miner" class="mono">' + short(p.owner) + '</td><td data-label="Paid" class="num">' + gk.gnr(p.value) + " GNR</td></tr>";
      }).join("");
      view.innerHTML = '<h2 style="font-size:20px;margin:8px 0 12px">Block ' + h.height + "</h2>" + card(kv([
        ["Hash", '<span class="mono">' + b.hash + "</span>"], ["Previous", '<a class="mono" href="#/block/' + h.prev + '">' + h.prev + "</a>"],
        ["Time", new Date(h.timestamp * 1000).toISOString().replace("T", " ").slice(0, 19) + " UTC"], ["Confirmations", b.confirmations],
        ["Difficulty", Number(h.difficulty).toLocaleString("en-US")], ["Nonce", h.nonce],
        ["Reward to the pool", gk.gnr((b.subsidy || 0) + (b.fees || 0)) + " GNR"], ["Found by", '<span class="mono">' + short(b.miner) + "</span>"],
        ["Shares credited", (b.shares || []).length + 1], ["Payouts made", (b.payouts || []).length],
        ["Note tree root", '<span class="mono">' + h.note_root + "</span>"], ["MMR root (light clients)", '<span class="mono">' + h.mmr_root + "</span>"], ["Size", (b.size / 1024).toFixed(1) + " KB"]
      ])) +
        (payouts ? '<h2 style="font-size:20px;margin:22px 0 12px">Payouts</h2>' + card('<table class="data"><thead><tr><th>Miner</th><th class="num">Paid</th></tr></thead><tbody>' + payouts + "</tbody></table>") : "") +
        '<h2 style="font-size:20px;margin:22px 0 12px">Shares</h2>' + card('<table class="data"><thead><tr><th>Share</th><th class="num">Mined on block</th><th>Miner</th></tr></thead><tbody>' + shares + "</tbody></table>") +
        '<h2 style="font-size:20px;margin:22px 0 12px">Transactions</h2>' + card('<table class="data"><thead><tr><th>Txid</th><th>Kind</th><th class="num">Fee</th><th class="num">Proof</th></tr></thead><tbody>' + txs + "</tbody></table>");
    }).catch(function () { err("Block not found."); });
  }

  function tx(id) {
    gk.api("/v1/tx/" + encodeURIComponent(id)).then(function (t) {
      var rows = [["Transaction", '<span class="mono">' + t.txid + "</span>"], ["Status", t.status === "confirmed" ? '<span class="ok">confirmed</span>' : '<span class="warn">pending</span>']];
      if (t.height != null) rows.push(["Block", '<a href="#/block/' + t.height + '">' + t.height + "</a>"], ["Confirmations", t.confirmations]);
      if (t.fee != null) rows.push(["Fee", gk.gnr(t.fee) + " GNR"]);
      rows.push(["Privacy", "Sender, recipient and amount are hidden by a zk-STARK proof."]);
      view.innerHTML = '<h2 style="font-size:20px;margin:8px 0 12px">Transaction</h2>' + card(kv(rows));
    }).catch(function () { err("Transaction not found."); });
  }

  function nullifier(id) {
    gk.api("/v1/nullifier/" + encodeURIComponent(id)).then(function (n) {
      view.innerHTML = '<h2 style="font-size:20px;margin:8px 0 12px">Nullifier</h2>' + card(kv([["Nullifier", '<span class="mono">' + gk.esc(id) + "</span>"],
        ["Spent", n.spent_height != null ? '<span class="ok">yes</span>, in block <a href="#/block/' + n.spent_height + '">' + n.spent_height + "</a>" : "not spent"]]));
    }).catch(function () { err("Invalid nullifier."); });
  }

  window.exSearch = function (e) {
    e.preventDefault();
    var q = document.getElementById("q").value.trim();
    if (/^\d+$/.test(q)) location.hash = "#/block/" + q;
    else if (/^[0-9a-fA-F]{64}$/.test(q)) {
      gk.api("/v1/block/" + q).then(function () { location.hash = "#/block/" + q; }).catch(function () {
        gk.api("/v1/tx/" + q).then(function () { location.hash = "#/tx/" + q; }).catch(function () { location.hash = "#/nullifier/" + q; });
      });
    } else err("Enter a block height or a 64-character hex id.");
    return false;
  };

  function route() {
    var p = location.hash.replace(/^#\/?/, "").split("/");
    if (p[0] === "block" && p[1]) block(p[1]);
    else if (p[0] === "tx" && p[1]) tx(p[1]);
    else if (p[0] === "nullifier" && p[1]) nullifier(p[1]);
    else home();
  }
  window.addEventListener("hashchange", route);
  stats(); route();
  setInterval(function () { if (!document.hidden) { stats(); if (!location.hash || location.hash === "#/") home(); } }, 20000);
  setInterval(function () { if (!document.hidden && dash) dash.reload(); }, 60000);
})();
