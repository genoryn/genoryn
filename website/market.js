/* genoryn.io/market: the live offer board and market price, read from the node's /v1/swap API. */
(function () {
  var PER = 20, page = { sell: 0, buy: 0 }, data = { offers: [], pending: [], market: {}, recent: [], btc: 0, now: 0 };
  var $ = function (id) { return document.getElementById(id); };
  var sat = function (p) { return GKValue.btc(p); };
  var usd = function (p) { return data.btc > 0 && p != null ? GKValue.usd(p * data.btc / 1e8) : ""; };
  var gnr = function (v) { return (v / 1e8).toLocaleString("en-US", { maximumFractionDigits: 8 }); };
  var btc = function (v) { return (v / 1e8).toLocaleString("en-US", { minimumFractionDigits: 4, maximumFractionDigits: 8 }); };
  var ago = function (s) { return s >= 3600 ? Math.floor(s / 3600) + " h" : Math.max(1, Math.floor(s / 60)) + " min"; };
  var price = function (o) { return o.sats / (o.gnr / 1e8); };

  var dollars = function (sats) { return data.btc > 0 ? GKValue.usd(sats * data.btc / 1e8) : ""; };
  var left = function (s) {
    var d = s / 86400;
    return s < 3600 ? Math.max(1, Math.floor(s / 60)) + " min" : s < 172800 ? Math.floor(s / 3600) + " h" : d < 60 ? Math.floor(d) + " days" : d < 365 ? Math.floor(d / 30.44) + " months" : "1 year";
  };
  var cell = function (main, sub) { return "<b>" + main + "</b>" + (sub ? "<em>" + sub + "</em>" : ""); };

  function rows(list, kind, when) {
    var big = Math.max.apply(null, list.map(function (o) { return o.gnr; }).concat([1]));
    return list.map(function (o) {
      var p = price(o), off = o.online === false, w = Math.max(2, Math.round(o.gnr / big * 100));
      return "<tr class=\"" + (off ? "mk-off" : "") + "\" style=\"--depth:" + w + "%\">" +
        "<td class=\"mk-px\"><div class=\"mk-pxw\"><span class=\"mk-live" + (off ? " off" : "") + "\" title=\"" + (off ? "Maker offline: it cannot be taken until the maker's wallet is back" : "Maker online: the swap starts within seconds") + "\"></span><div>" + cell(sat(p) + (o.auto ? "<span class=\"mk-qtag\" title=\"Quick Swap: anyone can swap any part of it, in one click\">Quick</span>" : ""), usd(p)) + "</div></div></td>" +
        "<td class=\"num\">" + cell(gnr(o.gnr) + (o.backed ? "<i class=\"mk-ok\" title=\"The seller's GNR is set aside and proven to the board\">✓</i>" : ""), kind === "pend" ? (o.funded ? "bitcoin paid" : "taken, not paid yet") : "") + "</td>" +
        "<td class=\"num\">" + cell(btc(o.sats) + " BTC" + (o.deposited ? "<i class=\"mk-ok\" title=\"The buyer's bitcoin is deposited: the board found it on the Bitcoin chain, and the buyer's wallet pays the swap by itself\">✓</i>" : ""), dollars(o.sats)) + "</td>" +
        "<td class=\"num\">" + when(o) + "</td></tr>";
    }).join("");
  }

  function table(kind) {
    var side = kind === "sell" ? "sell_gnr" : "buy_gnr";
    // Online makers first (they can be taken now), best price first within each group.
    var list = data.offers.filter(function (o) { return o.side === side; });
    list.sort(function (a, b) {
      var oa = a.online === false ? 1 : 0, ob = b.online === false ? 1 : 0;
      return oa - ob || (kind === "sell" ? price(a) - price(b) : price(b) - price(a));
    });
    var pages = Math.max(1, Math.ceil(list.length / PER));
    page[kind] = Math.min(page[kind], pages - 1);
    var shown = list.slice(page[kind] * PER, (page[kind] + 1) * PER);
    var g = list.reduce(function (s, o) { return s + o.gnr; }, 0), b = list.reduce(function (s, o) { return s + o.sats; }, 0);
    var on = list.filter(function (o) { return o.online !== false; }).length;
    $(kind === "sell" ? "mkSellN" : "mkBuyN").innerHTML = list.length
      ? "<b>" + on + "</b> online" + (list.length > on ? " · " + (list.length - on) + " offline" : "") + "<br>" + gnr(g) + " GNR · " + btc(b) + " BTC" + (dollars(b) ? " · <b>≈ " + dollars(b) + "</b>" : "")
      : "none yet";
    $(kind === "sell" ? "mkSell" : "mkBuy").innerHTML = shown.length
      ? rows(shown, kind, function (o) { return left(Math.max(0, o.expires - data.now)); })
      : "<tr><td colspan=\"4\" class=\"muted mk-empty\">No " + kind + " offers yet. Publish one in the wallet's Swap tab.</td></tr>";
    var pg = $(kind === "sell" ? "mkSellP" : "mkBuyP");
    pg.innerHTML = pages > 1
      ? "<button class=\"btn\" data-mk=\"" + kind + "\" data-d=\"-1\"" + (page[kind] ? "" : " disabled") + ">‹</button><span>" + (page[kind] + 1) + " / " + pages + "</span><button class=\"btn\" data-mk=\"" + kind + "\" data-d=\"1\"" + (page[kind] < pages - 1 ? "" : " disabled") + ">›</button>"
      : "";
  }

  function render() {
    var m = data.market || {}, np = Number(m.priced) || 0;
    $("mkPrice").textContent = m.price != null ? sat(m.price) : "no swaps yet";
    $("mkPriceL").textContent = m.price != null ? (usd(m.price) ? "≈ " + usd(m.price) + " · " : "") + "average of " + np + " swap" + (np === 1 ? "" : "s") : "after the first completed swap";
    $("mkAsk").textContent = m.best_ask != null ? sat(m.best_ask) : "—"; $("mkAskU").textContent = m.best_ask != null ? usd(m.best_ask) : "";
    $("mkAskN").textContent = m.asks ? "(" + m.asks + ")" : "";
    $("mkBid").textContent = m.best_bid != null ? sat(m.best_bid) : "—"; $("mkBidU").textContent = m.best_bid != null ? usd(m.best_bid) : "";
    $("mkBidN").textContent = m.bids ? "(" + m.bids + ")" : "";
    $("mkVol").textContent = (m.trades || 0) + " swap" + (m.trades === 1 ? "" : "s");
    $("mkVolU").textContent = gnr(m.volume_gnr || 0) + " GNR · " + (m.pending || 0) + " running";
    table("sell"); table("buy");
    // A swap runs once its bitcoin is paid; an offer that was only taken may never be.
    var paid = data.pending.filter(function (o) { return o.funded; }), wait = data.pending.length - paid.length;
    var pb = paid.reduce(function (s, o) { return s + o.sats; }, 0);
    $("mkPendN").innerHTML = (paid.length ? "<b>" + paid.length + "</b> paid and running · " + btc(pb) + " BTC" + (dollars(pb) ? " · <b>≈ " + dollars(pb) + "</b>" : "") : "none paid right now") +
      (wait ? "<br>" + wait + " taken, not paid yet" : "");
    $("mkPend").innerHTML = data.pending.length
      ? rows(data.pending, "pend", function (o) { return left(Math.max(0, data.now - (o.since || data.now))) + " ago"; })
      : "<tr><td colspan=\"4\" class=\"muted mk-empty\">No swap running right now.</td></tr>";
    var all = data.recent, vol = all.reduce(function (a, t) { return a + Number(t.gnr || 0); }, 0), vb = all.reduce(function (a, t) { return a + Number(t.sats || 0); }, 0);
    $("mkAll").textContent = all.length;
    $("mkAllV").textContent = all.length ? gnr(vol) + " GNR" + (dollars(vb) ? " · ≈ " + dollars(vb) : "") : "";
    $("mkRecentN").textContent = all.length ? "· " + all.length : "";
    $("mkRecent").innerHTML = all.length ? all.map(card).join("") : "<p class=\"muted\">No completed swaps yet.</p>";
    history();
    totals();
  }
  // What the board holds, in bitcoin and in dollars.
  function totals() {
    if (!$("mktBtc")) return;
    var sum = function (l, k) { return l.reduce(function (a, o) { return a + Number(o[k] || 0); }, 0); };
    var put = function (id, l, word, none) {
      var s = sum(l, "sats"), g = sum(l, "gnr");
      $(id).textContent = l.length ? (dollars(s) || btc(s) + " BTC") : "—";
      $(id + "N").textContent = l.length ? gnr(g) + " GNR · " + btc(s) + " BTC · " + l.length + " " + word + (l.length === 1 ? "" : "s") : none;
    };
    $("mktBtc").textContent = data.btc > 0 ? GKValue.usd(data.btc) : "—";
    $("mktBtcN").textContent = data.btc > 0 ? "per bitcoin" : "the price could not be read";
    put("mktSell", data.offers.filter(function (o) { return o.side === "sell_gnr"; }), "sell offer", "no sell offers");
    put("mktBuy", data.offers.filter(function (o) { return o.side === "buy_gnr"; }), "buy offer", "no buy offers");
    var paidNow = data.pending.filter(function (o) { return o.funded; }), waiting = data.pending.length - paidNow.length;
    put("mktProg", paidNow, "swap", waiting ? "" : "none right now");
    if (waiting) $("mktProgN").textContent += (paidNow.length ? " · " : "") + waiting + " taken, not paid yet";
    put("mktDone", data.recent, "swap", "no swaps yet");
  }
  // Price history: one point per completed swap, oldest first.
  var charts = {};
  function draw(id, series, opts) {
    var host = $(id);
    if (!host || !window.GKCharts) return;
    if (charts[id]) charts[id].update(series, opts); else charts[id] = GKCharts.chart(host, series, opts);
  }
  function history() {
    if (!window.GKCharts || !$("mkChartPrice")) return;
    var at = function (x) { return (x.claim && x.claim.time) || x.at; };
    var all = data.recent.slice().sort(function (a, b) { return at(a) - at(b); });
    var pick = function (f) { return all.map(function (x) { return { t: at(x), v: f(x), h: x.claim ? x.claim.height : null }; }); };
    var satAxis = function (v, axis) { return axis ? GKCharts.fmtNum(v) + " sat" : sat(v) + (usd(v) ? " · ≈ " + usd(v) : ""); };
    draw("mkChartPrice", pick(function (x) { return Number(x.price) || 0; }), { kind: "area", straight: true, points: true, fmt: satAxis, label: "Price per GNR of every completed swap", height: 230 });
    draw("mkChartVol", pick(function (x) { return Number(x.gnr || 0) / 1e8; }), { kind: "bars", fmt: function (v, axis) { return axis ? GKCharts.fmtNum(v) : gnr(v * 1e8) + " GNR"; }, label: "GNR traded in each swap", height: 230 });
    draw("mkChartBtc", pick(function (x) { return Number(x.sats || 0); }), { kind: "bars", fmt: function (v, axis) { return axis ? GKCharts.fmtNum(v) + " sat" : btc(v) + " BTC" + (dollars(v) ? " · ≈ " + dollars(v) : ""); }, label: "Bitcoin paid in each swap", height: 200 });
    var last = all[all.length - 1];
    $("mkHistLast").textContent = last ? "last " + sat(last.price) : "no swaps yet";
    $("mkHistVol").textContent = all.length ? gnr(all.reduce(function (a, x) { return a + Number(x.gnr || 0); }, 0)) + " GNR" : "—";
    $("mkHistBtc").textContent = all.length ? btc(all.reduce(function (a, x) { return a + Number(x.sats || 0); }, 0)) + " BTC" : "—";
  }
  function hashrate() {
    if (!window.GKCharts || !$("mkChartRate")) return;
    gk.api("/v1/stats?range=7d").then(function (s) {
      draw("mkChartRate", (s.points || []).map(function (p) { return { t: p.t, v: p.hashrate, h: p.h }; }), { kind: "area", fmt: GKCharts.fmtRate, label: "Network hashrate, last 7 days", height: 200 });
      $("mkHistRate").textContent = gk.rate((s.summary || {}).hashrate);
    }).catch(function () {});
  }
  var esc = function (s) { return gk.esc(s); };
  var short = function (h) { return h ? h.slice(0, 10) + "…" + h.slice(-8) : "—"; };
  var when = function (t) { return t ? new Date(t * 1000).toLocaleString("en-GB", { day: "numeric", month: "short", year: "numeric", hour: "2-digit", minute: "2-digit", timeZoneName: "short" }) : "—"; };
  function card(t) {
    var c = t.claim, conf = Number(t.confirmations || 0);
    var hash = function (label, v) {
      return "<div class=\"mk-sw-h\"><span>" + label + "</span><code title=\"" + esc(v) + "\">" + esc(short(v)) + "</code>" +
        "<button type=\"button\" class=\"mk-copy\" data-copy=\"" + esc(v) + "\" aria-label=\"Copy " + label + "\">Copy</button></div>";
    };
    var step = function (on, label, sub) {
      return "<li class=\"" + (on ? "on" : "") + "\"><i aria-hidden=\"true\"></i><b>" + label + "</b><span>" + sub + "</span></li>";
    };
    return "<article class=\"glass mk-sw\">" +
      "<header class=\"mk-sw-top\">" +
        "<div class=\"mk-sw-amt\"><span class=\"mk-sw-coin\">" + GKValue.coin("gnr") + "<b>" + gnr(t.gnr) + "</b><em>GNR</em></span>" +
        "<span class=\"mk-sw-arrow\" aria-hidden=\"true\">⇄</span>" +
        "<span class=\"mk-sw-coin\">" + GKValue.coin("btc") + "<b>" + btc(t.sats) + "</b><em>BTC" + (dollars(t.sats) ? " · ≈ " + dollars(t.sats) : "") + "</em></span></div>" +
        "<span class=\"mk-sw-badge" + (c ? "" : " wait") + "\">" + (c ? "Completed" : "Reported") + "</span>" +
      "</header>" +
      "<div class=\"mk-sw-price\"><b>" + sat(t.price) + "</b> per GNR" + (usd(t.price) ? " <span>≈ " + usd(t.price) + "</span>" : "") +
        "<span class=\"mk-sw-when\">" + (c ? when(c.time) + " · " + ago(Math.max(0, data.now - c.time)) + " ago" : ago(Math.max(0, data.now - t.at)) + " ago") + "</span></div>" +
      "<ol class=\"mk-sw-steps\">" +
        step(true, "GNR locked", "for the buyer only") +
        step(!!c, "Bitcoin paid", "into the bitcoin contract") +
        step(!!c, "Secret revealed", "the seller took the bitcoin") +
        step(!!c, "GNR claimed", c ? "block #" + Number(c.height).toLocaleString("en-US") : "waiting") +
      "</ol>" +
      (c ? "<div class=\"mk-sw-links\"><a href=\"/explorer#/tx/" + esc(c.txid) + "\">GNR claim " + esc(short(c.txid)) + " ↗</a>" +
        "<a href=\"/explorer#/block/" + Number(c.height) + "\">Block #" + Number(c.height).toLocaleString("en-US") + " · " + conf.toLocaleString("en-US") + " confirmations</a></div>" : "") +
      "<details class=\"mk-sw-more\"><summary>What the blockchain proves</summary>" +
        hash("Hash lock (SHA-256)", t.hash) +
        (c ? hash("Secret revealed", c.secret) : "") +
        (c ? "<p>The secret hashes to the lock, and the same secret unlocked the bitcoin side. The GNR could only have been refunded to the seller from block #" + Number(c.refund_height).toLocaleString("en-US") + "; it was claimed before that.</p>" : "<p>Counted once its claim is on chain.</p>") +
      "</details></article>";
  }
  // Quick Swap: what the board can swap right now, and what an amount would get. The plan is
  // the wallet's own (best price first, whole offers where they fit, at most three parts).
  var qx = { side: "buy", unit: "usd" };
  var MIN_PART_SATS = 10000, MIN_PART_GNR = 3 * 2560;
  function quickOffers(buying) {
    return data.offers.filter(function (o) {
      return o.auto && o.online !== false && o.side === (buying ? "sell_gnr" : "buy_gnr") && (buying ? o.backed : o.deposited);
    });
  }
  // Bitcoin for g GNR of an offer at its price, rounded for its maker (BigInt: no overflow).
  function partSats(o, g) {
    if (g <= 0 || g > o.gnr) return null;
    if (g === o.gnr) return o.sats;
    var n = BigInt(Math.round(g)) * BigInt(o.sats), d = BigInt(o.gnr);
    return Number(o.side === "sell_gnr" ? (n + d - 1n) / d : n / d);
  }
  function plan(buying, want, inSats) {
    var v = quickOffers(buying).slice().sort(function (a, b) {
      return (buying ? price(a) - price(b) : price(b) - price(a)) || b.gnr - a.gnr;
    });
    var left = want, parts = [];
    for (var i = 0; i < v.length && parts.length < 3 && left > 0; i++) {
      var o = v[i], g;
      if (!inSats) g = Math.min(left, o.gnr);
      else if (left >= o.sats) g = o.gnr;
      else { g = Math.floor(left * o.gnr / o.sats); while (g > 0 && partSats(o, g) > left) g--; }
      var s = partSats(o, g);
      if (s == null) continue;
      var whole = g === o.gnr;
      if (!whole && (g < Math.max((o.auto && o.auto.min) || 0, MIN_PART_GNR) || s < MIN_PART_SATS)) continue;
      left -= inSats ? Math.min(s, left) : g;
      parts.push({ gnr: g, sats: s });
    }
    return parts;
  }
  function quick() {
    if (!$("qxSell")) return;
    var k = data.quick || null;
    var s = (k && k.sell) || {}, b = (k && k.buy) || {}, n = (s.offers || 0) + (b.offers || 0);
    $("qxLiveBox").classList.toggle("off", !n);
    $("qxLive").textContent = !k ? "Quick Swap starts with wallet 1.0.46" : n ? n + " offer" + (n === 1 ? "" : "s") + " online, updated " + new Date().toLocaleTimeString("en-GB", { hour: "2-digit", minute: "2-digit", second: "2-digit" }) : "No Quick Swap offer online right now";
    tween($("qxSell"), s.offers ? gnr(s.gnr) + " GNR" : "—");
    $("qxSellN").textContent = s.offers ? "from " + sat(s.best) + (usd(s.best) ? " · " + usd(s.best) : "") + " per GNR · " + s.offers + " offer" + (s.offers === 1 ? "" : "s") : "no sell offer online";
    tween($("qxBuy"), b.offers ? (dollars(b.sats) || btc(b.sats) + " BTC") : "—");
    $("qxBuyN").textContent = b.offers ? btc(b.sats) + " BTC · up to " + sat(b.best) + " per GNR · " + b.offers + " offer" + (b.offers === 1 ? "" : "s") : "no buy offer online";
    tween($("qxRun"), k ? String(Number(k.parts_in_progress || 0)) : "—");
    qcalc();
  }
  // Numbers that change glow once.
  function tween(el, text) {
    if (el.textContent === text) return;
    el.textContent = text;
    el.classList.remove("qx-bump"); void el.offsetWidth; el.classList.add("qx-bump");
  }
  function qcalc() {
    if (!$("qxAmt")) return;
    var buy = qx.side === "buy", raw = Number(String($("qxAmt").value).replace(/,/g, ""));
    var gUnit = qx.unit === "gnr";
    $("qxInL").textContent = gUnit ? (buy ? "You get" : "You sell") : (buy ? "You pay" : "You receive");
    $("qxOutL").textContent = gUnit ? (buy ? "You would pay about" : "You would receive about") : (buy ? "You would get about" : "You would sell about");
    $("qxAmt").placeholder = qx.unit === "usd" ? "100" : qx.unit === "btc" ? "0.001" : "100";
    if (!(raw > 0)) { $("qxOut").textContent = "—"; $("qxOutN").textContent = "Type an amount to see what the board gives for it now."; return; }
    var want = gUnit ? Math.round(raw * 1e8) : qx.unit === "btc" ? Math.round(raw * 1e8) : data.btc > 0 ? Math.round(raw / data.btc * 1e8) : 0;
    if (!want) { $("qxOut").textContent = "—"; $("qxOutN").textContent = "The bitcoin price could not be read; use BTC or GNR."; return; }
    var p = plan(buy, want, !gUnit);
    var g = p.reduce(function (a, x) { return a + x.gnr; }, 0), s = p.reduce(function (a, x) { return a + x.sats; }, 0);
    if (!p.length) { $("qxOut").textContent = "—"; $("qxOutN").textContent = quickOffers(buy).length ? "That is less than the smallest part of the offers online (at least 10,000 sat)." : "No Quick Swap offer is online on this side right now."; return; }
    $("qxOut").textContent = gUnit ? btc(s) + " BTC" : gnr(g) + " GNR";
    var short = gUnit ? g < want : s + MIN_PART_SATS < want;
    $("qxOutN").textContent = sat(s / (g / 1e8)) + " per GNR on average" + (dollars(s) ? " · ≈ " + dollars(s) : "") + " · " + p.length + " part" + (p.length === 1 ? "" : "s") +
      (short ? " · the board has less than that right now" : "") + ". Plus the bitcoin network fee.";
  }
  document.addEventListener("click", function (e) {
    var t = e.target.closest ? e.target.closest("#qxSide button, #qxUnit button") : null;
    if (!t) return;
    if (t.dataset.s) { qx.side = t.dataset.s; document.querySelectorAll("#qxSide button").forEach(function (x) { x.classList.toggle("on", x === t); }); }
    if (t.dataset.u) { qx.unit = t.dataset.u; document.querySelectorAll("#qxUnit button").forEach(function (x) { x.classList.toggle("on", x === t); }); }
    qcalc();
  });
  if ($("qxAmt")) $("qxAmt").addEventListener("input", qcalc);
  function load() {
    Promise.all([
      gk.api("/v1/swap/offers").catch(function () { return { offers: [], now: Date.now() / 1000 }; }),
      gk.api("/v1/swap/market").catch(function () { return { market: {}, recent: [] }; }),
      gk.api("/v1/btc-price").catch(function () { return {}; }),
      gk.api("/v1/info").catch(function () { return null; })
    ]).then(function (r) {
      data.offers = r[0].offers || []; data.pending = r[0].in_progress || []; data.now = r[0].now || Date.now() / 1000;
      data.market = r[1].market || {}; data.recent = r[1].recent || []; data.quick = r[1].quick || null;
      data.btc = Number(r[2].USD) || 0;
      render();
      quick();
    });
  }
  document.addEventListener("click", function (e) {
    var cp = e.target.closest ? e.target.closest("[data-copy]") : null;
    if (cp) {
      navigator.clipboard && navigator.clipboard.writeText(cp.getAttribute("data-copy")).then(function () {
        cp.textContent = "Copied"; setTimeout(function () { cp.textContent = "Copy"; }, 1400);
      });
      return;
    }
    var b = e.target.closest ? e.target.closest("[data-mk]") : null;
    if (!b) return;
    page[b.getAttribute("data-mk")] += Number(b.getAttribute("data-d"));
    render();
  });
  load();
  hashrate();
  setInterval(function () { if (!document.hidden) load(); }, 15000);
  setInterval(function () { if (!document.hidden) hashrate(); }, 300000);
})();
