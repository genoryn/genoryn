/* Reference value of GNR before it trades anywhere: the estimated electricity cost of
   mining one GNR, from the network hashrate. Not a market price. Shared by genoryn.io
   and the wallet. */
(function () {
  // One RTX 5090 does about 74 MH/s on KryptoHash at up to 575 W board power.
  var REF = { name: "RTX 5090", hashrate: 74e6, watts: 575 };
  var BLOCKS_PER_DAY = 1440;

  /* hashrate: network H/s · kwh: price per kWh in USD · subsidy: gori per block.
     Returns the pieces of the estimate, or null without a hashrate. */
  function estimate(hashrate, kwh, subsidy) {
    hashrate = Number(hashrate) || 0;
    if (hashrate <= 0) return null;
    kwh = Number(kwh) > 0 ? Number(kwh) : 0.15;
    var perBlock = (Number(subsidy) || 5e8) / 1e8;
    var gpus = hashrate / REF.hashrate;
    var kw = gpus * REF.watts / 1000;
    var usdPerDay = kw * 24 * kwh;
    var gnrPerDay = perBlock * BLOCKS_PER_DAY;
    return { usd: usdPerDay / gnrPerDay, gpus: gpus, kw: kw, usdPerDay: usdPerDay, gnrPerDay: gnrPerDay, kwh: kwh };
  }

  /* $0.0031, $0.40, $1,234.56; anything below a hundredth of a cent is "< $0.0001". */
  function usd(v) {
    v = Number(v) || 0;
    if (v <= 0) return "$0";
    if (v < 1e-4) return "< $0.0001";
    if (v < 0.01) return "$" + v.toPrecision(2).replace(/(\.\d*?)0+$/, "$1").replace(/\.$/, "");
    return "$" + v.toLocaleString("en-US", { minimumFractionDigits: 2, maximumFractionDigits: 2 });
  }

  /* "≈ $2.92", or "< $0.0001" without the ≈. */
  function approx(v) {
    var s = usd(v);
    return s.charAt(0) === "<" ? s : "≈ " + s;
  }

  /* The GNR price index, shared by genoryn.io and the wallet. The market leads; the cost
     to mine is a floor that a thin market cannot break:
       market ≥ cost: price = market
       market < cost: price = c × market + (1 − c) × cost
     market: the board's robust median (swaps, swaps in progress, offers) in USD;
     cost: electricity to mine one GNR at today's network hashrate, so the more miners
       compete, the more every GNR costs to make (miners rarely sell below that);
     c: the market's confidence (0 to 1, grows with the GNR traded behind its price), so
       only a deep market can price GNR below its cost.
     o: { sat: market sat per GNR, btcUsd, conf, hashrate, subsidy, kwh } */
  function index(o) {
    o = o || {};
    var sat = Number(o.sat), btc = Number(o.btcUsd);
    var m = sat > 0 && btc > 0 ? sat * btc / 1e8 : null;
    var e = estimate(o.hashrate, o.kwh, o.subsidy);
    var f = e ? e.usd : null;
    if (m == null && f == null) return null;
    if (m == null) return { usd: f, market: null, floor: f, conf: 0, est: e, source: "cost" };
    var c = o.conf == null || !isFinite(Number(o.conf)) ? 1 : Math.max(0, Math.min(1, Number(o.conf)));
    if (f == null) return { usd: m, market: m, floor: null, conf: c, est: null, source: "market" };
    if (m >= f) return { usd: m, market: m, floor: f, conf: c, est: e, source: "market" };
    return { usd: c * m + (1 - c) * f, market: m, floor: f, conf: c, est: e, source: "floor" };
  }

  /* Coin marks in the text colour (so they follow the theme): the Genoryn sign and the
     Bitcoin B, each on a ring. Use GKValue.coin("gnr") / GKValue.coin("btc"). */
  var SIGN = '<path d="M186 128A58 58 0 1 1 171.1 89.19" fill="none" stroke="currentColor" stroke-width="30"/>' +
    '<path fill="currentColor" d="M134 113h67v30h-67zM106 31h14v54h-14zM106 171h14v54h-14zM136 31h14v54h-14zM136 171h14v54h-14z"/>';
  var SPRITE = '<svg xmlns="http://www.w3.org/2000/svg" style="position:absolute;width:0;height:0;overflow:hidden" aria-hidden="true">' +
    '<symbol id="c-gnr" viewBox="0 0 256 256"><circle cx="128" cy="128" r="119" fill="none" stroke="currentColor" stroke-width="13"/>' +
    '<g transform="translate(128 128) scale(.74) translate(-128 -128)">' + SIGN + "</g></symbol>" +
    '<symbol id="c-btc" viewBox="0 0 256 256"><circle cx="128" cy="128" r="119" fill="none" stroke="currentColor" stroke-width="13"/>' +
    '<g transform="rotate(14 128 128)"><path fill="currentColor" fill-rule="evenodd" d="M92 76h58c26 0 38 14 38 31 0 12-7 21-18 25 17 4 26 15 26 31 0 21-15 35-42 35H92zM118 96v28h29c11 0 17-6 17-14s-6-14-17-14zM118 144v32h33c13 0 19-7 19-16s-6-16-19-16z"/>' +
    '<path fill="currentColor" d="M108 52h14v26h-14zM134 52h14v26h-14zM108 196h14v26h-14zM134 196h14v26h-14z"/></g></symbol></svg>';
  function sprite() {
    if (document.getElementById("c-gnr") || !document.body) return;
    var d = document.createElement("div");
    d.innerHTML = SPRITE;
    document.body.insertBefore(d.firstChild, document.body.firstChild);
  }
  if (document.body) sprite(); else document.addEventListener("DOMContentLoaded", sprite);
  function coin(kind, cls) {
    kind = kind === "btc" ? "btc" : "gnr";
    return '<svg class="coin coin-' + kind + (cls ? " " + cls : "") + '" aria-label="' + (kind === "btc" ? "BTC" : "GNR") + '" role="img"><use href="#c-' + kind + '"/></svg>';
  }

  window.GKValue = { REF: REF, estimate: estimate, index: index, usd: usd, approx: approx, coin: coin, sprite: sprite };
})();
