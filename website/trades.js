/* genoryn.io/trades: every completed GNR swap, from the node's /v1/swap/history API. The table
   comes a page at a time, the per-swap chart shows the newest swaps of the period, and every
   other figure comes from the history's day-by-day summary. */
(function () {
  var $ = function (id) { return document.getElementById(id); };
  var e = function (s) { return gk.esc(s); };
  var st = { range: 30, page: 0, btc: 0, sum: null, swaps: [], total: 0, charts: {}, mode: null, seq: 0 };
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
  // Which swaps the figures and charts show (1.0.55). "verified": the swaps whose bitcoin the
  // board found on the Bitcoin chain, which nobody can make up; "all": every swap two wallets
  // reported. Verified comes first whenever there is one.
  var hasV = function () { return !!(st.sum && st.sum.all && st.sum.all.verified_figures); };
  var mode = function () { if (!hasV()) return "all"; return st.mode || (Number(st.sum.all.verified) > 0 ? "verified" : "all"); };
  function pick(f, m) {
    if (!f) return {};
    var v = f.verified_figures;
    if (m !== "verified" || !v) return f;
    return { swaps: Number(f.verified) || 0, verified: Number(f.verified) || 0, gnr: v.gnr, sats: v.sats, low: v.low, high: v.high, median: v.median, vwap: v.vwap, day: f.day, first: f.first, last: f.last };
  }
  function figures() {
    var s = st.sum || {}, m = mode(), raw = s.all || {}, a = pick(s.all, m), d = pick(s.day, m), has = hasV();
    document.querySelectorAll("#thMode button").forEach(function (b) { var on = b.dataset.m === m; b.classList.toggle("on", on); b.setAttribute("aria-pressed", String(on)); b.disabled = !has; });
    $("thModeNote").textContent = !has ? T("This board does not tell verified swaps apart in its figures yet: every reported swap is shown.")
      : m === "verified" ? T("Swaps whose bitcoin the board found on the Bitcoin chain. Nobody can make up such a payment, so a false report cannot move these figures.")
      : T("Every swap two wallets reported. Those whose bitcoin was not found are drawn in grey and do not set the scale of the charts.");
    var unv = Math.max(0, (Number(raw.swaps) || 0) - (Number(raw.verified) || 0));
    $("thN").textContent = GKI18N.num(a.swaps || 0);
    $("thNN").textContent = a.swaps ? T("{n} in the last 24 hours", { n: GKI18N.num(d.swaps || 0) }) + (a.first ? " · " + T("since {date}", { date: new Date(a.first * 1000).toLocaleDateString(GKI18N.lang) }) : "") : raw.swaps ? T("none verified yet") : T("no swaps yet");
    $("thNU").textContent = !has || !raw.swaps ? "" : m === "verified" ? (unv ? Tn(unv, "and {n} more reported, its bitcoin not found", "and {n} more reported, their bitcoin not found", { n: GKI18N.num(unv) }) : T("every reported swap is verified"))
      : T("{n} of them verified", { n: GKI18N.num(Number(raw.verified) || 0) });
    $("thG").textContent = a.swaps ? gnrShort(a.gnr) + " GNR" : "—";
    $("thGN").textContent = a.swaps ? T("{amount} GNR in the last 24 hours", { amount: gnrShort(d.gnr || 0) }) : "";
    $("thB").textContent = a.swaps ? btcShort(a.sats) + " BTC" : "—";
    $("thBN").textContent = a.swaps && st.btc > 0 ? "≈ " + dollars(a.sats) : "";
    $("thAvg").textContent = main(a.vwap);
    $("thAvgN").textContent = (a.vwap != null && st.btc > 0 ? sat(a.vwap) + " · " : "") + T("weighed by the bitcoin moved");
    $("thMed").textContent = main(a.median);
    $("thRange").textContent = a.low != null ? T("low {low} · high {high}", { low: main(a.low), high: main(a.high) }) : "";
  }
  // The scale a chart keeps: the verified values with a little room around them.
  function scale(vals) {
    vals = vals.filter(function (x) { return x != null && isFinite(x); });
    if (!vals.length) return undefined;
    var lo = Math.min.apply(null, vals), hi = Math.max.apply(null, vals), pad = Math.max((hi - lo) * 0.18, hi * 0.04);
    return [Math.max(0, lo - pad), hi + pad];
  }
  function charts() {
    var s = st.sum || {}, m = mode(), has = hasV(), D = 86400, now = Math.floor(Date.now() / 1000), today = now - now % D, since = st.range ? now - st.range * D : 0;
    // Every day of the period has its place, with or without a swap.
    var figs = {}, keys = [];
    (s.days || []).forEach(function (x) { figs[Number(x.day)] = pick(x, m); keys.push(Number(x.day)); });
    var first = st.range ? today - (st.range - 1) * D : Math.min.apply(null, [today].concat(keys));
    var daily = [];
    for (var t = first; t <= today; t += D) { var f = figs[t]; daily.push(f && f.swaps ? f : { day: t, swaps: 0, gnr: 0, sats: 0, median: null }); }
    var weekly = daily.length > 180, days = weekly ? weeks(daily) : daily, step = weekly ? 7 * D : D, mid = step / 2;
    $("thMedH").textContent = weekly ? T("Median price per week") : T("Median price per day");
    $("thVolH").textContent = weekly ? T("GNR traded per week") : T("GNR traded per day");
    $("thBtcH").textContent = weekly ? T("Bitcoin traded per week") : T("Bitcoin traded per day");
    $("thCntH").textContent = weekly ? T("Swaps per week") : T("Swaps per day");
    var satAxis = function (v, axis) { return axis ? GKCharts.fmtNum(v) + " sat" : sat(v) + (usd(v) ? " · ≈ " + usd(v) : ""); };
    var dayLabel = function (x) { var d = new Date((x.t - mid) * 1000).toLocaleDateString(GKI18N.lang, { day: "numeric", month: "short", year: "numeric" }); return weekly ? T("week of {date}", { date: d }) : d; };
    var span = [days.length ? days[0].day : first, (days.length ? days[days.length - 1].day : today) + step], none = T("no swaps");
    // One point per swap. The scale comes from the verified swaps; a reported swap whose bitcoin
    // was not found is a small grey point, and one far off the scale sits at the edge with its
    // real price in the tip.
    var sw = st.swaps.filter(function (x) { return m !== "verified" || x.verified || !has; }).sort(function (x, y) { return at(x) - at(y); });
    // Each swap says whether it is verified, also on a board whose figures do not.
    var told = has || sw.some(function (x) { return x.verified; });
    var solid = sw.filter(function (x) { return x.verified || !told; }), soft = sw.length - solid.length;
    var range = soft ? scale(solid.map(function (x) { return Number(x.price); })) : undefined;
    var far = range ? sw.filter(function (x) { return !x.verified && (Number(x.price) < range[0] || Number(x.price) > range[1]); }).length : 0;
    var note = T("reported; its bitcoin was not found");
    var pts = sw.map(function (x) { var grey = told && !x.verified; return { t: at(x), v: Number(x.price), h: x.claim ? x.claim.height : null, soft: grey, note: grey ? note : undefined }; });
    var t0 = since || (sw.length ? at(sw[0]) : now - D), few = sw.length <= 40;
    draw("thChPrice", pts, { kind: "area", straight: true, points: sw.length <= 200, zero: false, range: range, ordinal: few, span: few ? undefined : [t0 - (now - t0) * 0.03, now + (now - t0) * 0.03], fmt: satAxis,
      label: T("Price per GNR of every completed swap"), height: 240, empty: (s.all || {}).swaps && m === "verified" ? T("No verified swap in this period") : undefined });
    $("thLegend").innerHTML = soft ? "<span><i></i>" + e(T("verified swap")) + "</span><span><i class=\"soft\"></i>" + e(T("reported, bitcoin not found")) + "</span>" + (far ? "<span><i class=\"far\"></i>" + e(T("off the scale: its price is in the tip")) + "</span>" : "") : "";
    // The medians of the days keep the scale of the verified ones too.
    var mrange = m === "all" && has ? scale((s.days || []).map(function (x) { return x.verified_figures ? x.verified_figures.median : null; })) : undefined;
    draw("thChMed", days.filter(function (x) { return x.median != null; }).map(function (x) { return { t: x.day + mid, v: Number(x.median) }; }), { kind: "line", straight: true, points: true, zero: false, range: mrange, span: span, tipTime: dayLabel, fmt: satAxis, label: $("thMedH").textContent, height: 200 });
    var bars = function (id, val, fmt, label, extra) {
      var o = { kind: "bars", span: span, days: !weekly, tipTime: dayLabel, fmt: fmt, label: label, height: 200 };
      for (var k in extra) o[k] = extra[k];
      draw(id, days.map(function (x) { return { t: x.day + mid, v: x.swaps ? val(x) : null, none: !x.swaps, note: x.swaps ? undefined : none }; }), o);
    };
    bars("thChVol", function (x) { return x.gnr / 1e8; }, function (v, axis) { return axis ? GKCharts.fmtNum(v) : gnr(v * 1e8) + " GNR"; }, $("thVolH").textContent);
    bars("thChBtc", function (x) { return x.sats; }, function (v, axis) { return axis ? GKCharts.fmtNum(v) + " sat" : btc(v) + " BTC" + (dollars(v) ? " · ≈ " + dollars(v) : ""); }, $("thBtcH").textContent);
    bars("thChCnt", function (x) { return x.swaps; }, function (v, axis) { return axis ? String(Math.round(v)) : Tn(Math.round(v), "{n} swap", "{n} swaps"); }, $("thCntH").textContent, { integer: true, height: 180 });
    var add = function (k) { return daily.reduce(function (a, x) { return a + Number(x[k] || 0); }, 0); };
    var last = solid[solid.length - 1] || sw[sw.length - 1], ps = solid.map(function (x) { return Number(x.price); }).sort(function (a, b) { return a - b; });
    var med = ps.length ? (ps.length % 2 ? ps[(ps.length - 1) / 2] : (ps[ps.length / 2 - 1] + ps[ps.length / 2]) / 2) : null;
    $("thLast").textContent = last ? (st.total > sw.length ? T("newest {n} of {m}", { n: GKI18N.num(sw.length), m: GKI18N.num(st.total) }) + " · " : "") + T("last {price}", { price: sat(last.price) }) : "—";
    $("thMedR").textContent = sat(med);
    $("thVolR").textContent = add("swaps") ? gnrShort(add("gnr")) + " GNR" : "—";
    $("thBtcR").textContent = add("swaps") ? btcShort(add("sats")) + " BTC" : "—";
    $("thCntR").textContent = add("swaps") ? Tn(add("swaps"), "{n} swap", "{n} swaps", { n: GKI18N.num(add("swaps")) }) : "—";
    $("thNote").textContent = T("Every point is one completed swap, at the price its two wallets agreed on. Move over a chart to read a swap or a day.");
    document.querySelectorAll("#thRng button").forEach(function (b) { var on = Number(b.dataset.r) === st.range; b.classList.toggle("on", on); b.setAttribute("aria-pressed", String(on)); });
  }
  /* A swap's proof: what both chains hold of it, so anyone can see that it happened. The GNR
     claim carries the swap's secret; the Bitcoin transaction that took the payment out of
     the contract carries the same secret; and the SHA-256 of that secret is the hash lock
     both coins were locked with (worked out here, in this page). Opened row by row. */
  var shown = {}, open = {};
  var HEX = /^[0-9a-f]{64}$/;
  var mid = function (h) { h = String(h || ""); return h.length > 26 ? h.slice(0, 14) + "…" + h.slice(-10) : h; };
  function proofHtml(x) {
    var c = x.claim, b = x.btc;
    var copy = function (v) { return "<button type=\"button\" class=\"btn\" data-copy=\"" + e(v) + "\">" + e(T("Copy")) + "</button>"; };
    var btx = function (id) { return HEX.test(id) ? "<a class=\"btn\" href=\"https://mempool.space/tx/" + id + "\" target=\"_blank\" rel=\"noopener noreferrer\" title=\"" + e(T("See this transaction on mempool.space, a public Bitcoin explorer")) + "\">" + e(T("View ↗")) + "</a>" : ""; };
    var line = function (k, v, acts, note) { return "<div class=\"thp-r\"><span class=\"thp-k\">" + e(k) + "</span><span class=\"mono thp-v\" title=\"" + e(v) + "\">" + e(mid(v)) + "</span><span class=\"thp-x\">" + acts + "</span>" + (note ? "<em class=\"thp-n\">" + note + "</em>" : "") + "</div>"; };
    var gnrSide = c && HEX.test(c.txid) ? line(T("Claim of the GNR"), c.txid, copy(c.txid) + "<a class=\"btn\" href=\"" + explorer + "#/tx/" + c.txid + "\">" + e(T("View ↗")) + "</a>", e(T("block {height} · {time}", { height: GKI18N.num(Number(c.height)), time: when(c.time) })))
      : "<p class=\"thp-none\">" + e(T("The claim of the GNR is not in a block yet.")) + "</p>";
    var lock = line(T("Hash lock"), x.hash, copy(x.hash)) + (c && HEX.test(c.secret || "") ? line(T("Secret"), c.secret, copy(c.secret), "<span class=\"thp-chk\" data-hash=\"" + e(x.hash) + "\" data-secret=\"" + e(c.secret) + "\">" + e(T("checking…")) + "</span>") : "");
    var fund = b ? String(b.funding || "").split(":") : [];
    var btcSide = b && HEX.test(fund[0]) && HEX.test(b.claim)
      ? line(T("Bitcoin paid into the swap's contract"), fund[0], copy(fund[0]) + btx(fund[0]), e(T("output {n} · Bitcoin block {height}", { n: Number(fund[1]) || 0, height: GKI18N.num(Number(b.funded_height)) }))) +
        line(T("Bitcoin taken with the secret"), b.claim, copy(b.claim) + btx(b.claim))
      : "<p class=\"thp-none\">" + e(x.verified ? T("The board found this swap's bitcoin on the Bitcoin chain. It writes the two Bitcoin transactions down from version 1.0.58 on; this swap is older.") : T("The board has not found this swap's bitcoin on the Bitcoin chain: it is a report, and counts as unverified.")) + "</p>";
    return "<div class=\"thp\"><div class=\"thp-g\"><h4>" + e(T("On the GNR chain")) + "</h4>" + gnrSide + "</div><div class=\"thp-g\"><h4>" + e(T("The lock both coins shared")) + "</h4>" + lock + "</div><div class=\"thp-g\"><h4>" + e(T("On the Bitcoin chain")) + "</h4>" + btcSide + "</div>" +
      "<p class=\"thp-how\">" + e(T("How to check it yourself: the claim on the GNR chain shows the secret; the Bitcoin transaction that took the payment carries the same secret in its witness; and the SHA-256 of that secret is the hash lock. Only a swap that really happened leaves all three. Amounts on the GNR side stay hidden: the price is what the two wallets reported, and for a verified swap the bitcoin is what the Bitcoin chain shows.")) + "</p></div>";
  }
  function proofOpen(btn) {
    var x = shown[btn.getAttribute("data-proof")], tr = btn.closest("tr");
    if (!x || !tr) return;
    var row = document.createElement("tr");
    row.className = "thp-row";
    row.innerHTML = "<td colspan=\"5\">" + proofHtml(x) + "</td>";
    tr.parentNode.insertBefore(row, tr.nextSibling);
    btn.setAttribute("aria-expanded", "true");
    var chk = row.querySelector(".thp-chk");
    if (!chk) return;
    var done = function (ok) {
      chk.textContent = ok === true ? T("✓ SHA-256 of this secret is the hash lock (checked in this window)") : ok === false ? T("✗ This secret does not open the hash lock") : T("(this window cannot work out SHA-256; check it with any tool)");
      chk.classList.add(ok === true ? "yes" : ok === false ? "no" : "na");
    };
    try {
      var bytes = new Uint8Array(chk.getAttribute("data-secret").match(/../g).map(function (h) { return parseInt(h, 16); }));
      crypto.subtle.digest("SHA-256", bytes).then(function (d) {
        var hex = Array.prototype.map.call(new Uint8Array(d), function (v) { return ("0" + v.toString(16)).slice(-2); }).join("");
        done(hex === chk.getAttribute("data-hash"));
      }, function () { done(null); });
    } catch (err) { done(null); }
  }
  function row(x) {
    var c = x.claim, h = c ? GKI18N.num(Number(c.height)) : "", id = HEX.test(x.hash || "") ? x.hash : "";
    if (id) shown[id] = x;
    return "<tr><td>" + e(when(at(x))) + "</td>" +
      "<td><b class=\"mono\">" + e(sat(x.price)) + "</b>" + (usd(x.price) ? "<br><small class=\"muted\">≈ " + e(usd(x.price)) + "</small>" : "") + "</td>" +
      "<td class=\"num\">" + e(gnr(x.gnr)) + "</td>" +
      "<td class=\"num\">" + e(btc(x.sats)) + (dollars(x.sats) ? "<br><small class=\"muted\">≈ " + e(dollars(x.sats)) + "</small>" : "") + "</td>" +
      "<td class=\"num th-px\">" + (c ? "<a href=\"" + explorer + "#/tx/" + e(c.txid) + "\">" + e(T("Block #{h}", { h: h })) + " ↗</a>" : e(T("waiting"))) +
      (x.verified ? " <i class=\"mk-ok\" title=\"" + e(T("The board found this swap's bitcoin on the Bitcoin chain")) + "\">BTC ✓</i>" : "") +
      (id ? " <button type=\"button\" class=\"btn th-pb\" data-proof=\"" + id + "\" aria-expanded=\"false\">" + e(T("Proof")) + "</button>" : "") + "</td></tr>";
  }
  function table(p) {
    var per = Number($("thPer").value) || 50, ver = mode() === "verified";
    gk.api("/v1/swap/history?offset=" + p * per + "&limit=" + per + (ver ? "&verified=true" : "")).then(function (r) {
      var total = Number(r.total) || 0, pages = Math.max(1, Math.ceil(total / per));
      if (p > 0 && p >= pages) return table(pages - 1);
      st.page = p;
      $("thAllN").textContent = total ? "· " + GKI18N.num(total) : "";
      shown = {};
      $("thRows").innerHTML = (r.swaps || []).length ? r.swaps.map(row).join("") : "<tr><td colspan=\"5\" class=\"muted mk-empty\">" + e(T("No completed swaps yet.")) + "</td></tr>";
      // A proof that was open stays open when the page of swaps is read again.
      Array.prototype.forEach.call($("thRows").querySelectorAll("[data-proof]"), function (b) { if (open[b.getAttribute("data-proof")]) proofOpen(b); });
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
    var now = Math.floor(Date.now() / 1000), since = st.range ? now - st.range * 86400 : 0, seq = ++st.seq;
    Promise.all([
      gk.api("/v1/swap/history/summary").catch(function () { return null; }),
      gk.api("/v1/btc-price").catch(function () { return {}; })
    ]).then(function (r) {
      if (seq !== st.seq) return null;
      st.btc = Number((r[1] || {}).USD) || 0;
      st.sum = r[0];
      // The swaps of the period for the chart: the verified ones alone when that is what is shown.
      var q = "/v1/swap/history?limit=200" + (since ? "&since=" + since : "") + (mode() === "verified" ? "&verified=true" : "");
      return gk.api(q).catch(function () { return null; }).then(function (first) {
        if (seq !== st.seq) return null;
        st.swaps = (first && first.swaps) || [];
        st.total = (first && Number(first.total)) || st.swaps.length;
        // More of the period's swaps, page by page up to the cap.
        var more = function (off) {
          if (!first || !first.more || st.swaps.length >= CHART_SWAPS) return Promise.resolve(true);
          return gk.api(q + "&offset=" + off).then(function (x) {
            st.swaps = st.swaps.concat(x.swaps || []);
            first.more = x.more && (x.swaps || []).length > 0;
            return more(off + (x.swaps || []).length);
          }).catch(function () { return true; });
        };
        return more(st.swaps.length);
      });
    }).then(function (ok) {
      if (!ok || seq !== st.seq) return;
      if (!st.sum) { $("thNote").textContent = T("The trade history could not be read right now."); return; }
      figures(); charts(); table(st.page);
    });
  }
  document.addEventListener("click", function (ev) {
    var p = ev.target.closest ? ev.target.closest("[data-proof], [data-copy]") : null;
    if (p && p.hasAttribute("data-copy")) {
      var text = p.getAttribute("data-copy");
      if (navigator.clipboard) navigator.clipboard.writeText(text).then(function () { var was = p.textContent; p.textContent = T("Copied"); setTimeout(function () { p.textContent = was; }, 1400); }, function () {});
      return;
    }
    if (p) {
      var id = p.getAttribute("data-proof"), next = p.closest("tr").nextElementSibling;
      if (next && next.classList.contains("thp-row")) { next.parentNode.removeChild(next); p.setAttribute("aria-expanded", "false"); delete open[id]; }
      else { open[id] = true; proofOpen(p); }
      return;
    }
    var b = ev.target.closest ? ev.target.closest("#thRng button, #thMode button, [data-pg]") : null;
    if (!b || b.disabled) return;
    if (b.dataset.r != null) { st.range = Number(b.dataset.r) || 0; load(); }
    else if (b.dataset.m != null) { st.mode = b.dataset.m; st.page = 0; load(); }
    else table(Number(b.getAttribute("data-pg")) || 0);
  });
  $("thPer").addEventListener("change", function () { table(0); });
  load();
  setInterval(function () { if (!document.hidden) load(); }, 60000);
})();
