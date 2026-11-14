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
        "<td class=\"mk-px\"><div class=\"mk-pxw\"><span class=\"mk-live" + (off ? " off" : "") + "\" title=\"" + (off ? "Maker offline: it cannot be taken until the maker's wallet is back" : "Maker online: the swap starts within seconds") + "\"></span><div>" + cell(sat(p), usd(p)) + "</div></div></td>" +
        "<td class=\"num\">" + cell(gnr(o.gnr)) + "</td>" +
        "<td class=\"num\">" + cell(btc(o.sats) + " BTC", dollars(o.sats)) + "</td>" +
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
    var pb = data.pending.reduce(function (s, o) { return s + o.sats; }, 0);
    $("mkPendN").innerHTML = data.pending.length ? "<b>" + data.pending.length + "</b> running · " + btc(pb) + " BTC" + (dollars(pb) ? " · <b>≈ " + dollars(pb) + "</b>" : "") : "none right now";
    $("mkPend").innerHTML = data.pending.length
      ? rows(data.pending, "pend", function (o) { return left(Math.max(0, data.now - (o.since || data.now))) + " ago"; })
      : "<tr><td colspan=\"4\" class=\"muted mk-empty\">No swap running right now.</td></tr>";
    var all = data.recent, vol = all.reduce(function (a, t) { return a + Number(t.gnr || 0); }, 0), vb = all.reduce(function (a, t) { return a + Number(t.sats || 0); }, 0);
    $("mkAll").textContent = all.length;
    $("mkAllV").textContent = all.length ? gnr(vol) + " GNR" + (dollars(vb) ? " · ≈ " + dollars(vb) : "") : "";
    $("mkRecentN").textContent = all.length ? "· " + all.length : "";
    $("mkRecent").innerHTML = all.length ? all.map(card).join("") : "<p class=\"muted\">No completed swaps yet.</p>";
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
  function load() {
    Promise.all([
      gk.api("/v1/swap/offers").catch(function () { return { offers: [], now: Date.now() / 1000 }; }),
      gk.api("/v1/swap/market").catch(function () { return { market: {}, recent: [] }; }),
      gk.api("/v1/btc-price").catch(function () { return {}; }),
      gk.api("/v1/info").catch(function () { return null; })
    ]).then(function (r) {
      data.offers = r[0].offers || []; data.pending = r[0].in_progress || []; data.now = r[0].now || Date.now() / 1000;
      data.market = r[1].market || {}; data.recent = r[1].recent || [];
      data.btc = Number(r[2].USD) || 0;
      render();
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
  setInterval(function () { if (!document.hidden) load(); }, 30000);
})();
