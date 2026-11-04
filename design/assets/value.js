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

  window.GKValue = { REF: REF, estimate: estimate, usd: usd, approx: approx };
})();
