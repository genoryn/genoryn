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

  /* Rewards by year: 5 GNR a block, halving every 2,100,000 blocks, one block a minute. */
  var HALVING = 2100000, BLOCK_SECS = 60, SUBSIDY0 = 5e8;
  function rewardAt(h) {
    if (h < 1) return 0;
    var e = Math.floor((h - 1) / HALVING);
    return e >= 33 ? 0 : Math.floor(SUBSIDY0 / Math.pow(2, e));
  }
  /* What one of `computers` equally fast computers mines at block `h` (GNR). */
  function perComputer(computers, h) {
    var r = rewardAt(h) / 1e8, n = Math.max(1, Number(computers) || 1);
    var day = r * 86400 / BLOCK_SECS / n;
    return { reward: r, day: day, month: day * 30.44, year: day * 365.25, secsPer1: day > 0 ? 86400 / day : null };
  }
  function span(s) {
    if (s == null || !isFinite(s)) return "never";
    var f = function (v, u) { v = v < 10 ? Math.round(v * 10) / 10 : Math.round(v); return v.toLocaleString("en-US") + " " + u + (v === 1 ? "" : "s"); };
    return s < 60 ? f(s, "second") : s < 3600 ? f(s / 60, "minute") : s < 86400 ? f(s / 3600, "hour") : s < 86400 * 365.25 ? f(s / 86400, "day") : f(s / (86400 * 365.25), "year");
  }
  function amt(v) { return v.toLocaleString("en-US", { maximumFractionDigits: v < 0.01 ? 6 : v < 1 ? 4 : v < 100 ? 2 : 0 }); }

  /* The miner calculator, the same on genoryn.io and in the wallet. live() returns
     { height, miners } from the node (miners: computers mining now, estimated). */
  function mountCalc(root, live) {
    var now = new Date(), y0 = now.getFullYear();
    var years = "";
    for (var k = 0; k <= 40; k++) years += '<option value="' + k + '">' + (k ? (y0 + k) + " (in " + k + " year" + (k === 1 ? "" : "s") + ")" : "Now (" + y0 + ")") + "</option>";
    var sizes = [100, 1000, 100000, 1000000, 10000000];
    root.innerHTML =
      '<div class="mc-now">' +
      '<div><span>Block now</span><b data-c="h">—</b><em data-c="hr">—</em></div>' +
      '<div><span>Computers mining now</span><b data-c="n">—</b><em>estimated from the shares</em></div>' +
      '<div><span>Each one mines now</span><b data-c="d">—</b><em data-c="dm">—</em></div>' +
      '<div class="mc-hl"><span>1 GNR takes one computer</span><b data-c="t">—</b><em>on average, right now</em></div></div>' +
      '<div class="mc-in"><label>When<select data-c="y">' + years + '</select></label>' +
      '<label>Computers mining<select data-c="s">' + sizes.map(function (n) { return '<option value="' + n + '">' + n.toLocaleString("en-US") + "</option>"; }).join("") +
      '<option value="now">As many as now</option><option value="custom">Other number</option></select></label>' +
      '<label class="mc-custom" style="display:none">Number of computers<input data-c="c" type="number" min="1" step="1" inputmode="numeric" value="50000"></label></div>' +
      '<div class="mc-res" data-c="r"></div>' +
      '<div class="mc-tw"><table class="mc-table"><thead><tr><th>Computers mining</th><th>Each one a day</th><th>a month</th><th>a year</th><th>1 GNR takes one computer</th></tr></thead><tbody data-c="rows"></tbody></table></div>' +
      '<p class="mc-note">Every block pays its reward to all miners together, split by the work each one does; here every computer is equally fast. One block a minute; the reward starts at 5 GNR and halves every 2,100,000 blocks (about every 4 years); at most 21,000,000 GNR will ever exist. Electricity and hardware are not counted.</p>';
    var q = function (k) { return root.querySelector('[data-c="' + k + '"]'); };
    function draw() {
      var L = live() || {}, h = Number(L.height) || 0, nNow = Math.max(1, Number(L.miners) || 1);
      var k = Number(q("y").value), at = h + Math.round(k * 365.25 * 86400 / BLOCK_SECS);
      var sel = q("s").value;
      root.querySelector(".mc-custom").style.display = sel === "custom" ? "" : "none";
      var n = sel === "now" ? nNow : sel === "custom" ? Math.max(1, Math.round(Number(q("c").value) || 1)) : Number(sel);
      var p0 = perComputer(nNow, h || 1);
      q("h").textContent = h ? "#" + h.toLocaleString("en-US") : "—";
      q("hr").textContent = h ? amt(p0.reward) + " GNR a block · next halving at #" + ((Math.floor(Math.max(0, h - 1) / HALVING) + 1) * HALVING + 1).toLocaleString("en-US") : "—";
      q("n").textContent = L.miners != null ? nNow.toLocaleString("en-US") : "—";
      q("d").textContent = h ? amt(p0.day) + " GNR a day" : "—";
      q("dm").textContent = h ? amt(p0.month) + " a month · " + amt(p0.year) + " a year" : "—";
      q("t").textContent = h ? span(p0.secsPer1) : "—";
      var p = perComputer(n, at || 1);
      q("r").innerHTML = "<b>" + (k ? y0 + k : "Now") + "</b>: block ≈ #" + Math.max(1, at).toLocaleString("en-US") + ", " + amt(p.reward) + " GNR a block. With <b>" + n.toLocaleString("en-US") +
        "</b> computers each mines <b>" + amt(p.day) + " GNR a day</b> (" + amt(p.month) + " a month, " + amt(p.year) + " a year): one GNR every <b>" + span(p.secsPer1) + "</b>.";
      q("rows").innerHTML = sizes.map(function (m) {
        var x = perComputer(m, at || 1);
        return "<tr" + (m === n ? ' class="on"' : "") + "><td>" + m.toLocaleString("en-US") + "</td><td>" + amt(x.day) + " GNR</td><td>" + amt(x.month) + "</td><td>" + amt(x.year) + "</td><td>" + span(x.secsPer1) + "</td></tr>";
      }).join("");
    }
    ["y", "s", "c"].forEach(function (k) { q(k).addEventListener("input", draw); q(k).addEventListener("change", draw); });
    draw();
    return { update: draw };
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

  window.GKValue = { REF: REF, perGpu: perGpu, rewardAt: rewardAt, perComputer: perComputer, mountCalc: mountCalc, market: market, usd: usd, approx: approx, btc: btc, coin: coin, sprite: sprite };
})();
