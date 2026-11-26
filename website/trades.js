/* genoryn.io/trades: every completed GNR swap, from the node's /v1/swap/history API. The table
   comes a page at a time, the per-swap chart shows the newest swaps of the period, and every
   other figure comes from the history's day-by-day summary. */
(function () {
  var $ = function (id) { return document.getElementById(id); };
  var e = function (s) { return gk.esc(s); };
  var st = { range: 30, page: 0, btc: 0, sum: null, swaps: [], total: 0, charts: {} };
  var CHART_SWAPS = 1000;
  var sat = function (p) { return p == null || !isFinite(p) ? "—" : GKValue.btc(p); };
  var usd = function (p) { return st.btc > 0 && p != null && isFinite(p) ? GKValue.usd(p * st.btc / 1e8) : ""; };
  var dollars = function (s) { return st.btc > 0 ? GKValue.usd(Number(s || 0) * st.btc / 1e8) : ""; };
  var gnrShort = function (v) { v = Number(v || 0); return GKI18N.num(v / 1e8, { maximumFractionDigits: v >= 1e10 ? 0 : 2 }); };
  var gnr = function (v) { return GKI18N.num(Number(v || 0) / 1e8, { maximumFractionDigits: 8 }); };
  var btcShort = function (v) { v = Number(v || 0); return GKI18N.num(v / 1e8, { maximumFractionDigits: v >= 1e8 ? 2 : v >= 1e6 ? 4 : 6 }); };
  var btc = function (v) { return GKI18N.num(Number(v || 0) / 1e8, { minimumFractionDigits: 4, maximumFractionDigits: 8 }); };
  var main = function (p) { return p == null || !isFinite(p) ? "—" : st.btc > 0 ? usd(p) : sat(p); };
  var when = function (t) {
    try { return new Date(t * 1000).toLocaleString(GKI18N.lang, { day: "numeric", month: "short", year: "numeric", hour: "2-digit", minute: "2-digit" }); }
    catch (x) { return new Date(t * 1000).toISOString().slice(0, 16).replace("T", " "); }
  };
  var at = function (x) { return (x.claim && x.claim.time) || x.at; };
  var explorer = GKI18N.path("/explorer");
  function draw(id, data, opts) {
    var host = $(id);
    if (!host || !window.GKCharts) return;
    if (st.charts[id]) st.charts[id].update(data, opts); else st.charts[id] = GKCharts.chart(host, data, opts);
  }
  // Days grouped into weeks (from Monday): totals add up; the week's median is the median of
  // its days' medians weighted by their swaps.
  function weeks(days) {
    var by = {};
    days.forEach(function (d) { var wk = d.day - ((Math.floor(d.day / 86400) + 3) % 7) * 86400; (by[wk] = by[wk] || []).push(d); });
    return Object.keys(by).map(Number).sort(function (a, b) { return a - b; }).map(function (wk) {
      var l = by[wk], ms = l.filter(function (x) { return x.median != null; }).map(function (x) { return [Number(x.median), Number(x.swaps) || 1]; }).sort(function (a, b) { return a[0] - b[0]; });
      var half = ms.reduce(function (a, x) { return a + x[1]; }, 0) / 2, acc = 0, med = null;
      for (var i = 0; i < ms.length; i++) { acc += ms[i][1]; if (acc >= half) { med = ms[i][0]; break; } }
      var add = function (k) { return l.reduce(function (a, x) { return a + Number(x[k] || 0); }, 0); };
      return { day: wk, swaps: add("swaps"), gnr: add("gnr"), sats: add("sats"), median: med };
    });
  }
  function figures() {
    var s = st.sum || {}, a = s.all || {}, d = s.day || {};
    $("thN").textContent = GKI18N.num(a.swaps || 0);
    $("thNN").textContent = a.swaps ? T("{n} in the last 24 hours", { n: GKI18N.num(d.swaps || 0) }) + (a.first ? " · " + T("since {date}", { date: new Date(a.first * 1000).toLocaleDateString(GKI18N.lang) }) : "") : T("no swaps yet");
    $("thG").textContent = a.swaps ? gnrShort(a.gnr) + " GNR" : "—";
    $("thGN").textContent = a.swaps ? T("{amount} GNR in the last 24 hours", { amount: gnrShort(d.gnr || 0) }) : "";
    $("thB").textContent = a.swaps ? btcShort(a.sats) + " BTC" : "—";
    $("thBN").textContent = a.swaps && st.btc > 0 ? "≈ " + dollars(a.sats) : "";
    $("thAvg").textContent = main(a.vwap);
    $("thAvgN").textContent = (a.vwap != null && st.btc > 0 ? sat(a.vwap) + " · " : "") + T("weighed by the bitcoin moved");
    $("thMed").textContent = main(a.median);
    $("thRange").textContent = a.low != null ? T("low {low} · high {high}", { low: main(a.low), high: main(a.high) }) : "";
  }
  function charts() {
    var s = st.sum || {}, now = Date.now() / 1000, since = st.range ? now - st.range * 86400 : 0;
    var daily = (s.days || []).filter(function (x) { return x.day + 86400 > since; });
    var weekly = daily.length > 180, days = weekly ? weeks(daily) : daily, mid = weekly ? 3.5 * 86400 : 43200;
    $("thMedH").textContent = weekly ? T("Median price per week") : T("Median price per day");
    $("thVolH").textContent = weekly ? T("GNR traded per week") : T("GNR traded per day");
    $("thBtcH").textContent = weekly ? T("Bitcoin traded per week") : T("Bitcoin traded per day");
    $("thCntH").textContent = weekly ? T("Swaps per week") : T("Swaps per day");
    var satAxis = function (v, axis) { return axis ? GKCharts.fmtNum(v) + " sat" : sat(v) + (usd(v) ? " · ≈ " + usd(v) : ""); };
    var sw = st.swaps.slice().sort(function (x, y) { return at(x) - at(y); });
    draw("thChPrice", sw.map(function (x) { return { t: at(x), v: Number(x.price), h: x.claim ? x.claim.height : null }; }), { kind: "area", straight: true, points: sw.length <= 200, zero: false, fmt: satAxis, label: T("Price per GNR of every completed swap"), height: 240 });
    draw("thChMed", days.filter(function (x) { return x.median != null; }).map(function (x) { return { t: x.day + mid, v: x.median }; }), { kind: "line", straight: true, points: days.length <= 120, zero: false, fmt: satAxis, label: $("thMedH").textContent, height: 200 });
    draw("thChVol", days.map(function (x) { return { t: x.day + mid, v: x.gnr / 1e8 }; }), { kind: "bars", fmt: function (v, axis) { return axis ? GKCharts.fmtNum(v) : gnr(v * 1e8) + " GNR"; }, label: $("thVolH").textContent, height: 200 });
    draw("thChBtc", days.map(function (x) { return { t: x.day + mid, v: x.sats }; }), { kind: "bars", fmt: function (v, axis) { return axis ? GKCharts.fmtNum(v) + " sat" : btc(v) + " BTC" + (dollars(v) ? " · ≈ " + dollars(v) : ""); }, label: $("thBtcH").textContent, height: 200 });
    draw("thChCnt", days.map(function (x) { return { t: x.day + mid, v: x.swaps }; }), { kind: "bars", integer: true, fmt: function (v, axis) { return axis ? String(Math.round(v)) : Tn(Math.round(v), "{n} swap", "{n} swaps"); }, label: $("thCntH").textContent, height: 180 });
    var add = function (k) { return daily.reduce(function (a, x) { return a + Number(x[k] || 0); }, 0); };
    var last = sw[sw.length - 1], ps = sw.map(function (x) { return Number(x.price); }).sort(function (a, b) { return a - b; });
    var med = ps.length ? (ps.length % 2 ? ps[(ps.length - 1) / 2] : (ps[ps.length / 2 - 1] + ps[ps.length / 2]) / 2) : null;
    $("thLast").textContent = last ? (st.total > sw.length ? T("newest {n} of {m}", { n: GKI18N.num(sw.length), m: GKI18N.num(st.total) }) + " · " : "") + T("last {price}", { price: sat(last.price) }) : "—";
    $("thMedR").textContent = sat(med);
    $("thVolR").textContent = daily.length ? gnrShort(add("gnr")) + " GNR" : "—";
    $("thBtcR").textContent = daily.length ? btcShort(add("sats")) + " BTC" : "—";
    $("thCntR").textContent = daily.length ? Tn(add("swaps"), "{n} swap", "{n} swaps", { n: GKI18N.num(add("swaps")) }) : "—";
    $("thNote").textContent = T("Every point is one completed swap, at the price its two wallets agreed on. Move over a chart to read a swap or a day.");
    document.querySelectorAll("#thRng button").forEach(function (b) { var on = Number(b.dataset.r) === st.range; b.classList.toggle("on", on); b.setAttribute("aria-pressed", String(on)); });
  }
  function row(x) {
    var c = x.claim, h = c ? GKI18N.num(Number(c.height)) : "";
    return "<tr><td>" + e(when(at(x))) + "</td>" +
      "<td><b class=\"mono\">" + e(sat(x.price)) + "</b>" + (usd(x.price) ? "<br><small class=\"muted\">≈ " + e(usd(x.price)) + "</small>" : "") + "</td>" +
      "<td class=\"num\">" + e(gnr(x.gnr)) + "</td>" +
      "<td class=\"num\">" + e(btc(x.sats)) + (dollars(x.sats) ? "<br><small class=\"muted\">≈ " + e(dollars(x.sats)) + "</small>" : "") + "</td>" +
      "<td class=\"num\">" + (c ? "<a href=\"" + explorer + "#/tx/" + e(c.txid) + "\">" + e(T("Block #{h}", { h: h })) + " ↗</a>" : e(T("waiting"))) +
      (x.verified ? " <i class=\"mk-ok\" title=\"" + e(T("The board found this swap's bitcoin on the Bitcoin chain")) + "\">BTC ✓</i>" : "") + "</td></tr>";
  }
  function table(p) {
    var per = Number($("thPer").value) || 50, ver = $("thVer").checked;
    gk.api("/v1/swap/history?offset=" + p * per + "&limit=" + per + (ver ? "&verified=true" : "")).then(function (r) {
      var total = Number(r.total) || 0, pages = Math.max(1, Math.ceil(total / per));
      if (p > 0 && p >= pages) return table(pages - 1);
      st.page = p;
      $("thAllN").textContent = total ? "· " + GKI18N.num(total) : "";
      $("thRows").innerHTML = (r.swaps || []).length ? r.swaps.map(row).join("") : "<tr><td colspan=\"5\" class=\"muted mk-empty\">" + e(T("No completed swaps yet.")) + "</td></tr>";
      $("thPager").innerHTML = pages > 1
        ? "<button class=\"btn\" data-pg=\"0\"" + (p ? "" : " disabled") + " aria-label=\"" + e(T("First page")) + "\">«</button>" +
          "<button class=\"btn\" data-pg=\"" + (p - 1) + "\"" + (p ? "" : " disabled") + " aria-label=\"" + e(T("Previous page")) + "\">‹</button>" +
          "<span>" + e(T("Page {n} of {m}", { n: GKI18N.num(p + 1), m: GKI18N.num(pages) })) + "</span>" +
          "<button class=\"btn\" data-pg=\"" + (p + 1) + "\"" + (p < pages - 1 ? "" : " disabled") + " aria-label=\"" + e(T("Next page")) + "\">›</button>" +
          "<button class=\"btn\" data-pg=\"" + (pages - 1) + "\"" + (p < pages - 1 ? "" : " disabled") + " aria-label=\"" + e(T("Last page")) + "\">»</button>"
        : "";
    }).catch(function () {
      $("thRows").innerHTML = "<tr><td colspan=\"5\" class=\"muted mk-empty\">" + e(T("The trade history could not be read right now.")) + "</td></tr>";
    });
  }
  function load() {
    var now = Math.floor(Date.now() / 1000), since = st.range ? now - st.range * 86400 : 0;
    Promise.all([
      gk.api("/v1/swap/history/summary").catch(function () { return null; }),
      gk.api("/v1/swap/history?limit=200" + (since ? "&since=" + since : "")).catch(function () { return null; }),
      gk.api("/v1/btc-price").catch(function () { return {}; })
    ]).then(function (r) {
      st.btc = Number((r[2] || {}).USD) || 0;
      st.sum = r[0];
      st.swaps = (r[1] && r[1].swaps) || [];
      st.total = (r[1] && Number(r[1].total)) || st.swaps.length;
      // More of the period's swaps for the chart, page by page up to the cap.
      var more = function (off) {
        if (!r[1] || !r[1].more || st.swaps.length >= CHART_SWAPS) return Promise.resolve();
        return gk.api("/v1/swap/history?limit=200&offset=" + off + (since ? "&since=" + since : "")).then(function (x) {
          st.swaps = st.swaps.concat(x.swaps || []);
          r[1].more = x.more;
          return more(off + (x.swaps || []).length);
        }).catch(function () {});
      };
      return more(st.swaps.length);
    }).then(function () {
      if (!st.sum) { $("thNote").textContent = T("The trade history could not be read right now."); return; }
      figures(); charts(); table(st.page);
    });
  }
  document.addEventListener("click", function (ev) {
    var b = ev.target.closest ? ev.target.closest("#thRng button, [data-pg]") : null;
    if (!b || b.disabled) return;
    if (b.dataset.r != null) { st.range = Number(b.dataset.r) || 0; load(); }
    else table(Number(b.getAttribute("data-pg")) || 0);
  });
  $("thVer").addEventListener("change", function () { table(0); });
  $("thPer").addEventListener("change", function () { table(0); });
  load();
  setInterval(function () { if (!document.hidden) load(); }, 60000);
})();
