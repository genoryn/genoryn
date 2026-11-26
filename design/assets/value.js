/* GNR prices and coin marks, shared by genoryn.io and the wallet. The price of GNR is the
   market price of the offer board only: what people pay each other in atomic swaps. */
(function () {
  // One RTX 5090 does about 74 MH/s on KryptoHash (used for "what one GPU mines a day").
  var REF = { name: "RTX 5090", hashrate: 74e6 };
  var BLOCKS_PER_DAY = 1440;
  var lang = function () { return window.GKI18N ? GKI18N.lang : "en"; };
  var num = function (v, o) { return window.GKI18N ? GKI18N.num(v, o) : Number(v).toLocaleString("en-US", o); };
  function fmt(v, o) {
    try { return new Intl.NumberFormat(lang(), o).format(v); } catch (e) {}
    try { delete o.currencyDisplay; return new Intl.NumberFormat(lang(), o).format(v); } catch (e) {}
    return (o.style === "currency" ? "$" : "") + Number(v).toLocaleString("en-US", { maximumFractionDigits: 2 });
  }

  /* GNR one REF GPU mines a day at the network hashrate (H/s); subsidy in krypton per block. */
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
    if (s == null || !isFinite(s)) return T("never");
    var r = function (v) { return v < 10 ? Math.round(v * 10) / 10 : Math.round(v); };
    var o = function (v) { return { n: num(v) }; };
    if (s < 60) { var a = r(s); return Tn(a, "{n} second", "{n} seconds", o(a)); }
    if (s < 3600) { var b = r(s / 60); return Tn(b, "{n} minute", "{n} minutes", o(b)); }
    if (s < 86400) { var c = r(s / 3600); return Tn(c, "{n} hour", "{n} hours", o(c)); }
    if (s < 86400 * 365.25) { var d = r(s / 86400); return Tn(d, "{n} day", "{n} days", o(d)); }
    var y = r(s / (86400 * 365.25));
    return Tn(y, "{n} year", "{n} years", o(y));
  }
  function digits(v) { return v < 0.01 ? 6 : v < 1 ? 4 : v < 100 ? 2 : 0; }
  function amt(v) { return num(v, { maximumFractionDigits: digits(v) }); }

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
  /* Millions and more in words, in the page's language ("21 trillion", "21 Billionen", "21兆"). */
  function big(v, money) {
    if (!(v >= 1e6)) return null;
    var o = { notation: "compact", compactDisplay: "long", maximumFractionDigits: digits(v / Math.pow(10, Math.floor(Math.log10(v) / 3) * 3)) };
    if (money) { o.style = "currency"; o.currency = "USD"; o.currencyDisplay = "narrowSymbol"; }
    return fmt(v, o);
  }
  function dollars(v) { return big(v, true) || (v >= 100 ? fmt(Math.round(v), { style: "currency", currency: "USD", currencyDisplay: "narrowSymbol", maximumFractionDigits: 0 }) : usd(v)); }
  function count(v) { return big(v) || num(Math.round(v)); }
  function options(list, sel, f) {
    return list.map(function (v) { return '<option value="' + v + '"' + (v === sel ? " selected" : "") + ">" + f(v) + "</option>"; }).join("");
  }
  var esc = function (s) { return String(s).replace(/[&<>"']/g, function (c) { return { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]; }); };

  /* The miner and price calculator, the same on genoryn.io and in the wallet. live() returns
     { height, hashrate (H/s), supply (krypton), sat (market sat per GNR), btcUsd } from the node. */
  function mountCalc(root, live) {
    var now = new Date(), y0 = now.getFullYear();
    var years = "";
    for (var k = 0; k <= 40; k++) years += '<option value="' + k + '">' + esc(k ? Tn(k, "{year} (in {n} year)", "{year} (in {n} years)", { year: y0 + k }) : T("Now ({year})", { year: y0 })) + "</option>";
    var sizes = [100, 1000, 100000, 1000000, 10000000];
    var people = [1000, 10000, 100000, 1000000, 10000000, 100000000];
    var holds = [100, 1000, 10000, 100000];
    var joins = [1, 10, 100, 1000, 10000];
    var shares = [0.0001, 0.001, 0.01, 0.1, 0.5, 1];
    var worths = [0.001, 0.003, 0.01, 0.1, 1];
    var sfYears = [0, 4, 8, 12, 16, 20, 30, 40];
    var fmtN = function (n) { return num(n); };
    var box = function (label, key, sub, cls) { return "<div" + (cls ? ' class="' + cls + '"' : "") + "><span>" + label + '</span><b data-c="' + key + '">—</b><em' + (sub == null ? ' data-c="' + key + 'e">—' : ">" + sub) + "</em></div>"; };
    var head = function (cols) { return "<thead><tr>" + cols.map(function (c) { return "<th>" + c + "</th>"; }).join("") + "</tr></thead>"; };
    root.innerHTML =
      '<div class="mc-now">' +
      '<div><span>' + TE("Block now") + '</span><b data-c="h">—</b><em data-c="hr">—</em></div>' +
      box(TE("Network hashrate"), "n", TE("measured from the shares of every block")) +
      '<div><span>' + TE("One {gpu} mines now", { gpu: REF.name }) + '</span><b data-c="d">—</b><em data-c="dm">—</em></div>' +
      box(TE("1 GNR takes one {gpu}", { gpu: REF.name }), "t", TE("on average, right now"), "mc-hl") + "</div>" +
      '<div class="mc-px mc-first">' +
      '<h4 class="mc-h">' + TE("Join now: what your computers would mine") + "</h4>" +
      '<p class="mc-sub" data-c="js">—</p>' +
      '<div class="mc-in"><label>' + TE("Your GPUs (each like one {gpu}, {rate} MH/s)", { gpu: REF.name, rate: REF.hashrate / 1e6 }) + '<select data-c="jd">' + options(joins, 1, fmtN) + '<option value="custom">' + TE("Other number") + "</option></select></label>" +
      '<label class="mc-cjd" style="display:none">' + TE("Number of GPUs") + '<input data-c="jdc" type="number" min="1" step="1" inputmode="numeric" value="50"></label></div>' +
      '<div class="mc-now">' +
      '<div><span>' + TE("Your share of the network") + '</span><b data-c="jsh">—</b><em data-c="jshe">—</em></div>' +
      '<div><span>' + TE("You would mine") + '</span><b data-c="jg">—</b><em data-c="jge">—</em></div>' +
      box(TE("1 GNR would take you"), "jt", TE("on average, at today's hashrate"), "mc-hl") +
      '<div><span>' + TE("Worth at the market price") + '</span><b data-c="jv">—</b><em data-c="jve">—</em></div></div>' +
      '<div class="mc-tw"><table class="mc-table"><thead><tr><th>' + TE("Your GPUs") + "</th><th>" + TE("Your share") + "</th><th>" + TE("GNR a day") + "</th><th>" + TE("1 GNR takes") + "</th><th>" + TE("A day at the market") + '</th><th data-c="jph">' + TE("A day at the what-if price") + '</th></tr></thead><tbody data-c="jrows"></tbody></table></div>' +
      '<p class="mc-note">' + TE("The more GPUs join, the smaller each one's share: your GPUs are added to everyone's, and every block's reward is split by the work. GPUs of other kinds count by their hashrate.") + "</p></div>" +
      '<h4 class="mc-h">' + TE("The future: by year and number of computers") + "</h4>" +
      '<div class="mc-in"><label>' + TE("When") + '<select data-c="y">' + years + "</select></label>" +
      '<label>' + TE("Computers mining") + '<select data-c="s">' + options(sizes, 100, fmtN) +
      '<option value="now">' + TE("As much hashrate as now") + '</option><option value="custom">' + TE("Other number") + "</option></select></label>" +
      '<label class="mc-custom" style="display:none">' + TE("Number of computers") + '<input data-c="c" type="number" min="1" step="1" inputmode="numeric" value="50000"></label></div>' +
      '<div class="mc-res" data-c="r"></div>' +
      '<div class="mc-tw"><table class="mc-table">' + head([TE("Computers mining"), TE("Each one a day"), TE("a month"), TE("a year"), TE("1 GNR takes one computer")]) + '<tbody data-c="rows"></tbody></table></div>' +
      '<p class="mc-note">' + TE("Every block pays its reward to all miners together, split by the work each one does; here every computer is equally fast. One block a minute; the reward starts at 5 GNR and halves every 2,100,000 blocks (about every 4 years); at most 21,000,000 GNR will ever exist. Electricity and hardware are not counted.") + "</p>" +
      '<div class="mc-px">' +
      '<h4 class="mc-h">' + TE("Scarcity: how rare GNR becomes") + "</h4>" +
      '<p class="mc-sub">' + TE("Stock-to-flow: the GNR that exist divided by the new GNR mined in the next year, so how many years of mining are already stored. The higher it is, the rarer the coin. Gold is about {gold}. Every halving doubles it, and there will never be more than 21,000,000 GNR.", { gold: GOLD_SF }) + "</p>" +
      '<div class="mc-now">' +
      '<div><span>' + TE("Scarcity now") + '</span><b data-c="sf0">—</b><em data-c="sf0e">—</em></div>' +
      '<div><span>' + TE("Scarcity in the year above") + '</span><b data-c="sfy">—</b><em data-c="sfye">—</em></div>' +
      box(TE("As scarce as gold"), "sfg", TE("stock-to-flow {n}", { n: GOLD_SF }), "mc-hl") +
      '<div class="mc-hl"><span>' + TE("As scarce as Bitcoin is today") + '</span><b data-c="sfb">—</b><em data-c="sfbe">—</em></div></div>' +
      '<div class="mc-tw"><table class="mc-table">' + head([TE("Year"), TE("GNR that exist"), TE("New GNR that year"), TE("GNR scarcity"), TE("Bitcoin then"), TE("Gold")]) + '<tbody data-c="sfrows"></tbody></table></div>' +
      '<p class="mc-note">' + TE("Scarcity is a fact of the schedule; it does not set a price by itself. A coin is worth something only while people want it: scarcity decides how much of it each of them can have.") + "</p></div>" +
      '<div class="mc-px">' +
      '<h4 class="mc-h">' + TE("Price calculator: what if") + "</h4>" +
      '<p class="mc-sub">' + TE("Three simple ways to put a number on GNR in the year and with the computers chosen above. None is a forecast; each shows what one assumption means per coin.") + "</p>" +
      '<h5 class="mc-h5">' + TE("1. The work behind one GNR") + "</h5>" +
      '<p class="mc-sub">' + TE("GNR only comes from mining. The more computers share the same reward, and the more halvings have passed, the more work one GNR takes. If a day of a computer's mining stays worth what it is worth today, one GNR is worth more in proportion.") + "</p>" +
      '<div class="mc-in"><label>' + TE("One GNR is worth today") + '<select data-c="ws"><option value="market">' + TE("the market price") + "</option>" + options(worths, -1, function (v) { return TE("{price} (1,000 GNR for {thousand})", { price: usd(v), thousand: dollars(v * 1000) }); }) + '<option value="custom">' + TE("Other amount") + "</option></select></label>" +
      '<label class="mc-cws" style="display:none">' + TE("Dollars for one GNR today") + '<input data-c="wsc" type="number" min="0" step="any" inputmode="decimal" value="0.003"></label></div>' +
      '<div class="mc-res" data-c="wres"></div>' +
      '<div class="mc-tw"><table class="mc-table">' + head([TE("Computers mining"), TE("Each one a day"), TE("Work behind 1 GNR"), TE("One GNR worth"), TE("1,000 GNR worth")]) + '<tbody data-c="wrows"></tbody></table></div>' +
      '<h5 class="mc-h5">' + TE("2. A share of Bitcoin's value") + "</h5>" +
      '<p class="mc-sub" data-c="bsub">—</p>' +
      '<div class="mc-in"><label>' + TE("GNR worth, as a share of Bitcoin today") + '<select data-c="bs">' + options(shares, 0.01, function (v) { return amt(v * 100) + " %"; }) + "</select></label></div>" +
      '<div class="mc-res" data-c="bres"></div>' +
      '<div class="mc-tw"><table class="mc-table">' + head([TE("Share of Bitcoin's value"), TE("All GNR worth"), TE("One GNR worth"), TE("One computer a day")]) + '<tbody data-c="brows"></tbody></table></div>' +
      '<h5 class="mc-h5">' + TE("3. People and dollars") + "</h5>" +
      '<p class="mc-sub">' + TE("A price is what people pay. If a number of people each hold GNR worth some dollars, one GNR is worth all their dollars divided by the GNR that exist then. The reward halves while more people arrive, so each person can own less and less.") + "</p>" +
      '<div class="mc-in mc-in4"><label>' + TE("People holding GNR") + '<select data-c="pp">' + options(people, 10000, fmtN) + '<option value="custom">' + TE("Other number") + "</option></select></label>" +
      '<label class="mc-cpp" style="display:none">' + TE("Number of people") + '<input data-c="ppc" type="number" min="1" step="1" inputmode="numeric" value="50000"></label>' +
      '<label>' + TE("Each one holds") + '<select data-c="ph">' + options(holds, 1000, function (v) { return dollars(v); }) + '<option value="custom">' + TE("Other amount") + "</option></select></label>" +
      '<label class="mc-cph" style="display:none">' + TE("Dollars each") + '<input data-c="phc" type="number" min="1" step="1" inputmode="decimal" value="500"></label></div>' +
      '<div class="mc-now">' +
      '<div><span>' + TE("GNR that exist then") + '</span><b data-c="sp">—</b><em data-c="spe">—</em></div>' +
      '<div><span>' + TE("GNR for each person") + '</span><b data-c="pe">—</b><em data-c="pee">—</em></div>' +
      '<div class="mc-hl"><span>' + TE("One GNR would be worth") + '</span><b data-c="pr">—</b><em data-c="pre">—</em></div>' +
      '<div><span>' + TE("Each computer would earn") + '</span><b data-c="pc">—</b><em data-c="pce">—</em></div></div>' +
      '<div class="mc-res" data-c="pres"></div>' +
      '<div class="mc-tw"><table class="mc-table">' + head([TE("People holding GNR"), TE("GNR each"), TE("One GNR worth"), TE("All GNR worth"), TE("One computer a day")]) + '<tbody data-c="prows"></tbody></table></div>' +
      '<p class="mc-note" data-c="mk"></p>' +
      '<p class="mc-note">' + TE("A what-if calculation, not a forecast and not financial advice: nobody knows the future price. The real price is only what people pay each other on the market.") + "</p></div>";
    var q = function (k) { return root.querySelector('[data-c="' + k + '"]'); };
    var show = function (cls, on) { root.querySelector(cls).style.display = on ? "" : "none"; };
    function pick(sel, input, cls) {
      var v = q(sel).value;
      show(cls, v === "custom");
      return v === "custom" ? Math.max(1, Number(q(input).value) || 1) : Number(v);
    }
    function draw() {
      // "Computers" are computers as fast as one REF GPU: the network's hashrate is measured from
      // its shares, while how many machines produce it is known to nobody.
      var L = live() || {}, h = Number(L.height) || 0, H = Number(L.hashrate) || 0, nNow = H > 0 ? Math.max(1, H / REF.hashrate) : 1;
      var hs = function (v) { return v >= 1e12 ? amt(v / 1e12) + " TH/s" : v >= 1e9 ? amt(v / 1e9) + " GH/s" : v >= 1e6 ? amt(v / 1e6) + " MH/s" : v >= 1e3 ? amt(v / 1e3) + " kH/s" : amt(v) + " H/s"; };
      var k = Number(q("y").value), at = h + Math.round(k * 365.25 * 86400 / BLOCK_SECS);
      var sel = q("s").value;
      show(".mc-custom", sel === "custom");
      var n = sel === "now" ? Math.max(1, Math.round(nNow)) : sel === "custom" ? Math.max(1, Math.round(Number(q("c").value) || 1)) : Number(sel);
      var btcUsd = Number(L.btcUsd) || 0, mkt = market(L.sat, btcUsd);
      var p0 = perComputer(nNow, h || 1);
      var yearName = k ? String(y0 + k) : T("Now");
      q("h").textContent = h ? "#" + num(h) : "—";
      q("hr").textContent = h ? T("{r} GNR a block · next halving at #{h}", { r: amt(p0.reward), h: num((Math.floor(Math.max(0, h - 1) / HALVING) + 1) * HALVING + 1) }) : "—";
      q("n").textContent = H > 0 ? hs(H) : "—";
      q("d").textContent = h ? T("{gnr} GNR a day", { gnr: amt(p0.day) }) : "—";
      q("dm").textContent = h ? T("{m} a month · {y} a year", { m: amt(p0.month), y: amt(p0.year) }) + (mkt != null ? " · " + T("≈ {usd} a day now", { usd: dollars(p0.day * mkt) }) : "") : "—";
      q("t").textContent = h ? span(p0.secsPer1) : "—";
      var p = perComputer(n, at || 1);
      q("r").innerHTML = TH("**{year}**: block ≈ #{h}, {r} GNR a block. With **{n}** computers each mines **{day} GNR a day** ({month} a month, {yr} a year): one GNR every **{every}**.",
        { year: yearName, h: num(Math.max(1, at)), r: amt(p.reward), n: num(n), day: amt(p.day), month: amt(p.month), yr: amt(p.year), every: span(p.secsPer1) }) +
        (mkt != null ? " " + TH("At today's market price ({price}) that is ≈ {usd} a day.", { price: usd(mkt), usd: dollars(p.day * mkt) }) : "");
      q("rows").innerHTML = sizes.map(function (m) {
        var x = perComputer(m, at || 1);
        return "<tr" + (m === n ? ' class="on"' : "") + "><td>" + num(m) + "</td><td>" + amt(x.day) + " GNR</td><td>" + amt(x.month) + "</td><td>" + amt(x.year) + "</td><td>" + esc(span(x.secsPer1)) + "</td></tr>";
      }).join("");

      // Price: the dollars people hold divided by the GNR that exist in the chosen year.
      var who = Math.round(pick("pp", "ppc", ".mc-cpp")), each = pick("ph", "phc", ".mc-cph");
      var liveSupply = Number(L.supply) > 0 ? Number(L.supply) / 1e8 : supplyAt(h);
      var coins = Math.max(liveSupply + supplyAt(at) - supplyAt(h), 1e-8);
      var price = who * each / coins, btcCap = btcUsd * BTC_SUPPLY;
      var vsBtc = function (cap) {
        if (!(btcCap > 0)) return "";
        var r = cap / btcCap;
        return r >= 0.995 ? T("{x}× all Bitcoin today", { x: amt(r) }) : T("{pct} % of all Bitcoin today", { pct: r * 100 >= 0.01 ? amt(r * 100) : "< " + num(0.01) });
      };
      q("sp").textContent = count(coins);
      q("spe").textContent = T("{pct} % of the 21,000,000 there will ever be", { pct: amt(coins / 21e4) });
      q("pe").textContent = amt(coins / who) + " GNR";
      q("pee").textContent = T("if {n} people shared all of it", { n: num(who) });
      q("pr").textContent = dollars(price);
      q("pre").textContent = btcUsd > 0 ? btc(price / btcUsd * 1e8) + (mkt != null ? " · " + T("{x}× today's market price", { x: amt(price / mkt) }) : "") : "—";
      q("pc").textContent = T("{usd} a day", { usd: dollars(p.day * price) });
      q("pce").textContent = T("with {n} computers mining in {year}", { n: num(n), year: k ? y0 + k : y0 });
      q("pres").innerHTML = TH("**{year}**: about **{coins} GNR** exist. If **{who}** people each hold **{each}** of GNR, that is {total} for {coins} GNR: **{price} a GNR**. All GNR together would be worth {cap}.",
        { year: yearName, coins: count(coins), who: num(who), each: dollars(each), total: dollars(who * each), price: dollars(price), cap: dollars(price * coins) + (btcCap > 0 ? " (" + vsBtc(price * coins) + ")" : "") });
      q("prows").innerHTML = people.map(function (m) {
        var x = m * each / coins;
        return "<tr" + (m === who ? ' class="on"' : "") + "><td>" + num(m) + "</td><td>" + amt(coins / m) + "</td><td>" + dollars(x) + "</td><td>" + dollars(x * coins) + "</td><td>" + dollars(p.day * x) + "</td></tr>";
      }).join("");
      var full = 1e6 * 21e6;
      q("mk").textContent = (mkt != null ? T("Market price now: {usd} a GNR ({btc}), from real swaps on the offer board; all {mined} GNR mined so far are worth {worth}.", { usd: usd(mkt), btc: btc(L.sat), mined: count(liveSupply), worth: dollars(mkt * liveSupply) }) + " "
        : T("No market price yet: it appears with the first swaps.") + " ") +
        T("For scale: at {million} a GNR, all 21,000,000 GNR would be worth {all}.", { million: dollars(1e6), all: dollars(full) + (btcCap > 0 ? " (" + vsBtc(full) + ")" : "") });

      // Scarcity by the two schedules, from today's block (GNR) and today's date (Bitcoin).
      var hh = Math.max(1, h), yearMs = 365.25 * 864e5, t0 = now.getTime();
      var sfTxt = function (v) { return !isFinite(v) ? "∞" : v < 1 ? amt(v) : num(Math.round(v)); };
      var sfNow = gnrScarcity(hh), sfAt = gnrScarcity(Math.max(1, at)), btcNow = btcScarcity(t0);
      var newYear = function (x) { return supplyAt(x + GNR_YEAR) - supplyAt(x); };
      var when = function (target) {
        for (var y = 0; y <= 80; y += 0.25) if (gnrScarcity(hh + y * GNR_YEAR) >= target) return y;
        return null;
      };
      var yearOf = function (y) { return y == null ? T("after {year}", { year: y0 + 80 }) : y === 0 ? T("already") : T("around {year}", { year: new Date(t0 + y * yearMs).getFullYear() }); };
      q("sf0").textContent = sfTxt(sfNow);
      q("sf0e").textContent = T("{have} GNR exist · {fresh} new in a year", { have: count(supplyAt(hh)), fresh: count(newYear(hh)) });
      q("sfy").textContent = sfTxt(sfAt);
      q("sfye").textContent = T("{year}: {have} GNR · {fresh} new a year", { year: k ? y0 + k : T("now"), have: count(supplyAt(Math.max(1, at))), fresh: count(newYear(Math.max(1, at))) });
      q("sfg").textContent = yearOf(when(GOLD_SF));
      q("sfb").textContent = yearOf(when(btcNow));
      q("sfbe").textContent = T("stock-to-flow {n}", { n: sfTxt(btcNow) });
      q("sfrows").innerHTML = sfYears.map(function (y) {
        var x = hh + Math.round(y * GNR_YEAR);
        return "<tr" + (y === k ? ' class="on"' : "") + "><td>" + (y ? y0 + y : esc(T("Now"))) + "</td><td>" + count(supplyAt(x)) + "</td><td>" + count(newYear(x)) + "</td><td>" + sfTxt(gnrScarcity(x)) +
          "</td><td>" + sfTxt(btcScarcity(t0 + y * yearMs)) + "</td><td>≈ " + GOLD_SF + "</td></tr>";
      }).join("");

      // The work behind one GNR: today's price × how much less one computer mines then.
      var wsel = q("ws").value;
      show(".mc-cws", wsel === "custom");
      var px0 = wsel === "market" ? (mkt != null ? mkt : 0.003) : wsel === "custom" ? Math.max(0, Number(q("wsc").value) || 0) : Number(wsel);
      var dayNow = perComputer(nNow, h || 1).day;
      var workPx = function (m) { var d = perComputer(m, at || 1).day; return d > 0 ? px0 * dayNow / d : null; };
      var wp = workPx(n);
      q("wres").innerHTML = TH("Today one {gpu} mines **{day} GNR a day**, worth {worth} at {price} a GNR", { gpu: REF.name, day: amt(dayNow), worth: dollars(dayNow * px0), price: usd(px0) }) +
        (wsel === "market" && mkt == null ? " " + TH("(no market price yet: {usd} assumed)", { usd: usd(0.003) }) : "") + ". " +
        TH("**{year}** with **{n}** computers it mines {day} GNR a day, so one GNR takes **{x}× the work**. For that day of mining to be worth the same, one GNR would be worth **{one}** and 1,000 GNR {thousand}.",
          { year: yearName, n: num(n), day: amt(p.day), x: amt(dayNow / Math.max(p.day, 1e-18)), one: wp != null ? dollars(wp) : "—", thousand: wp != null ? dollars(wp * 1000) : "—" });
      q("wrows").innerHTML = sizes.map(function (m) {
        var x = workPx(m), d = perComputer(m, at || 1).day;
        return "<tr" + (m === n ? ' class="on"' : "") + "><td>" + num(m) + "</td><td>" + amt(d) + " GNR</td><td>" + amt(dayNow / Math.max(d, 1e-18)) + "×</td><td>" + (x != null ? dollars(x) : "—") + "</td><td>" + (x != null ? dollars(x * 1000) : "—") + "</td></tr>";
      }).join("");

      // A share of Bitcoin's value today, spread over the GNR that exist in the chosen year.
      var share = Number(q("bs").value) || 0.01;
      q("bsub").textContent = btcCap > 0 ? T("Bitcoin is worth about {cap} today ({price} × {supply} BTC). If all GNR together became worth a part of that, one GNR would be that part divided by the GNR that exist then.", { cap: dollars(btcCap), price: dollars(btcUsd), supply: count(BTC_SUPPLY) })
        : T("Waiting for the Bitcoin price…");
      var sharePx = function (s) { return btcCap > 0 ? s * btcCap / coins : null; };
      var bp = sharePx(share);
      q("bres").innerHTML = bp == null ? "—" : TH("**{year}**: at **{pct} %** of Bitcoin's value ({cap}) and {coins} GNR, one GNR would be worth **{price}**{vs}, and each of {n} computers would earn {day} a day.",
        { year: yearName, pct: amt(share * 100), cap: dollars(share * btcCap), coins: count(coins), price: dollars(bp), vs: mkt != null ? " (" + T("{x}× today's market price", { x: amt(bp / mkt) }) + ")" : "", n: num(n), day: dollars(p.day * bp) });
      q("brows").innerHTML = btcCap > 0 ? shares.map(function (s) {
        var x = sharePx(s);
        return "<tr" + (s === share ? ' class="on"' : "") + "><td>" + amt(s * 100) + " %</td><td>" + dollars(s * btcCap) + "</td><td>" + dollars(x) + "</td><td>" + dollars(p.day * x) + "</td></tr>";
      }).join("") : "";

      // Join now: your GPUs added to the network's live hashrate, sharing today's block reward.
      var D = Math.round(pick("jd", "jdc", ".mc-cjd"));
      var perDay = rewardAt(h || 1) / 1e8 * 86400 / BLOCK_SECS;
      var join = function (d) {
        var mine = d * REF.hashrate, sh = mine / (H + mine), day = perDay * sh;
        return { sh: sh, day: day, secs: day > 0 ? 86400 / day : null };
      };
      var pct = function (x) { return x * 100 >= 0.01 ? amt(x * 100) + " %" : "< " + num(0.01) + " %"; };
      var ok = H > 0 && h > 0, j = join(D);
      q("js").textContent = ok ? T("Live from the blockchain: the network hashes at {rate} and shares {day} GNR a day. Add your own GPUs and see your share.", { rate: hs(H), day: amt(perDay) }) : T("Waiting for the network hashrate…");
      q("jsh").textContent = ok ? pct(j.sh) : "—";
      q("jshe").textContent = ok ? Tn(D, "{n} GPU added to today's {rate}", "{n} GPUs added to today's {rate}", { n: num(D), rate: hs(H) }) : "—";
      q("jg").textContent = ok ? T("{gnr} GNR a day", { gnr: amt(j.day) }) : "—";
      q("jge").textContent = ok ? T("{m} a month · {y} a year", { m: amt(j.day * 30.44), y: amt(j.day * 365.25) }) : "—";
      q("jt").textContent = ok ? span(j.secs) : "—";
      q("jv").textContent = ok && mkt != null ? T("{usd} a day", { usd: dollars(j.day * mkt) }) : "—";
      q("jve").textContent = mkt != null ? (ok ? T("{usd} a month", { usd: dollars(j.day * 30.44 * mkt) }) + " · " : "") + T("at {price} a GNR", { price: usd(mkt) }) : T("no market price yet: it appears with the first swaps");
      q("jph").textContent = T("A day at {price} (what if, below)", { price: dollars(price) });
      q("jrows").innerHTML = ok ? joins.map(function (d) {
        var x = join(d);
        return "<tr" + (d === D ? ' class="on"' : "") + "><td>" + num(d) + "</td><td>" + pct(x.sh) + "</td><td>" + amt(x.day) + " GNR</td><td>" + esc(span(x.secs)) + "</td><td>" +
          (mkt != null ? dollars(x.day * mkt) : "—") + "</td><td>" + dollars(x.day * price) + "</td></tr>";
      }).join("") : "";
    }
    ["jd", "jdc", "y", "s", "c", "ws", "wsc", "bs", "pp", "ppc", "ph", "phc"].forEach(function (k) { q(k).addEventListener("input", draw); q(k).addEventListener("change", draw); });
    draw();
    return { update: draw };
  }

  /* $0.0031, $0.40, $1,234.56 (in the page's language); below a hundredth of a cent "< $0.0001". */
  function usd(v) {
    v = Number(v) || 0;
    var money = function (x, o) { o.style = "currency"; o.currency = "USD"; o.currencyDisplay = "narrowSymbol"; return fmt(x, o); };
    if (v <= 0) return money(0, { maximumFractionDigits: 0 });
    if (v < 1e-4) return "< " + money(0.0001, { maximumFractionDigits: 4 });
    if (v < 0.01) return money(v, { maximumSignificantDigits: 2 });
    return money(v, { minimumFractionDigits: 2, maximumFractionDigits: 2 });
  }

  /* A price in satoshis shown in bitcoin: 9700 -> "0.000097 BTC", 1e8 -> "1 BTC". */
  function btc(sat) {
    if (sat == null || !isFinite(Number(sat))) return "—";
    var v = Number(sat) / 1e8;
    if (v === 0) return "0 BTC";
    if (v < 1e-8) return "< " + num(1e-8, { maximumFractionDigits: 8 }) + " BTC";
    return num(v, { maximumFractionDigits: v >= 1 ? 4 : 8 }) + " BTC";
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

  /* Every amount written as "<number> GNR" or "<number> BTC" gets its coin mark in front, also
     amounts scripts write later. Only an icon is inserted, never text, so what a script reads
     back from an element (textContent) stays the same. Inputs, code, addresses and anything
     marked data-nocoin are left alone; an amount that already has a mark in front keeps it. */
  var AMOUNT = /[−+\-≈~]?\s?\d(?:[\d.,'\u00a0\u202f ]*\d)?[\u00a0 ]?(?:GNR|BTC)(?![A-Za-z])/g;
  var NOCOIN = "script,style,textarea,input,select,option,pre,code,svg,title,.mono,[data-nocoin],.coin-free";
  function markText(node) {
    var t = node.nodeValue, p = node.parentNode;
    if (!t || (t.indexOf("GNR") < 0 && t.indexOf("BTC") < 0) || !p || p.nodeType !== 1 || (p.closest && p.closest(NOCOIN))) return;
    AMOUNT.lastIndex = 0;
    var m, last = 0, out = null;
    while ((m = AMOUNT.exec(t))) {
      var at = m.index;
      while (at < t.length && /\s/.test(t.charAt(at))) at++;
      var before = at === 0 ? node.previousSibling : null;
      if (before && before.nodeType === 1 && before.classList && before.classList.contains("coin")) continue;
      if (at > 0 && /[\w.]/.test(t.charAt(at - 1))) continue;
      out = out || document.createDocumentFragment();
      if (at > last) out.appendChild(document.createTextNode(t.slice(last, at)));
      var kind = m[0].slice(-3) === "BTC" ? "btc" : "gnr";
      var s = document.createElementNS("http://www.w3.org/2000/svg", "svg");
      s.setAttribute("class", "coin cm cm-" + kind + " coin-" + kind);
      s.setAttribute("aria-hidden", "true");
      var u = document.createElementNS("http://www.w3.org/2000/svg", "use");
      u.setAttribute("href", "#c-" + kind);
      s.appendChild(u);
      out.appendChild(s);
      last = at;
    }
    if (!out) return;
    out.appendChild(document.createTextNode(t.slice(last)));
    p.replaceChild(out, node);
  }
  function markCoins(root) {
    if (!root) return;
    if (root.nodeType === 3) { markText(root); return; }
    if (root.nodeType !== 1 || (root.closest && root.closest(NOCOIN))) return;
    var w = document.createTreeWalker(root, NodeFilter.SHOW_TEXT), list = [];
    for (var n = w.nextNode(); n; n = w.nextNode()) list.push(n);
    list.forEach(markText);
  }
  function watchCoins() {
    if (!document.body || !window.MutationObserver || watchCoins.on) return;
    watchCoins.on = true;
    if (!document.getElementById("gk-coin-css")) {
      var st = document.createElement("style");
      st.id = "gk-coin-css";
      st.textContent = ".cm{width:1.05em;height:1.05em;margin:0 .3em 0 .05em;vertical-align:-.17em;flex:none}" +
        ".cm-gnr{filter:drop-shadow(0 0 5px color-mix(in srgb,currentColor 38%,transparent))}.cm-btc{opacity:.82}";
      document.head.appendChild(st);
    }
    var todo = [], queued = false;
    var mo = new MutationObserver(function (ms) {
      ms.forEach(function (m) {
        if (m.type === "characterData") todo.push(m.target);
        else for (var i = 0; i < m.addedNodes.length; i++) todo.push(m.addedNodes[i]);
      });
      if (queued) return;
      queued = true;
      requestAnimationFrame(function () {
        queued = false;
        var l = todo; todo = [];
        mo.disconnect();
        l.forEach(function (n) { if (n.isConnected) markCoins(n); });
        mo.observe(document.body, { childList: true, subtree: true, characterData: true });
      });
    });
    markCoins(document.body);
    mo.observe(document.body, { childList: true, subtree: true, characterData: true });
  }
  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", watchCoins); else watchCoins();

  window.GKValue = { markCoins: markCoins, REF: REF, perGpu: perGpu, rewardAt: rewardAt, perComputer: perComputer, supplyAt: supplyAt, gnrScarcity: gnrScarcity, btcScarcity: btcScarcity, mountCalc: mountCalc, market: market, usd: usd, approx: approx, btc: btc, coin: coin, sprite: sprite };
})();
