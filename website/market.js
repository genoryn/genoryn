/* genoryn.io/market: the live offer board and market price, read from the node's /v1/swap API. */
(function () {
  var PER = 20, page = { sell: 0, buy: 0 }, data = { offers: [], pending: [], market: {}, recent: [], btc: 0, now: 0 };
  var $ = function (id) { return document.getElementById(id); };
  var sat = function (p) { return p == null ? "—" : (p >= 100 ? Math.round(p).toLocaleString("en-US") : Number(p).toPrecision(3)) + " sat"; };
  var usd = function (p) { return data.btc > 0 && p != null ? GKValue.usd(p * data.btc / 1e8) : ""; };
  var gnr = function (v) { return (v / 1e8).toLocaleString("en-US", { maximumFractionDigits: 8 }); };
  var btc = function (v) { return (v / 1e8).toLocaleString("en-US", { minimumFractionDigits: 4, maximumFractionDigits: 8 }); };
  var ago = function (s) { return s >= 3600 ? Math.floor(s / 3600) + " h" : Math.max(1, Math.floor(s / 60)) + " min"; };
  var price = function (o) { return o.sats / (o.gnr / 1e8); };

  function table(kind) {
    var side = kind === "sell" ? "sell_gnr" : "buy_gnr";
    var list = data.offers.filter(function (o) { return o.side === side; });
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
    $("mkPrice").textContent = m.price != null ? sat(m.price) : "no offers yet";
    var nOff = (m.asks || 0) + (m.bids || 0);
    $("mkPriceL").textContent = (m.source === "swaps" ? "Market price · swaps, swaps in progress and offers"
      : m.source === "pending" ? "Market price · " + m.pending + " swap" + (m.pending === 1 ? "" : "s") + " in progress, " + nOff + " offer" + (nOff === 1 ? "" : "s")
      : m.source === "offers" ? "Market price · from " + nOff + " offer" + (nOff === 1 ? "" : "s") : "Market price per GNR") + (m.price != null && data.btc ? " · " + usd(m.price) : "");
    $("mkAsk").textContent = sat(m.best_ask); $("mkAskN").textContent = m.asks ? "(" + m.asks + ")" : "";
    $("mkBid").textContent = sat(m.best_bid); $("mkBidN").textContent = m.bids ? "(" + m.bids + ")" : "";
    $("mkVol").textContent = (m.trades || 0) + " done · " + (m.pending || 0) + " in progress";
    table("sell"); table("buy");
    $("mkPendN").textContent = data.pending.length ? data.pending.length + " running" : "";
    $("mkPend").innerHTML = data.pending.length ? data.pending.map(function (o) {
      var p = price(o);
      return "<tr><td class=\"mono\">" + sat(p) + " <span class=\"muted\">" + usd(p) + "</span></td><td class=\"mono\">" + gnr(o.gnr) + "</td><td class=\"mono\">" + btc(o.sats) + "</td><td>" + ago(Math.max(0, data.now - (o.since || data.now))) + " ago</td></tr>";
    }).join("") : "<tr><td colspan=\"4\" class=\"muted\">No swap running right now.</td></tr>";
    $("mkRecent").innerHTML = data.recent.length ? data.recent.map(function (t) {
      return "<tr><td>" + ago(Math.max(0, data.now - t.at)) + " ago</td><td class=\"mono\">" + sat(t.price) + " <span class=\"muted\">" + usd(t.price) + "</span></td><td class=\"mono\">" + gnr(t.gnr) + "</td><td class=\"mono\">" + btc(t.sats) + "</td></tr>";
    }).join("") : "<tr><td colspan=\"4\" class=\"muted\">No swaps yet.</td></tr>";
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
      var info = r[3];
      if (info) {
        var left = 3300 - (info.height + 1);
        $("mkLive").textContent = info.network === "mainnet" && left > 0
          ? "Swaps on mainnet activate at block 3,300: about " + Math.ceil(left / 60) + " hours from now (block " + info.height.toLocaleString("en-US") + " today). Offers can already be published."
          : "Swaps are live.";
      }
      render();
    });
  }
  document.addEventListener("click", function (e) {
    var b = e.target.closest ? e.target.closest("[data-mk]") : null;
    if (!b) return;
    page[b.getAttribute("data-mk")] += Number(b.getAttribute("data-d"));
    render();
  });
  load();
  setInterval(function () { if (!document.hidden) load(); }, 30000);
})();
