/* Genoryn Krypton charts: dependency-free SVG line/area/bar charts, a 3D supply disc and
   3D card tilt. Monochrome, theme-aware (CSS variables), responsive and accessible. */
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
  function timeLabel(t, span) {
    var d = new Date(t * 1000);
    if (span <= 2 * 86400) return d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
    return d.toLocaleDateString([], { month: "short", day: "numeric" });
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

  /* data: [{t, v}], opts: {fmt, kind: "area"|"line"|"bars", target, targetLabel, height, zero} */
  function chart(host, data, opts) {
    opts = opts || {};
    host.classList.add("gk-chart");
    var state = { data: data || [], opts: opts };
    host._gk = state;
    var tip = document.createElement("div");
    tip.className = "gk-tip float hidden";
    host.innerHTML = "";
    host.appendChild(tip);
    function draw() {
      var data = state.data, o = state.opts, W = Math.max(240, host.clientWidth), H = o.height || 190;
      var old = host.querySelector("svg"); if (old) old.remove();
      var svg = el("svg", { viewBox: "0 0 " + W + " " + H, width: W, height: H, role: "img", "aria-label": o.label || "chart" });
      host.insertBefore(svg, tip);
      if (!data.length) {
        el("text", { x: W / 2, y: H / 2, "text-anchor": "middle", class: "gk-empty" }, svg).textContent = "No data in this range yet";
        return;
      }
      var padL = 54, padR = 10, padT = 12, padB = 26, iw = W - padL - padR, ih = H - padT - padB;
      var t0 = data[0].t, t1 = data[data.length - 1].t, span = Math.max(1, t1 - t0);
      var vs = data.map(function (d) { return d.v; });
      if (o.target != null) vs.push(o.target);
      var vmin = o.zero === false ? Math.min.apply(null, vs) : Math.min(0, Math.min.apply(null, vs));
      var vmax = Math.max.apply(null, vs);
      if (vmax === vmin) vmax = vmin + (vmin === 0 ? 1 : Math.abs(vmin) * 0.1);
      if (o.integer) vmax = Math.max(vmax, vmin + 2);
      var ticks = niceTicks(vmin, vmax, 3);
      if (o.integer && ticks.length > 1 && ticks[1] - ticks[0] < 1) ticks = [vmin, Math.ceil((vmin + vmax) / 2), Math.ceil(vmax)];
      vmin = ticks[0]; vmax = ticks[ticks.length - 1];
      function X(t) { return padL + (data.length === 1 ? iw / 2 : ((t - t0) / span) * iw); }
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
      var nx = W < 420 ? 2 : 4;
      for (var i = 0; i <= nx; i++) {
        var tt = t0 + (span * i) / nx;
        el("text", { x: X(tt), y: H - 7, "text-anchor": i === 0 ? "start" : i === nx ? "end" : "middle", class: "gk-axis" }, svg).textContent = timeLabel(tt, span);
      }
      if (o.target != null) {
        el("line", { x1: padL, x2: W - padR, y1: Y(o.target), y2: Y(o.target), class: "gk-target" }, svg);
        if (o.targetLabel) el("text", { x: W - padR - 4, y: Y(o.target) - 6, "text-anchor": "end", class: "gk-axis" }, svg).textContent = o.targetLabel;
      }
      var pts = data.map(function (d) { return [X(d.t), Y(d.v)]; });
      if (o.kind === "bars") {
        var bw = Math.max(1.5, Math.min(14, (iw / data.length) * 0.66));
        pts.forEach(function (p) {
          el("rect", { x: p[0] - bw / 2, y: p[1], width: bw, height: Math.max(0.5, Y(vmin) - p[1]), rx: Math.min(3, bw / 3), class: "gk-bar" }, svg);
        });
      } else {
        var line = smooth(pts);
        if (o.kind !== "line") {
          el("path", { d: line + "L" + pts[pts.length - 1][0].toFixed(1) + "," + Y(vmin) + "L" + pts[0][0].toFixed(1) + "," + Y(vmin) + "Z", fill: "url(#" + gid + ")", class: "gk-area" }, svg);
        }
        var path = el("path", { d: line, class: "gk-line" }, svg);
        if (!reduce && !low && !host._drawn) {
          var len = path.getTotalLength ? path.getTotalLength() : 0;
          if (len) {
            path.style.strokeDasharray = len; path.style.strokeDashoffset = len;
            path.getBoundingClientRect();
            path.style.transition = "stroke-dashoffset 1.1s cubic-bezier(.2,.7,.2,1)";
            path.style.strokeDashoffset = 0;
          }
        }
        var last = pts[pts.length - 1];
        el("circle", { cx: last[0], cy: last[1], r: 3.5, class: "gk-last" }, svg);
      }
      host._drawn = true;
      var cross = el("line", { y1: padT, y2: padT + ih, class: "gk-cross", visibility: "hidden" }, svg);
      var dot = el("circle", { r: 4.5, class: "gk-dot", visibility: "hidden" }, svg);
      function move(ev) {
        var r = svg.getBoundingClientRect(), x = ((ev.clientX - r.left) / r.width) * W;
        var best = 0, bd = Infinity;
        pts.forEach(function (p, k) { var d = Math.abs(p[0] - x); if (d < bd) { bd = d; best = k; } });
        var p = pts[best], d = data[best];
        cross.setAttribute("x1", p[0]); cross.setAttribute("x2", p[0]); cross.setAttribute("visibility", "visible");
        dot.setAttribute("cx", p[0]); dot.setAttribute("cy", p[1]); dot.setAttribute("visibility", "visible");
        tip.innerHTML = "<b>" + (o.fmt || String)(d.v) + "</b><span>" + new Date(d.t * 1000).toLocaleString([], { month: "short", day: "numeric", hour: "2-digit", minute: "2-digit" }) + (d.h != null && isFinite(d.h) ? " · block " + Number(d.h) : "") + "</span>";
        tip.classList.remove("hidden");
        var left = (p[0] / W) * r.width;
        tip.style.left = Math.min(r.width - tip.offsetWidth - 4, Math.max(4, left - tip.offsetWidth / 2)) + "px";
        tip.style.top = Math.max(0, (p[1] / H) * r.height - tip.offsetHeight - 14) + "px";
      }
      function leave() { cross.setAttribute("visibility", "hidden"); dot.setAttribute("visibility", "hidden"); tip.classList.add("hidden"); }
      svg.addEventListener("pointermove", move);
      svg.addEventListener("pointerdown", move);
      svg.addEventListener("pointerleave", leave);
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
    host.querySelector(".gk-disc-label span").textContent = "issued";
  }

  /* Subtle 3D tilt with a moving glare for cards on pointer devices. */
  function tilt(selector) {
    if (reduce || low || !(window.matchMedia && matchMedia("(hover: hover) and (pointer: fine)").matches)) return;
    document.querySelectorAll(selector).forEach(function (c) {
      if (c._tilt) return;
      c._tilt = true;
      c.classList.add("gk-tilt");
      c.addEventListener("pointermove", function (e) {
        var r = c.getBoundingClientRect(), x = (e.clientX - r.left) / r.width, y = (e.clientY - r.top) / r.height;
        c.style.transform = "perspective(900px) rotateX(" + ((0.5 - y) * 6).toFixed(2) + "deg) rotateY(" + ((x - 0.5) * 7).toFixed(2) + "deg) translateZ(0)";
        c.style.setProperty("--gx", (x * 100).toFixed(1) + "%");
        c.style.setProperty("--gy", (y * 100).toFixed(1) + "%");
      });
      c.addEventListener("pointerleave", function () { c.style.transform = ""; });
    });
  }

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
      hashrate: { kind: "area", fmt: fmtRate, label: "Network hashrate" },
      difficulty: { kind: "line", fmt: fmtNum, label: "Difficulty", zero: false },
      block_time: { kind: "bars", fmt: function (v) { return v.toFixed(v < 10 ? 1 : 0) + " s"; }, target: 60, targetLabel: "target 60 s", label: "Block time" },
      shares: { kind: "area", fmt: function (v) { return v.toFixed(1); }, target: 32, targetLabel: "expected 32", label: "Shares per block" },
      miners: { kind: "bars", fmt: function (v) { return String(Math.round(v)); }, integer: true, label: "Active miners" },
      txs: { kind: "bars", fmt: function (v) { return v.toFixed(v < 10 ? 1 : 0); }, label: "Transactions per block" },
      issued: { kind: "area", fmt: function (v, axis) { v = v / 1e8; return axis ? fmtNum(v) : v.toLocaleString("en-US", { maximumFractionDigits: 0 }) + " GNR"; }, label: "GNR issued" }
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
