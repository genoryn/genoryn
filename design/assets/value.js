/* GNR prices and coin marks, shared by genoryn.io and the wallet. The price of GNR is the
   market price of the offer board only: what people pay each other in atomic swaps. */
(function () {
  // One RTX 5090 does about 74 MH/s on KryptoHash (used for "what one GPU mines a day").
  var REF = { name: "RTX 5090", hashrate: 74e6 };
  var BLOCKS_PER_DAY = 1440;

  /* GNR one REF GPU mines a day at the network hashrate (H/s); subsidy in gori per block. */
  function perGpu(hashrate, subsidy) {
    hashrate = Number(hashrate) || 0;
    if (hashrate <= 0) return null;
    return (Number(subsidy) || 5e8) / 1e8 * BLOCKS_PER_DAY * Math.min(1, REF.hashrate / hashrate);
  }

  /* What mining one GNR costs one miner: o = { hs: the miner's H/s, watts, kwh: USD per
     kWh, network: network H/s (at least the miner's own), subsidy: gori per block }.
     Every block pays its reward to all miners by their share of the work, so a miner earns
     blocksPerDay × reward × hs / network GNR a day, and pays watts × 24 h × kwh for it. */
  function mineCost(o) {
    var hs = Number(o.hs) || 0, w = Number(o.watts) || 0, kwh = Number(o.kwh) || 0;
    var net = Math.max(Number(o.network) || 0, hs);
    if (hs <= 0 || net <= 0) return null;
    var perDay = (Number(o.subsidy) || 5e8) / 1e8 * BLOCKS_PER_DAY;
    var gnrDay = perDay * hs / net;
    var usdDay = w / 1000 * 24 * kwh;
    return { share: hs / net, gnrDay: gnrDay, usdDay: usdDay, usdPerGnr: gnrDay > 0 ? usdDay / gnrDay : null, perDay: perDay };
  }

  /* $0.0031, $0.40, $1,234.56; anything below a hundredth of a cent is "< $0.0001". */
  function usd(v) {
    v = Number(v) || 0;
    if (v <= 0) return "$0";
    if (v < 1e-4) return "< $0.0001";
    if (v < 0.01) return "$" + v.toPrecision(2).replace(/(\.\d*?)0+$/, "$1").replace(/\.$/, "");
    return "$" + v.toLocaleString("en-US", { minimumFractionDigits: 2, maximumFractionDigits: 2 });
  }

  /* A price in satoshis shown in bitcoin: 9700 -> "0.000097 BTC", 1e8 -> "1 BTC". */
  function btc(sat) {
    if (sat == null || !isFinite(Number(sat))) return "—";
    var v = Number(sat) / 1e8;
    if (v === 0) return "0 BTC";
    if (v < 1e-8) return "< 0.00000001 BTC";
    var s = v >= 1 ? v.toLocaleString("en-US", { maximumFractionDigits: 4 }) : v.toFixed(8).replace(/0+$/, "").replace(/\.$/, "");
    return s + " BTC";
  }

  /* "≈ $2.92", or "< $0.0001" without the ≈. */
  function approx(v) {
    var s = usd(v);
    return s.charAt(0) === "<" ? s : "≈ " + s;
  }

  /* The market price of one GNR in dollars: the board's price (sat per GNR) at the BTC
     price, or null while there is none. */
  function market(sat, btcUsd) {
    sat = Number(sat); btcUsd = Number(btcUsd);
    return sat > 0 && btcUsd > 0 ? sat * btcUsd / 1e8 : null;
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

  window.GKValue = { REF: REF, perGpu: perGpu, mineCost: mineCost, market: market, usd: usd, approx: approx, btc: btc, coin: coin, sprite: sprite };
})();
