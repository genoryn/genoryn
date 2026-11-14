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

  function table(kind) {
    var side = kind === "sell" ? "sell_gnr" : "buy_gnr";
    // Only offers whose maker is online: the others cannot be taken now.
    var list = data.offers.filter(function (o) { return o.side === side && o.online !== false; });
    list.sort(function (a, b) { return kind === "sell" ? price(a) - price(b) : price(b) - price(a); });
    var pages = Math.max(1, Math.ceil(list.length / PER));
    page[kind] = Math.min(page[kind], pages - 1);
    var rows = list.slice(page[kind] * PER, (page[kind] + 1) * PER);
    $(kind === "sell" ? "mkSellN" : "mkBuyN").textContent = list.length + " open";
    $(kind === "sell" ? "mkSell" : "mkBuy").innerHTML = rows.length ? rows.map(function (o) {
      var p = price(o), off = o.online === false;
      return "<tr" + (off ? " class=\"mk-off\"" : "") + "><td class=\"mono\"><span class=\"mk-live" + (off ? " off" : "") + "\" title=\"" + (off ? "Maker offline" : "Maker online: the swap starts within seconds") + "\"></span>" + sat(p) + " <span class=\"muted\">" + usd(p) + "</span></td><td class=\"mono\">" + gnr(o.gnr) + "</td><td class=\"mono\">" + btc(o.sats) + "</td><td>" + ago(Math.max(0, o.expires - data.now)) + "</td></tr>";
    }).join("") : "<tr><td colspan=\"4\" class=\"muted\">No " + kind + " offers yet. Publish one in the wallet's Swap tab.</td></tr>";
    var pg = $(kind === "sell" ? "mkSellP" : "mkBuyP");
    pg.innerHTML = pages > 1
      ? "<button class=\"btn\" data-mk=\"" + kind + "\" data-d=\"-1\"" + (page[kind] ? "" : " disabled") + ">‹</button><span>" + (page[kind] + 1) + " / " + pages + "</span><button class=\"btn\" data-mk=\"" + kind + "\" data-d=\"1\"" + (page[kind] < pages - 1 ? "" : " disabled") + ">›</button>"
      : "";
  }

  function render() {
    var m = data.market || {};
    $("mkPrice").textContent = m.price != null ? sat(m.price) : "no swaps yet";
    var np = Number(m.priced) || 0;
    $("mkPriceL").textContent = (m.price != null ? "Market price · average of " + np + " completed swap" + (np === 1 ? "" : "s") : "Market price · after the first completed swap") + (m.price != null && data.btc ? " · " + usd(m.price) : "");
    $("mkAsk").textContent = sat(m.best_ask); $("mkAskN").textContent = m.asks ? "(" + m.asks + ")" : "";
    $("mkBid").textContent = sat(m.best_bid); $("mkBidN").textContent = m.bids ? "(" + m.bids + ")" : "";
    $("mkVol").textContent = (m.trades || 0) + " done · " + (m.pending || 0) + " in progress";
    table("sell"); table("buy");
    $("mkPendN").textContent = data.pending.length ? data.pending.length + " running" : "";
    $("mkPend").innerHTML = data.pending.length ? data.pending.map(function (o) {
      var p = price(o);
      return "<tr><td class=\"mono\">" + sat(p) + " <span class=\"muted\">" + usd(p) + "</span></td><td class=\"mono\">" + gnr(o.gnr) + "</td><td class=\"mono\">" + btc(o.sats) + "</td><td>" + ago(Math.max(0, data.now - (o.since || data.now))) + " ago</td></tr>";
    }).join("") : "<tr><td colspan=\"4\" class=\"muted\">No swap running right now.</td></tr>";
    var all = data.recent, vol = all.reduce(function (a, t) { return a + Number(t.gnr || 0); }, 0);
    $("mkAll").textContent = all.length;
    $("mkAllV").textContent = all.length ? "· " + gnr(vol) + " GNR" : "";
    $("mkRecentN").textContent = all.length ? all.length + " shown" : "";
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
        "<span class=\"mk-sw-coin\">" + GKValue.coin("btc") + "<b>" + btc(t.sats) + "</b><em>BTC</em></span></div>" +
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
