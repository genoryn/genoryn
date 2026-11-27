/* Genoryn Krypton charts: dependency-free SVG line/area/bar charts and a 3D supply disc.
   Monochrome, theme-aware (CSS variables), responsive and accessible. */
(function () {
  var NS = "http://www.w3.org/2000/svg";
  var reduce = window.matchMedia && matchMedia("(prefers-reduced-motion: reduce)").matches;
  var low = document.documentElement.getAttribute("data-fx") === "low";
  var uid = 0;

  function el(tag, attrs, parent) {
    var e = document.createElementNS(NS, tag);
    for (var k in attrs) e.setAttribute(k, attrs[k]);
    if (parent) parent.appendChild(e);
    return e;
  }
  function niceTicks(min, max, n) {
    if (!(max > min)) { max = min + 1; }
    var span = max - min, step = Math.pow(10, Math.floor(Math.log10(span / n)));
    var err = (n * step) / span;
    if (err <= 0.15) step *= 10; else if (err <= 0.35) step *= 5; else if (err <= 0.75) step *= 2;
    var lo = Math.floor(min / step) * step, hi = Math.ceil(max / step) * step, out = [];
    for (var v = lo; v <= hi + step / 2; v += step) out.push(v);
    return out;
  }
  function lang() { return window.GKI18N ? GKI18N.lang : undefined; }
  function timeLabel(t, span) {
    var d = new Date(t * 1000);
    if (span <= 2 * 86400) return d.toLocaleTimeString(lang(), { hour: "2-digit", minute: "2-digit" });
    return d.toLocaleDateString(lang(), { month: "short", day: "numeric" });
  }
  // Catmull-Rom to cubic Bezier, clamped so the curve never overshoots the data range.
  function smooth(pts) {
    if (pts.length < 3) return "M" + pts.map(function (p) { return p[0].toFixed(1) + "," + p[1].toFixed(1); }).join("L");
    var d = "M" + pts[0][0].toFixed(1) + "," + pts[0][1].toFixed(1);
    for (var i = 0; i < pts.length - 1; i++) {
      var p0 = pts[i - 1] || pts[i], p1 = pts[i], p2 = pts[i + 1], p3 = pts[i + 2] || p2;
      var lo = Math.min(p1[1], p2[1]), hi = Math.max(p1[1], p2[1]);
      var c1y = Math.min(hi, Math.max(lo, p1[1] + (p2[1] - p0[1]) / 6));
      var c2y = Math.min(hi, Math.max(lo, p2[1] - (p3[1] - p1[1]) / 6));
      d += "C" + (p1[0] + (p2[0] - p0[0]) / 6).toFixed(1) + "," + c1y.toFixed(1) + " " +
        (p2[0] - (p3[0] - p1[0]) / 6).toFixed(1) + "," + c2y.toFixed(1) + " " + p2[0].toFixed(1) + "," + p2[1].toFixed(1);
    }
    return d;
  }

  function esc(s) { return String(s == null ? "" : s).replace(/[&<>"']/g, function (c) { return { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]; }); }

  /* data: [{t, v, h, soft, none, note}], opts: {fmt, kind: "area"|"line"|"bars", target, targetLabel,
     height, zero, integer, straight (no smoothing: each point is a real event), points (a dot on
     every point), range: [lo, hi] (the scale; a value outside it is drawn at the edge, hollow and
     dashed, and the tip still shows its real figure), span: [t0, t1] (the time the x axis
     covers, when it is wider than the data: a period with empty days in it), ordinal (the
     points, or the bars, are spaced evenly, one after another, instead of by their time: events
     that happened close together stay readable; with clock: true their labels are times of
     day when they all fall within two days, else dates), tipTime(d)}.
     A point with v == null is a gap: the line stops there and starts again after it. A soft point
     (soft: true) is drawn small and grey and the line passes it by. A bar with none: true is an
     empty slot (a day with nothing in it), drawn as a faint tick so the days stay in their places. */
  function chart(host, data, opts) {
    opts = opts || {};
    host.classList.add("gk-chart");
    var state = { data: data || [], opts: opts };
    host._gk = state;
    var tip = document.createElement("div");
    tip.className = "gk-tip hidden";
    host.innerHTML = "";
    host.appendChild(tip);
    function draw() {
      var data = state.data, o = state.opts, W = Math.max(240, host.clientWidth), H = o.height || 190;
      var old = host.querySelector("svg"); if (old) old.remove();
      var svg = el("svg", { viewBox: "0 0 " + W + " " + H, width: W, height: H, role: "img", "aria-label": o.label || "chart" });
      host.insertBefore(svg, tip);
      var isNum = function (d) { return d.v != null && isFinite(d.v); };
      var bars = o.kind === "bars";
      var real = data.filter(function (d) { return isNum(d) && !(bars && d.none); });
      if (!real.length) {
        el("text", { x: W / 2, y: H / 2, "text-anchor": "middle", class: "gk-empty" }, svg).textContent = o.empty || T("No data in this range yet");
        return;
      }
      var padL = 54, padR = 10, padT = 14, padB = 26, iw = W - padL - padR, ih = H - padT - padB;
      var t0 = o.span ? Number(o.span[0]) : data[0].t, t1 = o.span ? Number(o.span[1]) : data[data.length - 1].t, span = Math.max(1, t1 - t0);
      var one = data.length === 1 && !o.span;
      // The scale comes from the range the caller trusts (if given), else from every value.
      var vs = o.range ? [Number(o.range[0]), Number(o.range[1])] : real.filter(function (d) { return !d.soft || bars; }).map(function (d) { return d.v; });
      if (!vs.length) vs = real.map(function (d) { return d.v; });
      if (o.target != null) vs.push(o.target);
      var vmin = o.zero === false ? Math.min.apply(null, vs) : Math.min(0, Math.min.apply(null, vs));
      var vmax = Math.max.apply(null, vs);
      if (vmax === vmin) {
        var pad = vmin === 0 ? 1 : Math.abs(vmin) * 0.1;
        if (o.zero === false) { vmin -= pad; vmax += pad; } else vmax = vmin + pad;
      }
      if (o.integer) vmax = Math.max(vmax, vmin + 2);
      var ticks = niceTicks(vmin, vmax, 3);
      if (o.integer && ticks.length > 1 && ticks[1] - ticks[0] < 1) ticks = [vmin, Math.ceil((vmin + vmax) / 2), Math.ceil(vmax)];
      vmin = ticks[0]; vmax = ticks[ticks.length - 1];
      var bw = bars ? Math.max(1.5, Math.min(o.barMax || 14, (iw / data.length) * 0.66)) : 0;
      // A few points sit away from the edges, so one or five swaps read as a chart, not a corner.
      var inset = bars ? bw / 2 + 1 : (!one && data.length <= 6 ? iw * 0.07 : 0);
      function X(t) { return padL + inset + (one ? (iw - 2 * inset) / 2 : ((t - t0) / span) * (iw - 2 * inset)); }
      // Where datum k sits: by its place in the row when ordinal, else by its time.
      var ord = !!o.ordinal;
      if (ord && !bars) inset = iw * (data.length <= 6 ? 0.1 : 0.03);
      function XK(k) { return ord ? padL + inset + (data.length === 1 ? (iw - 2 * inset) / 2 : (k / (data.length - 1)) * (iw - 2 * inset)) : X(data[k].t); }
      function Y(v) { return padT + ih - ((v - vmin) / (vmax - vmin)) * ih; }
      var gid = "gkg" + (++uid);
      var defs = el("defs", {}, svg);
      var g = el("linearGradient", { id: gid, x1: 0, y1: 0, x2: 0, y2: 1 }, defs);
      el("stop", { offset: "0%", class: "gk-stop-a" }, g);
      el("stop", { offset: "100%", class: "gk-stop-b" }, g);
      ticks.forEach(function (v) {
        el("line", { x1: padL, x2: W - padR, y1: Y(v), y2: Y(v), class: "gk-grid" }, svg);
        el("text", { x: padL - 8, y: Y(v) + 4, "text-anchor": "end", class: "gk-axis" }, svg).textContent = (o.fmt || String)(v, true);
      });
      var nx = one ? 0 : W < 420 ? 2 : 4;
      if (ord) {
        // A label under a few of the points, each with that point's own time.
        var real0 = data[0].t, real1 = data[data.length - 1].t, shown = {};
        nx = Math.min(nx, data.length - 1);
        for (var j = 0; j <= nx; j++) {
          var k = nx === 0 ? 0 : Math.round(((data.length - 1) * j) / nx);
          if (shown[k]) continue;
          shown[k] = 1;
          el("text", { x: XK(k), y: H - 7, "text-anchor": data.length === 1 ? "middle" : j === 0 ? "start" : j === nx ? "end" : "middle", class: "gk-axis" }, svg).textContent = timeLabel(data[k].t, o.clock ? real1 - real0 : Math.max(3 * 86400, real1 - real0));
        }
      } else {
        for (var i = 0; i <= nx; i++) {
          var tt = t0 + (span * i) / nx || t0;
          el("text", { x: X(tt), y: H - 7, "text-anchor": nx === 0 ? "middle" : i === 0 ? "start" : i === nx ? "end" : "middle", class: "gk-axis" }, svg).textContent = timeLabel(tt, o.days ? 3 * 86400 : span);
        }
      }
      if (o.target != null) el("line", { x1: padL, x2: W - padR, y1: Y(o.target), y2: Y(o.target), class: "gk-target" }, svg);
      // Where each point is drawn. far: 1 above the scale, -1 below it.
      var hov = [];
      var pts = data.map(function (d, k) {
        if (!isNum(d)) { if (d.note || d.none) hov.push({ x: XK(k), y: Y(vmin), d: d, gap: true }); return null; }
        var far = bars ? 0 : d.v > vmax ? 1 : d.v < vmin ? -1 : 0;
        var p = { x: XK(k), y: far > 0 ? padT + 5 : far < 0 ? padT + ih - 5 : Y(d.v), far: far, d: d };
        hov.push(p);
        return p;
      });
      if (bars) {
        pts.forEach(function (p, k) {
          var d = data[k], x = XK(k);
          if (!p || d.none) { el("rect", { x: x - bw / 2, y: Y(vmin) - 1.5, width: bw, height: 1.5, class: "gk-bar none" }, svg); return; }
          var y = Math.max(padT, p.y);
          el("rect", { x: x - bw / 2, y: y, width: bw, height: Math.max(1.5, Y(vmin) - y), rx: Math.min(3, bw / 3), class: "gk-bar" }, svg);
        });
      } else {
        // The line joins the points it trusts, run by run; a gap ends a run.
        var runs = [], cur = [];
        pts.forEach(function (p) {
          if (!p) { if (cur.length) { runs.push(cur); cur = []; } return; }
          if (p.far || p.d.soft) return;
          cur.push([p.x, p.y]);
        });
        if (cur.length) runs.push(cur);
        var first = !reduce && !low && !host._drawn;
        runs.forEach(function (run) {
          if (run.length < 2) return;
          var line = o.straight ? "M" + run.map(function (p) { return p[0].toFixed(1) + "," + p[1].toFixed(1); }).join("L") : smooth(run);
          if (o.kind !== "line") {
            el("path", { d: line + "L" + run[run.length - 1][0].toFixed(1) + "," + Y(vmin) + "L" + run[0][0].toFixed(1) + "," + Y(vmin) + "Z", fill: "url(#" + gid + ")", class: "gk-area" }, svg);
          }
          var path = el("path", { d: line, class: "gk-line" }, svg);
          if (first) {
            var len = path.getTotalLength ? path.getTotalLength() : 0;
            if (len) {
              path.style.strokeDasharray = len; path.style.strokeDashoffset = len;
              path.getBoundingClientRect();
              path.style.transition = "stroke-dashoffset 1.1s cubic-bezier(.2,.7,.2,1)";
              path.style.strokeDashoffset = 0;
            }
          }
        });
        pts.forEach(function (p) {
          if (!p) return;
          if (p.far) {
            el("circle", { cx: p.x, cy: p.y, r: 3.5, class: "gk-far" }, svg);
            var s = p.far > 0 ? -1 : 1, y0 = p.y + s * 6.5;
            el("path", { d: "M" + (p.x - 3).toFixed(1) + "," + y0.toFixed(1) + "L" + p.x.toFixed(1) + "," + (y0 + s * 3).toFixed(1) + "L" + (p.x + 3).toFixed(1) + "," + y0.toFixed(1), class: "gk-far-tick" }, svg);
          } else if (p.d.soft) el("circle", { cx: p.x, cy: p.y, r: 2.5, class: "gk-soft" }, svg);
        });
        runs.forEach(function (run) {
          if (o.points || run.length === 1) run.forEach(function (p) { el("circle", { cx: p[0], cy: p[1], r: 3, class: "gk-pt" }, svg); });
        });
        if (runs.length) {
          var lr = runs[runs.length - 1], last = lr[lr.length - 1];
          el("circle", { cx: last[0], cy: last[1], r: 3.5, class: "gk-last" }, svg);
        }
      }
      // The target's name goes over the series, so a tall bar or the line does not cut it.
      if (o.target != null && o.targetLabel) el("text", { x: W - padR - 4, y: Y(o.target) - 6, "text-anchor": "end", class: "gk-axis gk-target-label" }, svg).textContent = o.targetLabel;
      host._drawn = true;
      var cross = el("line", { y1: padT, y2: padT + ih, class: "gk-cross", visibility: "hidden" }, svg);
      var dot = el("circle", { r: 4.5, class: "gk-dot", visibility: "hidden" }, svg);
      function move(ev) {
        if (!hov.length) return;
        var r = svg.getBoundingClientRect(), x = ((ev.clientX - r.left) / r.width) * W;
        var best = 0, bd = Infinity;
        hov.forEach(function (p, k) { var d = Math.abs(p.x - x); if (d < bd) { bd = d; best = k; } });
        var p = hov[best], d = p.d;
        cross.setAttribute("x1", p.x); cross.setAttribute("x2", p.x); cross.setAttribute("visibility", "visible");
        dot.setAttribute("cx", p.x); dot.setAttribute("cy", p.y); dot.setAttribute("visibility", p.gap ? "hidden" : "visible");
        var when = o.tipTime ? o.tipTime(d) : new Date(d.t * 1000).toLocaleString(lang(), { month: "short", day: "numeric", hour: "2-digit", minute: "2-digit" });
        var val = p.gap ? d.note || "—" : (o.fmt || String)(d.v);
        tip.innerHTML = "<b>" + esc(val) + "</b><span>" + esc(when) + (d.h != null && isFinite(d.h) ? " · " + TE("block {h}", { h: Number(d.h) }) : "") + "</span>" + (d.note && !p.gap ? "<i>" + esc(d.note) + "</i>" : "");
        tip.classList.remove("hidden");
        var left = (p.x / W) * r.width;
        tip.style.left = Math.min(r.width - tip.offsetWidth - 4, Math.max(4, left - tip.offsetWidth / 2)) + "px";
        tip.style.top = Math.max(0, (p.y / H) * r.height - tip.offsetHeight - 14) + "px";
      }
      function leave() { cross.setAttribute("visibility", "hidden"); dot.setAttribute("visibility", "hidden"); tip.classList.add("hidden"); }
      svg.addEventListener("pointermove", move);
      svg.addEventListener("pointerdown", move);
      // A finger that lifts leaves the chart: what it tapped stays shown until the next tap
      // somewhere else. A mouse that leaves takes its figure with it.
      svg.addEventListener("pointerleave", function (ev) { if (ev.pointerType !== "touch") leave(); });
      svg.addEventListener("pointercancel", function (ev) { if (ev.pointerType !== "touch") leave(); });
      host._leave = leave;
      if (!host._out) {
        host._out = true;
        document.addEventListener("pointerdown", function (ev) { if (!host.contains(ev.target) && host._leave) host._leave(); }, { passive: true });
      }
    }
    state.draw = draw;
    draw();
    if (!host._ro && "ResizeObserver" in window) {
      var w0 = host.clientWidth;
      host._ro = new ResizeObserver(function () { if (Math.abs(host.clientWidth - w0) > 4) { w0 = host.clientWidth; host._gk.draw(); } });
      host._ro.observe(host);
    }
    return { update: function (d, o2) { state.data = d || []; if (o2) state.opts = o2; draw(); } };
  }

  /* A 3D disc showing issued vs remaining supply. */
  function disc(host, issued, max) {
    var frac = max > 0 ? Math.min(1, issued / max) : 0;
    if (!host._disc) {
      host.classList.add("gk-disc");
      host.innerHTML = '<div class="gk-disc-scene"><div class="gk-disc-stack"></div></div><div class="gk-disc-label"><b></b><span></span></div>';
      var stack = host.querySelector(".gk-disc-stack");
      for (var i = 0; i < 7; i++) {
        var layer = document.createElement("div");
        layer.className = "gk-disc-layer" + (i === 6 ? " top" : "");
        layer.style.transform = "translateZ(" + i * 2.2 + "px)";
        layer.innerHTML = '<svg viewBox="0 0 100 100"><circle cx="50" cy="50" r="40" class="gk-disc-rest"/><circle cx="50" cy="50" r="40" class="gk-disc-fill" pathLength="1000"/></svg>';
        stack.appendChild(layer);
      }
      host._disc = true;
    }
    host.querySelectorAll(".gk-disc-fill").forEach(function (c) { c.style.strokeDasharray = Math.max(frac * 1000, 2) + " 1000"; });
    host.querySelector(".gk-disc-label b").textContent = (frac * 100).toFixed(frac < 0.01 ? 4 : 2) + "%";
    host.querySelector(".gk-disc-label span").textContent = T("issued");
  }

  /* Cards used to tilt under the pointer. They are flat now: the light that follows the cursor
     (fx.js) does that job. Kept so that callers need no change. */
  function tilt() {}

  function compact(v, unit) {
    var d = v >= 100 ? 0 : v >= 10 ? 1 : 2;
    return Number(v.toFixed(d)).toString() + unit;
  }
  function fmtNum(v) {
    v = Number(v || 0);
    var a = Math.abs(v);
    return a >= 1e12 ? compact(v / 1e12, " T") : a >= 1e9 ? compact(v / 1e9, " G") : a >= 1e6 ? compact(v / 1e6, " M") : a >= 1e3 ? compact(v / 1e3, " k") : Number(v.toFixed(a < 10 && v % 1 ? 1 : 0)).toString();
  }
  function fmtRate(v) {
    v = Number(v || 0);
    var a = Math.abs(v);
    return a >= 1e12 ? compact(v / 1e12, " TH/s") : a >= 1e9 ? compact(v / 1e9, " GH/s") : a >= 1e6 ? compact(v / 1e6, " MH/s") : a >= 1e3 ? compact(v / 1e3, " kH/s") : Math.round(v) + " H/s";
  }

  /* A complete dashboard for /v1/stats. root contains [data-range] buttons and [data-chart]/[data-kpi] slots. */
  function dashboard(root, fetchStats, onSummary) {
    var charts = {}, current = root.getAttribute("data-default-range") || "24h";
    var defs = {
      hashrate: { kind: "area", fmt: fmtRate, label: T("Network hashrate") },
      difficulty: { kind: "line", fmt: fmtNum, label: T("Difficulty"), zero: false },
      block_time: { kind: "bars", fmt: function (v) { return v.toFixed(v < 10 ? 1 : 0) + " s"; }, target: 60, targetLabel: T("target 60 s"), label: T("Block time") },
      shares: { kind: "area", fmt: function (v) { return v.toFixed(1); }, target: 32, targetLabel: T("expected 32"), label: T("Shares per block") },
      txs: { kind: "bars", fmt: function (v) { return v.toFixed(v < 10 ? 1 : 0); }, label: T("Transactions per block") },
      issued: { kind: "area", fmt: function (v, axis) { v = v / 1e8; return axis ? fmtNum(v) : GKI18N.num(v, { maximumFractionDigits: 0 }) + " GNR"; }, label: T("GNR issued") }
    };
    function load(range) {
      current = range;
      root.querySelectorAll("[data-range]").forEach(function (b) { var on = b.getAttribute("data-range") === range; b.classList.toggle("on", on); b.setAttribute("aria-pressed", String(on)); });
      root.classList.add("loading");
      fetchStats(range).then(function (s) {
        root.classList.remove("loading");
        var pts = s.points || [];
        root.querySelectorAll("[data-chart]").forEach(function (host) {
          var k = host.getAttribute("data-chart"), d = defs[k];
          var series = pts.map(function (p) { return { t: p.t, v: Number(p[k] || 0), h: p.h }; });
          if (charts[k]) charts[k].update(series); else charts[k] = chart(host, series, d);
        });
        if (onSummary) onSummary(s.summary || {}, pts);
      }).catch(function () { root.classList.remove("loading"); });
    }
    root.querySelectorAll("[data-range]").forEach(function (b) { b.addEventListener("click", function () { load(b.getAttribute("data-range")); }); });
    load(current);
    return { reload: function () { load(current); } };
  }

  window.GKCharts = { chart: chart, disc: disc, tilt: tilt, dashboard: dashboard, fmtNum: fmtNum, fmtRate: fmtRate };
})();
