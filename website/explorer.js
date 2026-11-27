/* Explorer views on the public node API (/v1). Hash routes: #/ , #/block/<id>, #/tx/<id>, #/nullifier/<hex> */
(function () {
  var view = document.getElementById("view");
  function card(html) { return '<div class="glass table-card">' + html + "</div>"; }
  /* Node data only ever reaches HTML through these: hashes must be hex, numbers are numbers. */
  function hx(h) { return typeof h === "string" && /^[0-9a-f]{1,128}$/i.test(h) ? h : ""; }
  function nm(v) { var n = Number(v); return isFinite(n) ? n : 0; }
  function short(h) { h = hx(h); return h ? h.slice(0, 10) + "…" + h.slice(-8) : ""; }
  function kv(rows) { return '<div class="kv">' + rows.map(function (r) { return "<div>" + r[0] + "</div><div>" + r[1] + "</div>"; }).join("") + "</div>"; }
  function err(m) { view.innerHTML = card('<p style="padding:18px" class="err-site">' + gk.esc(m) + "</p>"); }
  function h2(text, first) { return '<h2 style="font-size:20px;margin:' + (first ? "8px" : "22px") + ' 0 12px">' + gk.esc(text) + "</h2>"; }
  function kb(bytes) { return T("{n} KB", { n: GKI18N.num(nm(bytes) / 1024, { minimumFractionDigits: 1, maximumFractionDigits: 1 }) }); }
  function utc(t) { return T("{time} UTC", { time: new Date(nm(t) * 1000).toISOString().replace("T", " ").slice(0, 19) }); }
  /* Table labels (also shown on phones through data-label). */
  var L = {
    height: T("Height"), hash: T("Hash"), age: T("Age"), shares: T("Shares"), txs: T("Transactions"), reward: T("Reward"), size: T("Size"),
    txid: T("Txid"), kind: T("Kind"), fee: T("Fee"), proof: T("Proof"), share: T("Share"), minedOn: T("Mined on block"), miner: T("Miner"),
    paid: T("Paid"), block: T("Block"), amount: T("Amount"), spendable: T("Spendable"), addresses: T("Addresses")
  };
  Object.keys(L).forEach(function (k) { L[k] = gk.esc(L[k]); });

  function stats() {
    gk.api("/v1/info").then(function (i) {
      document.getElementById("sHeight").textContent = GKI18N.num(i.height);
      document.getElementById("sSupply").textContent = gk.gnr(i.supply, 0);
      document.getElementById("sRate").textContent = gk.rate(i.hashrate);
      document.getElementById("sPool").textContent = i.mempool;
      gk.difficulty(i);
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
      kpi("block_time", T("{n} s avg", { n: avg("block_time").toFixed(0) }));
      kpi("shares", T("{n} avg", { n: GKI18N.num(avg("shares"), { minimumFractionDigits: 1, maximumFractionDigits: 1 }) }));
      kpi("issued", gk.gnr(s.issued, 0) + " GNR");
      kpi("issued2", gk.gnr(s.issued, 0) + " GNR");
      kpi("remaining", gk.gnr(s.remaining, 0) + " GNR");
      GKCharts.disc(document.getElementById("supplyDisc"), Number(s.issued || 0), Number(s.max_supply || 21e14));
    });
    GKCharts.tilt(".tilt-card");
  }

  // How many different addresses each recent block credits, by height (from the last
  // hour's statistics, asked for at most once a minute).
  var credited = { at: 0, by: {} };
  function creditedBy() {
    if (Date.now() - credited.at < 55000) return Promise.resolve(credited.by);
    return gk.api("/v1/stats?range=1h").then(function (s) {
      var by = {};
      gk.perBlock(s).forEach(function (b) { by[b.h] = b.miners; });
      credited = { at: Date.now(), by: by };
      return by;
    }).catch(function () { return credited.by; });
  }

  function home() {
    Promise.all([gk.api("/v1/blocks?count=25"), creditedBy()]).then(function (got) {
      var bs = got[0], by = got[1] || {};
      var rows = bs.map(function (b) {
        var who = by[nm(b.height)];
        return '<tr><td data-label="' + L.height + '"><a href="#/block/' + nm(b.height) + '">' + nm(b.height) + '</a></td><td data-label="' + L.hash + '" class="mono"><a href="#/block/' + hx(b.hash) + '">' + short(b.hash) +
          '</a></td><td data-label="' + L.age + '">' + gk.ago(b.timestamp) + '</td><td data-label="' + L.shares + '" class="num">' + nm(b.shares) + '</td><td data-label="' + L.addresses + '" class="num">' + (who != null ? nm(who) : "—") + '</td><td data-label="' + L.txs + '" class="num">' + nm(b.txs) +
          '</td><td data-label="' + L.reward + '" class="num">' + gk.gnr((b.subsidy || 0) + (b.fees || 0)) +
          '</td><td data-label="' + L.size + '" class="num">' + kb(b.size) + "</td></tr>";
      }).join("");
      view.innerHTML = h2(T("Latest blocks"), true) + card('<table class="data"><thead><tr><th>' + L.height + "</th><th>" + L.hash + "</th><th>" + L.age + '</th><th class="num">' + L.shares + '</th><th class="num" title="' + gk.esc(T("Different addresses the block credits: the one that found it and those of its shares")) + '">' + L.addresses + '</th><th class="num">' + gk.esc(T("Txs")) + '</th><th class="num">' + gk.esc(T("Reward GNR")) + '</th><th class="num">' + L.size + "</th></tr></thead><tbody>" + rows + "</tbody></table>") +
        '<p class="muted" style="margin-top:12px;font-size:13.5px">' + gk.esc(T("Each block adds its reward to a shared pool; every share credits its miner an equal slice, so miners earn in proportion to their hashrate.")) + " " +
        gk.esc(T("Addresses is how many different payout addresses a block credits; the average of the last hour is on the Mining page.")) + "</p>";
    }).catch(function () { err(T("The node API is not reachable right now.")); });
  }

  function block(id) {
    gk.api("/v1/block/" + encodeURIComponent(id)).then(function (b) {
      var h = b.header;
      // Krypton coins: issues, burns, deploys and pauses are public; transfers stay private
      // (a pausable coin's transfer shows only which coin moved).
      var coinKind = function (c) {
        if (!c) return gk.esc(T("private transfer"));
        var who = c.ticker ? gk.esc(c.ticker) : c.coin ? '<span class="mono">' + gk.esc(String(c.coin).slice(0, 8)) + "…</span>" : "";
        if (c.op === "transfer") return who ? gk.esc(T("coin transfer")) + " · " + who : gk.esc(T("private transfer"));
        if (c.op === "issue") return gk.esc(T("coin issue")) + " · " + who + " · " + gk.esc(T("{n} units", { n: GKI18N.num(nm(c.issue)) }));
        if (c.op === "burn") return gk.esc(T("coin burn")) + " · " + who + " · " + gk.esc(T("{n} units", { n: GKI18N.num(nm(c.burn)) }));
        return gk.esc(T("coin {op}", { op: c.op })) + (who ? " · " + who : "");
      };
      var txs = b.txs.map(function (t) {
        return '<tr><td data-label="' + L.txid + '" class="mono"><a href="#/tx/' + hx(t.txid) + '">' + short(t.txid) + '</a></td><td data-label="' + L.kind + '">' + coinKind(t.coin) + '</td><td data-label="' + L.fee + '" class="num">' + gk.gnr(t.fee) +
          '</td><td data-label="' + L.proof + '" class="num">' + kb(t.proof_bytes) + "</td></tr>";
      }).join("") || '<tr><td colspan="4" class="muted">' + gk.esc(T("No transactions")) + "</td></tr>";
      var shares = (b.shares || []).map(function (s) {
        return '<tr><td data-label="' + L.share + '" class="mono">' + short(s.hash) + '</td><td data-label="' + L.minedOn + '" class="num">' + nm(s.height) + '</td><td data-label="' + L.miner + '" class="mono">' + short(s.miner) + "</td></tr>";
      }).join("") || '<tr><td colspan="3" class="muted">' + gk.esc(T("Only the block itself was credited")) + "</td></tr>";
      var payouts = (b.payouts || []).map(function (p) {
        return '<tr><td data-label="' + L.miner + '" class="mono">' + short(p.owner) + '</td><td data-label="' + L.paid + '" class="num">' + gk.gnr(p.value) + " GNR</td></tr>";
      }).join("");
      var e = gk.esc;
      // The different addresses this block credits: the one that found it and its shares'.
      var owners = {};
      if (nm(h.height) > 0) { owners[hx(b.miner)] = 1; (b.shares || []).forEach(function (s) { owners[hx(s.miner)] = 1; }); }
      view.innerHTML = h2(T("Block {h}", { h: nm(h.height) }), true) + card(kv([
        [L.hash, '<span class="mono">' + hx(b.hash) + "</span>"], [e(T("Previous")), '<a class="mono" href="#/block/' + hx(h.prev) + '">' + hx(h.prev) + "</a>"],
        [e(T("Time")), e(utc(h.timestamp))], [e(T("Confirmations")), nm(b.confirmations)],
        [e(T("Difficulty")), GKI18N.num(h.difficulty)], [e(T("Nonce")), gk.esc(h.nonce)],
        [e(T("Reward to the pool")), gk.gnr((b.subsidy || 0) + (b.fees || 0)) + " GNR"], [e(T("Found by")), '<span class="mono">' + short(b.miner) + "</span>"],
        [e(T("Shares credited")), (b.shares || []).length + 1], [e(T("Addresses credited")), Object.keys(owners).length], [e(T("Payouts made")), (b.payouts || []).length],
        [e(T("Note tree root")), '<span class="mono">' + hx(h.note_root) + "</span>"], [e(T("MMR root (light clients)")), '<span class="mono">' + hx(h.mmr_root) + "</span>"], [L.size, e(kb(b.size))]
      ])) +
        (payouts ? h2(T("Payouts")) + card('<table class="data"><thead><tr><th>' + L.miner + '</th><th class="num">' + L.paid + "</th></tr></thead><tbody>" + payouts + "</tbody></table>") : "") +
        h2(T("Shares")) + card('<table class="data"><thead><tr><th>' + L.share + '</th><th class="num">' + L.minedOn + "</th><th>" + L.miner + "</th></tr></thead><tbody>" + shares + "</tbody></table>") +
        h2(T("Transactions")) + card('<table class="data"><thead><tr><th>' + L.txid + "</th><th>" + L.kind + '</th><th class="num">' + L.fee + '</th><th class="num">' + L.proof + "</th></tr></thead><tbody>" + txs + "</tbody></table>");
    }).catch(function () { err(T("Block not found.")); });
  }

  function tx(id) {
    gk.api("/v1/tx/" + encodeURIComponent(id)).then(function (t) {
      var e = gk.esc;
      var rows = [[e(T("Transaction")), '<span class="mono">' + hx(t.txid) + "</span>"], [e(T("Status")), t.status === "confirmed" ? '<span class="ok">' + e(T("confirmed")) + "</span>" : '<span class="warn">' + e(T("pending")) + "</span>"]];
      if (t.height != null) rows.push([L.block, '<a href="#/block/' + nm(t.height) + '">' + nm(t.height) + "</a>"], [e(T("Confirmations")), nm(t.confirmations)]);
      if (t.fee != null) rows.push([L.fee, gk.gnr(t.fee) + " GNR"]);
      rows.push([e(T("Privacy")), e(T("Sender, recipient and amount are hidden by a zk-STARK proof."))]);
      view.innerHTML = h2(T("Transaction"), true) + card(kv(rows));
    }).catch(function () { err(T("Transaction not found.")); });
  }

  function nullifier(id) {
    gk.api("/v1/nullifier/" + encodeURIComponent(id)).then(function (n) {
      var e = gk.esc, at = nm(n.spent_height);
      view.innerHTML = h2(T("Nullifier"), true) + card(kv([[e(T("Nullifier")), '<span class="mono">' + gk.esc(id) + "</span>"],
        [e(T("Spent")), n.spent_height != null ? '<span class="ok">' + e(T("yes")) + "</span>, " + e(T("in block {h}", { h: at })).replace(String(at), '<a href="#/block/' + at + '">' + at + "</a>") : e(T("not spent"))]]));
    }).catch(function () { err(T("Invalid nullifier.")); });
  }

  function post(p, body) {
    return fetch(p, { method: "POST", headers: { "content-type": "application/json", accept: "application/json" }, body: JSON.stringify(body) })
      .then(function (r) {
        // An answer that is not the node's (a gateway's error page) is said in words.
        return r.json().catch(function () { return null; }).then(function (j) {
          if (j == null) throw new Error(T("the node did not answer (HTTP {status}); try again in a moment", { status: r.status }));
          if (!r.ok) throw new Error(j.error || r.status);
          return j;
        });
      });
  }

  function proofForm(prefillAddress) {
    var e = gk.esc;
    return card('<div style="padding:18px"><h3 class="xh">' + e(T("Verify a payment proof")) + "</h3>" +
      '<p class="muted" style="margin:6px 0 12px">' + e(T("Senders create a proof in their wallet (History → Proof). It shows one payment and nothing else, checked against this chain.")) + "</p>" +
      '<label for="pf" class="pf-l">' + e(T("Payment proof")) + '</label><textarea id="pf" rows="3" placeholder="gnrpay1…" spellcheck="false"></textarea>' +
      '<label for="pfa" class="pf-l">' + e(T("Recipient full address (optional, confirms the recipient)")) + '</label><input id="pfa" placeholder="gnr1…" value="' + gk.esc(prefillAddress || "") + '" spellcheck="false">' +
      '<button class="btn btn-primary" type="button" data-act="proof" style="margin-top:12px">' + e(T("Verify")) + '</button><div id="pfr" style="margin-top:14px"></div>' +
      '<p class="muted pf-note">' + e(T("A proof is not on the blockchain. The sender's wallet makes it, and it shows this one payment to whoever has the text, so give it only to those who should see it. This page sends it to the node to be checked; it is not kept and not published.")) + "</p></div>");
  }

  window.exProof = function () {
    var out = document.getElementById("pfr"), e = gk.esc;
    out.innerHTML = '<p class="muted">' + e(T("Checking…")) + "</p>";
    post("/v1/verify-payment", { proof: document.getElementById("pf").value.trim(), address: document.getElementById("pfa").value.trim() || null }).then(function (r) {
      var txid = /^[0-9a-f]{64}$/.test(r.txid) ? r.txid : "", h = Number(r.height) || 0;
      var say = function (cls, title, text) { return '<p class="' + cls + '"><b>' + e(title) + "</b> " + e(text) + "</p>"; };
      var head = r.pending
        ? say("warn", T("Payment pending."), T("The payment was sent and is waiting in the mempool, but it is not in a block yet. Check again after the next block (about a minute): only a confirmed payment is final."))
        : r.recipient_confirmed === false
        ? say("bad", T("Not a payment to that address."), T("The transaction exists, but it did not pay the address you entered."))
        : r.recipient_verified
          ? say("ok", T("Valid payment proof."), T("The payment below went to the recipient shown."))
          : say("warn", T("Real payment, recipient not verified."), T("The amount below was paid on chain, but the recipient short address was written by the sender and nothing confirms it. Paste the recipient's full address above, or ask them to publish their short address, then check again."));
      out.innerHTML = head + kv([
        [L.amount, "<b>" + gk.gnr(r.value) + " GNR</b>"],
        [e(T("Recipient")), '<span class="mono">' + gk.esc(r.recipient) + "</span>" + (r.recipient_confirmed === false ? ' <span class="bad">· ' + e(T("does not match")) + "</span>" : r.recipient_verified ? ' <span class="ok">· ' + e(T("verified")) + "</span>" : ' <span class="warn">· ' + e(T("unverified")) + "</span>")],
        [e(T("Memo")), gk.esc(r.memo) || "—"], [e(T("Transaction")), '<a class="mono" href="#/tx/' + txid + '">' + short(txid) + "</a>"],
        r.pending ? [L.block, '<span class="warn">' + e(T("pending · waiting in the mempool")) + "</span>"] : [L.block, '<a href="#/block/' + h + '">' + h + "</a> · " + e(Tn(Number(r.confirmations) || 0, "{n} confirmation", "{n} confirmations"))],
        [e(T("Time")), r.pending ? e(T("not mined yet")) : e(utc(r.time))]
      ]);
    }).catch(function (x) { out.innerHTML = '<p class="bad"><b>' + e(T("Not valid:")) + "</b> " + gk.esc(x.message) + "</p>"; });
  };

  function address(id) {
    view.innerHTML = '<p class="muted">Looking up…</p>';
    gk.api("/v1/address-info/" + encodeURIComponent(id)).then(function (a) {
      var e = gk.esc;
      var rows = [[e(T("Status")), '<span class="ok"><b>' + e(T("Valid {network} address", { network: a.network })) + "</b></span>"], [e(T("Short address")), '<span class="mono wrap-any">' + gk.esc(a.short) + "</span>"]];
      var priv = '<p class="muted" style="margin:14px 0 0">' + e(T("Transfers are shielded: incoming and outgoing payments never appear in any explorer, for this or any other address. To show that a payment really happened, ask the sender for a payment proof and check it below.")) + "</p>";
      if (a.kind === "short") {
        rows.push([e(T("Full address")), a.published ? e(T("published")) : '<span class="warn">' + e(T("not published on this node")) + "</span>"]);
        view.innerHTML = h2(T("Address"), true) + card('<div style="padding:4px 0">' + kv(rows) + '</div><div style="padding:0 18px 18px">' +
          '<p class="muted">' + e(T("The short form is correct, but its owner has not published the full address here, so it cannot be resolved. In the wallet: Wallet → Receive → Publish short form, or search with the full address.")) + "</p>" + priv + "</div>") + proofForm();
        return;
      }
      var m = a.mining || {}, act = m.activity || {}, pays = act.payouts || [];
      rows.push([e(T("Published")), a.published ? e(T("yes, the short form resolves on this node")) : e(T("no (Wallet → Receive → Publish short form)"))], [e(T("Owner tag")), '<span class="mono wrap-any">' + gk.esc(a.owner) + "</span>"], [e(T("Cryptography")), gk.esc(a.crypto)]);
      var paid = pays.reduce(function (s, p) { return s + Number(p.value); }, 0);
      var nph = Number(m.next_payout_height) || 0;
      var eta = nph ? e(T("block {h} (in ~{n} min)", { h: nph, n: Math.max(0, nph - Number(a.height)) })) : "—";
      var blockLink = function (h) { return '<a href="#/block/' + h + '">' + e(T("block {h}", { h: h })) + "</a>"; };
      var mining = kv([
        [e(T("Rewards accruing")), "<b>" + gk.gnr(m.unpaid) + " GNR</b>"], [e(T("Next payout")), eta],
        [e(T("Paid so far")), e(Tn(pays.length, "{gnr} GNR in {n} payout", "{gnr} GNR in {n} payouts", { gnr: gk.gnr(paid) }))],
        [e(T("Blocks found")), String(Number(act.blocks_found) || 0)], [e(T("Blocks with its shares")), String(Number(act.blocks_with_shares) || 0)],
        [e(T("Mining since")), act.first_seen ? blockLink(Number(act.first_seen)) : "—"], [e(T("Last share")), act.last_seen ? blockLink(Number(act.last_seen)) : "—"]
      ]);
      var payRows = pays.slice().reverse().map(function (p) {
        var ph = Number(p.height), spend = ph + (Number(m.maturity) || 100);
        return '<tr><td data-label="' + L.block + '"><a href="#/block/' + ph + '">' + ph + '</a></td><td data-label="' + L.amount + '" class="num">' + gk.gnr(p.value) + ' GNR</td><td data-label="' + L.spendable + '">' + (Number(a.height) >= spend ? '<span class="ok">' + e(T("yes")) + "</span>" : e(T("from block {h}", { h: spend }))) + "</td></tr>";
      }).join("");
      view.innerHTML = h2(T("Address"), true) + card(kv(rows)) +
        h2(T("Mining")) + card(mining) +
        (payRows ? h2(T("Payouts")) + card('<table class="data"><thead><tr><th>' + L.block + '</th><th class="num">' + L.amount + "</th><th>" + L.spendable + "</th></tr></thead><tbody>" + payRows + "</tbody></table>") : "") +
        card('<div style="padding:0 18px 16px">' + priv + "</div>") + proofForm(id);
    }).catch(function (x) { err(T("Not a valid Genoryn Krypton address: {why}", { why: x.message })); });
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
    } else err(T("Enter a block height, a 64-character hash or id, an address (gnr1… / gnrs1…) or a payment proof (gnrpay1…)."));
    return false;
  };

  function route() {
    var p = location.hash.replace(/^#\/?/, "").split("/");
    if (p[0] === "block" && p[1]) block(p[1]);
    else if (p[0] === "tx" && p[1]) tx(p[1]);
    else if (p[0] === "nullifier" && p[1]) nullifier(p[1]);
    else if (p[0] === "address" && p[1]) { var a = null; try { a = decodeURIComponent(p[1]); } catch (e) {} if (a) address(a); else err(T("Not a valid address.")); }
    else if (p[0] === "proof") { view.innerHTML = h2(T("Payment proof"), true) + proofForm(); }
    else home();
    var isHome = !p[0];
    if (dashEl) dashEl.style.display = isHome ? "" : "none";
    if (!isHome) view.scrollIntoView({ block: "start" });
  }
  window.addEventListener("hashchange", route);
  stats(); route();
  // Searches from the home page arrive as /explorer?q=…
  var q0 = new URLSearchParams(location.search).get("q");
  if (q0 && document.getElementById("q")) {
    document.getElementById("q").value = q0.slice(0, 2000);
    history.replaceState(null, "", location.pathname + location.hash);
    window.exSearch({ preventDefault: function () {} });
  }
  setInterval(function () { if (!document.hidden) { stats(); if (!location.hash || location.hash === "#/") home(); } }, 20000);
  setInterval(function () { if (!document.hidden && dash) dash.reload(); }, 60000);
})();
