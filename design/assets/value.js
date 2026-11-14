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

  /* GNR in existence after block h (the subsidies only; fees move coins, they do not add any). */
  function supplyAt(h) {
    var s = 0, left = Math.max(0, Math.floor(h));
    for (var e = 0; left > 0 && e < 33; e++) {
      var n = Math.min(left, HALVING);
      s += n * Math.floor(SUBSIDY0 / Math.pow(2, e));
      left -= n;
    }
    return s / 1e8;
  }
  // About 19.95 million BTC exist in late 2026; only used to put a GNR total next to Bitcoin's.
  var BTC_SUPPLY = 19.95e6;
  var GNR_YEAR = 525960, BTC_YEAR = 52596, GOLD_SF = 60;

  /* Bitcoin by its own schedule: 50 BTC a block halving every 210,000 blocks, ten minutes a
     block, block 840,000 on 20 April 2024. */
  function btcSupplyAt(h) {
    var s = 0, left = Math.max(0, Math.floor(h));
    for (var e = 0; left > 0 && e < 33; e++) {
      var n = Math.min(left, 210000);
      s += n * Math.floor(5e9 / Math.pow(2, e));
      left -= n;
    }
    return s / 1e8;
  }
  function btcHeight(ms) { return 840000 + (ms - Date.UTC(2024, 3, 20)) / 6e5; }

  /* Stock-to-flow: the coins that exist divided by the coins mined in the following year,
     i.e. how many years of mining are already stored. Gold is about 60. */
  function scarcity(stock, next) { return next > 0 ? stock / next : Infinity; }
  function gnrScarcity(h) { return scarcity(supplyAt(h), supplyAt(h + GNR_YEAR) - supplyAt(h)); }
  function btcScarcity(ms) { var b = btcHeight(ms); return scarcity(btcSupplyAt(b), btcSupplyAt(b + BTC_YEAR) - btcSupplyAt(b)); }
  function big(v) {
    return v >= 1e12 ? amt(v / 1e12) + " trillion" : v >= 1e9 ? amt(v / 1e9) + " billion" : v >= 1e6 ? amt(v / 1e6) + " million" : null;
  }
  function dollars(v) { var b = big(v); return b ? "$" + b : v >= 100 ? "$" + Math.round(v).toLocaleString("en-US") : usd(v); }
  function count(v) { var b = big(v); return b || Math.round(v).toLocaleString("en-US"); }
  function options(list, sel, fmt) {
    return list.map(function (v) { return '<option value="' + v + '"' + (v === sel ? " selected" : "") + ">" + fmt(v) + "</option>"; }).join("");
  }

  /* The miner and price calculator, the same on genoryn.io and in the wallet. live() returns
     { height, miners, hashrate (H/s), supply (gori), sat (market sat per GNR), btcUsd } from the node. */
  function mountCalc(root, live) {
    var now = new Date(), y0 = now.getFullYear();
    var years = "";
    for (var k = 0; k <= 40; k++) years += '<option value="' + k + '">' + (k ? (y0 + k) + " (in " + k + " year" + (k === 1 ? "" : "s") + ")" : "Now (" + y0 + ")") + "</option>";
    var sizes = [100, 1000, 100000, 1000000, 10000000];
    var people = [1000, 10000, 100000, 1000000, 10000000, 100000000];
    var holds = [100, 1000, 10000, 100000];
    var joins = [1, 10, 100, 1000, 10000];
    var shares = [0.0001, 0.001, 0.01, 0.1, 0.5, 1];
    var worths = [0.001, 0.003, 0.01, 0.1, 1];
    var sfYears = [0, 4, 8, 12, 16, 20, 30, 40];
    var fmtN = function (n) { return n.toLocaleString("en-US"); };
    root.innerHTML =
      '<div class="mc-now">' +
      '<div><span>Block now</span><b data-c="h">—</b><em data-c="hr">—</em></div>' +
      '<div><span>Computers mining now</span><b data-c="n">—</b><em>estimated from the shares</em></div>' +
      '<div><span>Each one mines now</span><b data-c="d">—</b><em data-c="dm">—</em></div>' +
      '<div class="mc-hl"><span>1 GNR takes one computer</span><b data-c="t">—</b><em>on average, right now</em></div></div>' +
      '<div class="mc-px mc-first">' +
      '<h4 class="mc-h">Join now: what your computers would mine</h4>' +
      '<p class="mc-sub" data-c="js">—</p>' +
      '<div class="mc-in"><label>Your GPUs (each like one ' + REF.name + ', ' + REF.hashrate / 1e6 + ' MH/s)<select data-c="jd">' + options(joins, 1, fmtN) + '<option value="custom">Other number</option></select></label>' +
      '<label class="mc-cjd" style="display:none">Number of GPUs<input data-c="jdc" type="number" min="1" step="1" inputmode="numeric" value="50"></label></div>' +
      '<div class="mc-now">' +
      '<div><span>Your share of the network</span><b data-c="jsh">—</b><em data-c="jshe">—</em></div>' +
      '<div><span>You would mine</span><b data-c="jg">—</b><em data-c="jge">—</em></div>' +
      '<div class="mc-hl"><span>1 GNR would take you</span><b data-c="jt">—</b><em>on average, at today\'s hashrate</em></div>' +
      '<div><span>Worth at the market price</span><b data-c="jv">—</b><em data-c="jve">—</em></div></div>' +
      '<div class="mc-tw"><table class="mc-table"><thead><tr><th>Your GPUs</th><th>Your share</th><th>GNR a day</th><th>1 GNR takes</th><th>A day at the market</th><th data-c="jph">A day at the what-if price</th></tr></thead><tbody data-c="jrows"></tbody></table></div>' +
      '<p class="mc-note">The more GPUs join, the smaller each one\'s share: your GPUs are added to everyone\'s, and every block\'s reward is split by the work. GPUs of other kinds count by their hashrate.</p></div>' +
      '<h4 class="mc-h">The future: by year and number of computers</h4>' +
      '<div class="mc-in"><label>When<select data-c="y">' + years + '</select></label>' +
      '<label>Computers mining<select data-c="s">' + options(sizes, 100, fmtN) +
      '<option value="now">As many as now</option><option value="custom">Other number</option></select></label>' +
      '<label class="mc-custom" style="display:none">Number of computers<input data-c="c" type="number" min="1" step="1" inputmode="numeric" value="50000"></label></div>' +
      '<div class="mc-res" data-c="r"></div>' +
      '<div class="mc-tw"><table class="mc-table"><thead><tr><th>Computers mining</th><th>Each one a day</th><th>a month</th><th>a year</th><th>1 GNR takes one computer</th></tr></thead><tbody data-c="rows"></tbody></table></div>' +
      '<p class="mc-note">Every block pays its reward to all miners together, split by the work each one does; here every computer is equally fast. One block a minute; the reward starts at 5 GNR and halves every 2,100,000 blocks (about every 4 years); at most 21,000,000 GNR will ever exist. Electricity and hardware are not counted.</p>' +
      '<div class="mc-px">' +
      '<h4 class="mc-h">Scarcity: how rare GNR becomes</h4>' +
      '<p class="mc-sub">Stock-to-flow: the GNR that exist divided by the new GNR mined in the next year, so how many years of mining are already stored. The higher it is, the rarer the coin. Gold is about ' + GOLD_SF + '. Every halving doubles it, and there will never be more than 21,000,000 GNR.</p>' +
      '<div class="mc-now">' +
      '<div><span>Scarcity now</span><b data-c="sf0">—</b><em data-c="sf0e">—</em></div>' +
      '<div><span>Scarcity in the year above</span><b data-c="sfy">—</b><em data-c="sfye">—</em></div>' +
      '<div class="mc-hl"><span>As scarce as gold</span><b data-c="sfg">—</b><em>stock-to-flow ' + GOLD_SF + '</em></div>' +
      '<div class="mc-hl"><span>As scarce as Bitcoin is today</span><b data-c="sfb">—</b><em data-c="sfbe">—</em></div></div>' +
      '<div class="mc-tw"><table class="mc-table"><thead><tr><th>Year</th><th>GNR that exist</th><th>New GNR that year</th><th>GNR scarcity</th><th>Bitcoin then</th><th>Gold</th></tr></thead><tbody data-c="sfrows"></tbody></table></div>' +
      '<p class="mc-note">Scarcity is a fact of the schedule; it does not set a price by itself. A coin is worth something only while people want it: scarcity decides how much of it each of them can have.</p></div>' +
      '<div class="mc-px">' +
      '<h4 class="mc-h">Price calculator: what if</h4>' +
      '<p class="mc-sub">Three simple ways to put a number on GNR in the year and with the computers chosen above. None is a forecast; each shows what one assumption means per coin.</p>' +
      '<h5 class="mc-h5">1. The work behind one GNR</h5>' +
      '<p class="mc-sub">GNR only comes from mining. The more computers share the same reward, and the more halvings have passed, the more work one GNR takes. If a day of a computer\'s mining stays worth what it is worth today, one GNR is worth more in proportion.</p>' +
      '<div class="mc-in"><label>One GNR is worth today<select data-c="ws"><option value="market">the market price</option>' + options(worths, -1, function (v) { return "$" + v + " (1,000 GNR for $" + amt(v * 1000) + ")"; }) + '<option value="custom">Other amount</option></select></label>' +
      '<label class="mc-cws" style="display:none">Dollars for one GNR today<input data-c="wsc" type="number" min="0" step="any" inputmode="decimal" value="0.003"></label></div>' +
      '<div class="mc-res" data-c="wres"></div>' +
      '<div class="mc-tw"><table class="mc-table"><thead><tr><th>Computers mining</th><th>Each one a day</th><th>Work behind 1 GNR</th><th>One GNR worth</th><th>1,000 GNR worth</th></tr></thead><tbody data-c="wrows"></tbody></table></div>' +
      '<h5 class="mc-h5">2. A share of Bitcoin\'s value</h5>' +
      '<p class="mc-sub" data-c="bsub">—</p>' +
      '<div class="mc-in"><label>GNR worth, as a share of Bitcoin today<select data-c="bs">' + options(shares, 0.01, function (v) { return amt(v * 100) + " %"; }) + '</select></label></div>' +
      '<div class="mc-res" data-c="bres"></div>' +
      '<div class="mc-tw"><table class="mc-table"><thead><tr><th>Share of Bitcoin\'s value</th><th>All GNR worth</th><th>One GNR worth</th><th>One computer a day</th></tr></thead><tbody data-c="brows"></tbody></table></div>' +
      '<h5 class="mc-h5">3. People and dollars</h5>' +
      '<p class="mc-sub">A price is what people pay. If a number of people each hold GNR worth some dollars, one GNR is worth all their dollars divided by the GNR that exist then. The reward halves while more people arrive, so each person can own less and less.</p>' +
      '<div class="mc-in mc-in4"><label>People holding GNR<select data-c="pp">' + options(people, 10000, fmtN) + '<option value="custom">Other number</option></select></label>' +
      '<label class="mc-cpp" style="display:none">Number of people<input data-c="ppc" type="number" min="1" step="1" inputmode="numeric" value="50000"></label>' +
      '<label>Each one holds<select data-c="ph">' + options(holds, 1000, function (v) { return "$" + fmtN(v); }) + '<option value="custom">Other amount</option></select></label>' +
      '<label class="mc-cph" style="display:none">Dollars each<input data-c="phc" type="number" min="1" step="1" inputmode="decimal" value="500"></label></div>' +
      '<div class="mc-now">' +
      '<div><span>GNR that exist then</span><b data-c="sp">—</b><em data-c="spe">—</em></div>' +
      '<div><span>GNR for each person</span><b data-c="pe">—</b><em data-c="pee">—</em></div>' +
      '<div class="mc-hl"><span>One GNR would be worth</span><b data-c="pr">—</b><em data-c="pre">—</em></div>' +
      '<div><span>Each computer would earn</span><b data-c="pc">—</b><em data-c="pce">—</em></div></div>' +
      '<div class="mc-res" data-c="pres"></div>' +
      '<div class="mc-tw"><table class="mc-table"><thead><tr><th>People holding GNR</th><th>GNR each</th><th>One GNR worth</th><th>All GNR worth</th><th>One computer a day</th></tr></thead><tbody data-c="prows"></tbody></table></div>' +
      '<p class="mc-note" data-c="mk"></p>' +
      '<p class="mc-note">A what-if calculation, not a forecast and not financial advice: nobody knows the future price. The real price is only what people pay each other on the market.</p></div>';
    var q = function (k) { return root.querySelector('[data-c="' + k + '"]'); };
    var show = function (cls, on) { root.querySelector(cls).style.display = on ? "" : "none"; };
    function pick(sel, input, cls) {
      var v = q(sel).value;
      show(cls, v === "custom");
      return v === "custom" ? Math.max(1, Number(q(input).value) || 1) : Number(v);
    }
    function draw() {
      var L = live() || {}, h = Number(L.height) || 0, nNow = Math.max(1, Number(L.miners) || 1);
      var k = Number(q("y").value), at = h + Math.round(k * 365.25 * 86400 / BLOCK_SECS);
      var sel = q("s").value;
      show(".mc-custom", sel === "custom");
      var n = sel === "now" ? nNow : sel === "custom" ? Math.max(1, Math.round(Number(q("c").value) || 1)) : Number(sel);
      var btcUsd = Number(L.btcUsd) || 0, mkt = market(L.sat, btcUsd);
      var p0 = perComputer(nNow, h || 1);
      q("h").textContent = h ? "#" + h.toLocaleString("en-US") : "—";
      q("hr").textContent = h ? amt(p0.reward) + " GNR a block · next halving at #" + ((Math.floor(Math.max(0, h - 1) / HALVING) + 1) * HALVING + 1).toLocaleString("en-US") : "—";
      q("n").textContent = L.miners != null ? nNow.toLocaleString("en-US") : "—";
      q("d").textContent = h ? amt(p0.day) + " GNR a day" : "—";
      q("dm").textContent = h ? amt(p0.month) + " a month · " + amt(p0.year) + " a year" + (mkt != null ? " · ≈ " + dollars(p0.day * mkt) + " a day now" : "") : "—";
      q("t").textContent = h ? span(p0.secsPer1) : "—";
      var p = perComputer(n, at || 1);
      q("r").innerHTML = "<b>" + (k ? y0 + k : "Now") + "</b>: block ≈ #" + Math.max(1, at).toLocaleString("en-US") + ", " + amt(p.reward) + " GNR a block. With <b>" + n.toLocaleString("en-US") +
        "</b> computers each mines <b>" + amt(p.day) + " GNR a day</b> (" + amt(p.month) + " a month, " + amt(p.year) + " a year): one GNR every <b>" + span(p.secsPer1) + "</b>." +
        (mkt != null ? " At today's market price (" + usd(mkt) + ") that is ≈ " + dollars(p.day * mkt) + " a day." : "");
      q("rows").innerHTML = sizes.map(function (m) {
        var x = perComputer(m, at || 1);
        return "<tr" + (m === n ? ' class="on"' : "") + "><td>" + m.toLocaleString("en-US") + "</td><td>" + amt(x.day) + " GNR</td><td>" + amt(x.month) + "</td><td>" + amt(x.year) + "</td><td>" + span(x.secsPer1) + "</td></tr>";
      }).join("");

      // Price: the dollars people hold divided by the GNR that exist in the chosen year.
      var who = Math.round(pick("pp", "ppc", ".mc-cpp")), each = pick("ph", "phc", ".mc-cph");
      var liveSupply = Number(L.supply) > 0 ? Number(L.supply) / 1e8 : supplyAt(h);
      var coins = Math.max(liveSupply + supplyAt(at) - supplyAt(h), 1e-8);
      var price = who * each / coins, btcCap = btcUsd * BTC_SUPPLY;
      var vsBtc = function (cap) {
        if (!(btcCap > 0)) return "";
        var r = cap / btcCap;
        return r >= 0.995 ? amt(r) + "× all Bitcoin today" : (r * 100 >= 0.01 ? amt(r * 100) : "< 0.01") + " % of all Bitcoin today";
      };
      q("sp").textContent = count(coins);
      q("spe").textContent = amt(coins / 21e4) + " % of the 21,000,000 there will ever be";
      q("pe").textContent = amt(coins / who) + " GNR";
      q("pee").textContent = "if " + who.toLocaleString("en-US") + " people shared all of it";
      q("pr").textContent = dollars(price);
      q("pre").textContent = btcUsd > 0 ? btc(price / btcUsd * 1e8) + (mkt != null ? " · " + amt(price / mkt) + "× today's market price" : "") : "—";
      q("pc").textContent = dollars(p.day * price) + " a day";
      q("pce").textContent = "with " + n.toLocaleString("en-US") + " computers mining in " + (k ? y0 + k : y0);
      q("pres").innerHTML = "<b>" + (k ? y0 + k : "Now") + "</b>: about <b>" + count(coins) + " GNR</b> exist. If <b>" + who.toLocaleString("en-US") + "</b> people each hold <b>" + dollars(each) +
        "</b> of GNR, that is " + dollars(who * each) + " for " + count(coins) + " GNR: <b>" + dollars(price) + " a GNR</b>. All GNR together would be worth " + dollars(price * coins) +
        (btcCap > 0 ? " (" + vsBtc(price * coins) + ")" : "") + ".";
      q("prows").innerHTML = people.map(function (m) {
        var x = m * each / coins;
        return "<tr" + (m === who ? ' class="on"' : "") + "><td>" + m.toLocaleString("en-US") + "</td><td>" + amt(coins / m) + "</td><td>" + dollars(x) + "</td><td>" + dollars(x * coins) + "</td><td>" + dollars(p.day * x) + "</td></tr>";
      }).join("");
      var full = 1e6 * 21e6;
      q("mk").textContent = (mkt != null ? "Market price now: " + usd(mkt) + " a GNR (" + btc(L.sat) + "), from real swaps on the offer board; all " + count(liveSupply) + " GNR mined so far are worth " + dollars(mkt * liveSupply) + ". " : "No market price yet: it appears with the first swaps. ") +
        "For scale: at $1,000,000 a GNR, all 21,000,000 GNR would be worth $21 trillion" + (btcCap > 0 ? " (" + vsBtc(full) + ")" : "") + ".";

      // Scarcity by the two schedules, from today's block (GNR) and today's date (Bitcoin).
      var hh = Math.max(1, h), yearMs = 365.25 * 864e5, t0 = now.getTime();
      var sfTxt = function (v) { return !isFinite(v) ? "∞" : v < 1 ? amt(v) : Math.round(v).toLocaleString("en-US"); };
      var sfNow = gnrScarcity(hh), sfAt = gnrScarcity(Math.max(1, at)), btcNow = btcScarcity(t0);
      var newYear = function (x) { return supplyAt(x + GNR_YEAR) - supplyAt(x); };
      var when = function (target) {
        for (var y = 0; y <= 80; y += 0.25) if (gnrScarcity(hh + y * GNR_YEAR) >= target) return y;
        return null;
      };
      var yearOf = function (y) { return y == null ? "after " + (y0 + 80) : y === 0 ? "already" : "around " + new Date(t0 + y * yearMs).getFullYear(); };
      q("sf0").textContent = sfTxt(sfNow);
      q("sf0e").textContent = count(supplyAt(hh)) + " GNR exist · " + count(newYear(hh)) + " new in a year";
      q("sfy").textContent = sfTxt(sfAt);
      q("sfye").textContent = (k ? y0 + k : "now") + ": " + count(supplyAt(Math.max(1, at))) + " GNR · " + count(newYear(Math.max(1, at))) + " new a year";
      q("sfg").textContent = yearOf(when(GOLD_SF));
      q("sfb").textContent = yearOf(when(btcNow));
      q("sfbe").textContent = "stock-to-flow " + sfTxt(btcNow);
      q("sfrows").innerHTML = sfYears.map(function (y) {
        var x = hh + Math.round(y * GNR_YEAR);
        return "<tr" + (y === k ? ' class="on"' : "") + "><td>" + (y ? y0 + y : "Now") + "</td><td>" + count(supplyAt(x)) + "</td><td>" + count(newYear(x)) + "</td><td>" + sfTxt(gnrScarcity(x)) +
          "</td><td>" + sfTxt(btcScarcity(t0 + y * yearMs)) + "</td><td>≈ " + GOLD_SF + "</td></tr>";
      }).join("");

      // The work behind one GNR: today's price × how much less one computer mines then.
      var wsel = q("ws").value;
      show(".mc-cws", wsel === "custom");
      var p0 = wsel === "market" ? (mkt != null ? mkt : 0.003) : wsel === "custom" ? Math.max(0, Number(q("wsc").value) || 0) : Number(wsel);
      var dayNow = perComputer(nNow, h || 1).day;
      var workPx = function (m) { var d = perComputer(m, at || 1).day; return d > 0 ? p0 * dayNow / d : null; };
      var wp = workPx(n);
      q("wres").innerHTML = "Today one computer of " + nNow.toLocaleString("en-US") + " mines <b>" + amt(dayNow) + " GNR a day</b>, worth " + dollars(dayNow * p0) + " at " + usd(p0) + " a GNR" +
        (wsel === "market" && mkt == null ? " (no market price yet: $0.003 assumed)" : "") + ". <b>" + (k ? y0 + k : "Now") + "</b> with <b>" + n.toLocaleString("en-US") + "</b> computers it mines " +
        amt(p.day) + " GNR a day, so one GNR takes <b>" + amt(dayNow / Math.max(p.day, 1e-18)) + "× the work</b>. For that day of mining to be worth the same, one GNR would be worth <b>" +
        (wp != null ? dollars(wp) : "—") + "</b> and 1,000 GNR " + (wp != null ? dollars(wp * 1000) : "—") + ".";
      q("wrows").innerHTML = sizes.map(function (m) {
        var x = workPx(m), d = perComputer(m, at || 1).day;
        return "<tr" + (m === n ? ' class="on"' : "") + "><td>" + m.toLocaleString("en-US") + "</td><td>" + amt(d) + " GNR</td><td>" + amt(dayNow / Math.max(d, 1e-18)) + "×</td><td>" + (x != null ? dollars(x) : "—") + "</td><td>" + (x != null ? dollars(x * 1000) : "—") + "</td></tr>";
      }).join("");

      // A share of Bitcoin's value today, spread over the GNR that exist in the chosen year.
      var share = Number(q("bs").value) || 0.01;
      q("bsub").textContent = btcCap > 0 ? "Bitcoin is worth about " + dollars(btcCap) + " today (" + dollars(btcUsd) + " × " + count(BTC_SUPPLY) + " BTC). If all GNR together became worth a part of that, one GNR would be that part divided by the GNR that exist then."
        : "Waiting for the Bitcoin price…";
      var sharePx = function (s) { return btcCap > 0 ? s * btcCap / coins : null; };
      var bp = sharePx(share);
      q("bres").innerHTML = bp == null ? "—" : "<b>" + (k ? y0 + k : "Now") + "</b>: at <b>" + amt(share * 100) + " %</b> of Bitcoin's value (" + dollars(share * btcCap) + ") and " + count(coins) +
        " GNR, one GNR would be worth <b>" + dollars(bp) + "</b>" + (mkt != null ? " (" + amt(bp / mkt) + "× today's market price)" : "") + ", and each of " + n.toLocaleString("en-US") +
        " computers would earn " + dollars(p.day * bp) + " a day.";
      q("brows").innerHTML = btcCap > 0 ? shares.map(function (s) {
        var x = sharePx(s);
        return "<tr" + (s === share ? ' class="on"' : "") + "><td>" + amt(s * 100) + " %</td><td>" + dollars(s * btcCap) + "</td><td>" + dollars(x) + "</td><td>" + dollars(p.day * x) + "</td></tr>";
      }).join("") : "";

      // Join now: your GPUs added to the network's live hashrate, sharing today's block reward.
      var H = Number(L.hashrate) || 0, gpus = H / REF.hashrate, D = Math.round(pick("jd", "jdc", ".mc-cjd"));
      var perDay = rewardAt(h || 1) / 1e8 * 86400 / BLOCK_SECS;
      var join = function (d) {
        var mine = d * REF.hashrate, sh = mine / (H + mine), day = perDay * sh;
        return { sh: sh, day: day, secs: day > 0 ? 86400 / day : null };
      };
      var pct = function (x) { return x * 100 >= 0.01 ? amt(x * 100) + " %" : "< 0.01 %"; };
      var hs = function (v) { return v >= 1e12 ? amt(v / 1e12) + " TH/s" : v >= 1e9 ? amt(v / 1e9) + " GH/s" : v >= 1e6 ? amt(v / 1e6) + " MH/s" : v >= 1e3 ? amt(v / 1e3) + " kH/s" : amt(v) + " H/s"; };
      var ok = H > 0 && h > 0, j = join(D);
      q("js").textContent = ok ? "Live from the blockchain: the network hashes at " + hs(H) + ", about " + amt(gpus) + " " + REF.name + "-class GPUs on " + nNow.toLocaleString("en-US") +
        " computer" + (nNow === 1 ? "" : "s") + ", and shares " + amt(perDay) + " GNR a day. Add your own GPUs and see your share." : "Waiting for the network hashrate…";
      q("jsh").textContent = ok ? pct(j.sh) : "—";
      q("jshe").textContent = ok ? D.toLocaleString("en-US") + " GPU" + (D === 1 ? "" : "s") + " added to the " + amt(gpus) + " mining now" : "—";
      q("jg").textContent = ok ? amt(j.day) + " GNR a day" : "—";
      q("jge").textContent = ok ? amt(j.day * 30.44) + " a month · " + amt(j.day * 365.25) + " a year" : "—";
      q("jt").textContent = ok ? span(j.secs) : "—";
      q("jv").textContent = ok && mkt != null ? dollars(j.day * mkt) + " a day" : "—";
      q("jve").textContent = mkt != null ? (ok ? dollars(j.day * 30.44 * mkt) + " a month · " : "") + "at " + usd(mkt) + " a GNR" : "no market price yet: it appears with the first swaps";
      q("jph").textContent = "A day at " + dollars(price) + " (what if, below)";
      q("jrows").innerHTML = ok ? joins.map(function (d) {
        var x = join(d);
        return "<tr" + (d === D ? ' class="on"' : "") + "><td>" + d.toLocaleString("en-US") + "</td><td>" + pct(x.sh) + "</td><td>" + amt(x.day) + " GNR</td><td>" + span(x.secs) + "</td><td>" +
          (mkt != null ? dollars(x.day * mkt) : "—") + "</td><td>" + dollars(x.day * price) + "</td></tr>";
      }).join("") : "";
    }
    ["jd", "jdc", "y", "s", "c", "ws", "wsc", "bs", "pp", "ppc", "ph", "phc"].forEach(function (k) { q(k).addEventListener("input", draw); q(k).addEventListener("change", draw); });
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

  window.GKValue = { REF: REF, perGpu: perGpu, rewardAt: rewardAt, perComputer: perComputer, supplyAt: supplyAt, gnrScarcity: gnrScarcity, btcScarcity: btcScarcity, mountCalc: mountCalc, market: market, usd: usd, approx: approx, btc: btc, coin: coin, sprite: sprite };
})();
