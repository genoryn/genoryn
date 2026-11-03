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

  function post(p, body) {
    return fetch(p, { method: "POST", headers: { "content-type": "application/json", accept: "application/json" }, body: JSON.stringify(body) })
      .then(function (r) { return r.json().then(function (j) { if (!r.ok) throw new Error(j.error || r.status); return j; }); });
  }

  function proofForm(prefillAddress) {
    return card('<div style="padding:18px"><h3 class="xh">Verify a payment proof</h3>' +
      '<p class="muted" style="margin:6px 0 12px">Senders create a proof in their wallet (History → Proof). It shows one payment and nothing else, checked against this chain.</p>' +
      '<label for="pf" class="sr-only">Payment proof</label><textarea id="pf" rows="3" placeholder="gnrpay1…" spellcheck="false"></textarea>' +
      '<label for="pfa" class="sr-only">Recipient address (optional)</label><input id="pfa" placeholder="Recipient full address (optional, confirms the recipient)" value="' + gk.esc(prefillAddress || "") + '" spellcheck="false">' +
      '<button class="btn btn-primary" type="button" onclick="exProof()" style="margin-top:10px">Verify</button><div id="pfr" style="margin-top:14px"></div></div>');
  }

  window.exProof = function () {
    var out = document.getElementById("pfr");
    out.innerHTML = '<p class="muted">Checking…</p>';
    post("/v1/verify-payment", { proof: document.getElementById("pf").value.trim(), address: document.getElementById("pfa").value.trim() || null }).then(function (r) {
      out.innerHTML = '<p class="ok"><b>Valid payment proof</b></p>' + kv([
        ["Amount", "<b>" + gk.gnr(r.value) + " GNR</b>"], ["Recipient", '<span class="mono">' + gk.esc(r.recipient) + "</span>" + (r.recipient_confirmed === true ? ' <span class="ok">· matches the address</span>' : r.recipient_confirmed === false ? ' <span class="bad">· does not match the address</span>' : "")],
        ["Memo", gk.esc(r.memo) || "—"], ["Transaction", '<a class="mono" href="#/tx/' + r.txid + '">' + short(r.txid) + "</a>"],
        ["Block", '<a href="#/block/' + r.height + '">' + r.height + "</a> · " + r.confirmations + " confirmations"], ["Time", new Date(r.time * 1000).toISOString().replace("T", " ").slice(0, 19) + " UTC"]
      ]);
    }).catch(function (e) { out.innerHTML = '<p class="bad"><b>Not valid:</b> ' + gk.esc(e.message) + "</p>"; });
  };

  function address(id) {
    view.innerHTML = '<p class="muted">Looking up…</p>';
    gk.api("/v1/address-info/" + encodeURIComponent(id)).then(function (a) {
      var rows = [["Status", '<span class="ok"><b>Valid ' + gk.esc(a.network) + " address</b></span>"], ["Short address", '<span class="mono wrap-any">' + gk.esc(a.short) + "</span>"]];
      var priv = '<p class="muted" style="margin:14px 0 0">Transfers are shielded: incoming and outgoing payments never appear in any explorer, for this or any other address. To show that a payment really happened, ask the sender for a <b>payment proof</b> and check it below.</p>';
      if (a.kind === "short") {
        rows.push(["Full address", a.published ? "published" : '<span class="warn">not published on this node</span>']);
        view.innerHTML = '<h2 style="font-size:20px;margin:8px 0 12px">Address</h2>' + card('<div style="padding:4px 0">' + kv(rows) + '</div><div style="padding:0 18px 18px">' +
          '<p class="muted">The short form is correct, but its owner has not published the full address here, so it cannot be resolved. In the wallet: <b>Wallet → Receive → Publish short form</b>, or search with the full address.</p>' + priv + "</div>") + proofForm();
        return;
      }
      var m = a.mining || {}, act = m.activity || {}, pays = act.payouts || [];
      rows.push(["Published", a.published ? "yes, the short form resolves on this node" : 'no (Wallet → Receive → Publish short form)'], ["Owner tag", '<span class="mono wrap-any">' + gk.esc(a.owner) + "</span>"], ["Cryptography", gk.esc(a.crypto)]);
      var paid = pays.reduce(function (s, p) { return s + Number(p.value); }, 0);
      var eta = m.next_payout_height ? m.next_payout_height + " (in ~" + Math.max(0, m.next_payout_height - a.height) + " min)" : "—";
      var mining = kv([
        ["Rewards accruing", "<b>" + gk.gnr(m.unpaid) + " GNR</b>"], ["Next payout", "block " + eta],
        ["Paid so far", gk.gnr(paid) + " GNR in " + pays.length + " payout" + (pays.length === 1 ? "" : "s")],
        ["Blocks found", String(act.blocks_found || 0)], ["Blocks with its shares", String(act.blocks_with_shares || 0)],
        ["Mining since", act.first_seen ? '<a href="#/block/' + act.first_seen + '">block ' + act.first_seen + "</a>" : "—"], ["Last share", act.last_seen ? '<a href="#/block/' + act.last_seen + '">block ' + act.last_seen + "</a>" : "—"]
      ]);
      var payRows = pays.slice().reverse().map(function (p) {
        var spend = p.height + (m.maturity || 100);
        return '<tr><td data-label="Block"><a href="#/block/' + p.height + '">' + p.height + '</a></td><td data-label="Amount" class="num">' + gk.gnr(p.value) + ' GNR</td><td data-label="Spendable">' + (a.height >= spend ? '<span class="ok">yes</span>' : "from block " + spend) + "</td></tr>";
      }).join("");
      view.innerHTML = '<h2 style="font-size:20px;margin:8px 0 12px">Address</h2>' + card(kv(rows)) +
        '<h2 style="font-size:20px;margin:22px 0 12px">Mining</h2>' + card(mining) +
        (payRows ? '<h2 style="font-size:20px;margin:22px 0 12px">Payouts</h2>' + card('<table class="data"><thead><tr><th>Block</th><th class="num">Amount</th><th>Spendable</th></tr></thead><tbody>' + payRows + "</tbody></table>") : "") +
        card('<div style="padding:0 18px 16px">' + priv + "</div>") + proofForm(id);
    }).catch(function (e) { err("Not a valid Genoryn Krypton address: " + e.message); });
  }

  window.exSearch = function (e) {
    e.preventDefault();
    var q = document.getElementById("q").value.trim().replace(/\s+/g, "");
    if (/^[a-z]*pay1/.test(q)) { location.hash = "#/proof"; setTimeout(function () { var f = document.getElementById("pf"); if (f) f.value = q; }, 50); return false; }
    if (/^[a-z]*gnrs?1[a-z2-7]+$/.test(q)) { location.hash = "#/address/" + q; return false; }
    if (/^\d+$/.test(q)) location.hash = "#/block/" + q;
    else if (/^[0-9a-fA-F]{64}$/.test(q)) {
      gk.api("/v1/block/" + q).then(function () { location.hash = "#/block/" + q; }).catch(function () {
        gk.api("/v1/tx/" + q).then(function () { location.hash = "#/tx/" + q; }).catch(function () { location.hash = "#/nullifier/" + q; });
      });
    } else err("Enter a block height, a 64-character hash or id, an address (gnr1… / gnrs1…) or a payment proof (gnrpay1…).");
    return false;
  };

  function route() {
    var p = location.hash.replace(/^#\/?/, "").split("/");
    if (p[0] === "block" && p[1]) block(p[1]);
    else if (p[0] === "tx" && p[1]) tx(p[1]);
    else if (p[0] === "nullifier" && p[1]) nullifier(p[1]);
    else if (p[0] === "address" && p[1]) address(decodeURIComponent(p[1]));
    else if (p[0] === "proof") { view.innerHTML = '<h2 style="font-size:20px;margin:8px 0 12px">Payment proof</h2>' + proofForm(); }
    else home();
    var isHome = !p[0];
    if (dashEl) dashEl.style.display = isHome ? "" : "none";
    if (!isHome) view.scrollIntoView({ block: "start" });
  }
  window.addEventListener("hashchange", route);
  stats(); route();
  setInterval(function () { if (!document.hidden) { stats(); if (!location.hash || location.hash === "#/") home(); } }, 20000);
  setInterval(function () { if (!document.hidden && dash) dash.reload(); }, 60000);
})();
