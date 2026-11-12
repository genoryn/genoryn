/* genoryn.io: navbar state, drawer, live network numbers. Small and dependency free. */
(function () {
  var nav = document.getElementById("nav");
  var on = false;
  function onScroll() {
    var s = window.scrollY > 8;
    if (s !== on) { on = s; nav.classList.toggle("scrolled", s); }
  }
  window.addEventListener("scroll", onScroll, { passive: true });
  onScroll();
  var page = document.body.getAttribute("data-page");
  document.querySelectorAll(".nav-links a[data-page]").forEach(function (a) {
    if (a.getAttribute("data-page") === page) { a.classList.add("on", "rail"); a.setAttribute("aria-current", "page"); }
  });
  window.siteDrawer = function (open) {
    document.getElementById("drawer").classList.toggle("open", open);
    document.getElementById("drawerBackdrop").classList.toggle("hidden", !open);
    document.querySelector(".menu-btn").setAttribute("aria-expanded", String(open));
  };
  document.addEventListener("keydown", function (e) { if (e.key === "Escape") window.siteDrawer(false); });
  // Buttons declare their action in data-act: no inline handlers, so the CSP can forbid inline script.
  var acts = {
    theme: function () { window.fxToggleTheme(); },
    "drawer-open": function () { window.siteDrawer(true); },
    "drawer-close": function () { window.siteDrawer(false); },
    toc: function () { if (window.docsToc) window.docsToc(); },
    proof: function () { if (window.exProof) window.exProof(); }
  };
  document.addEventListener("click", function (e) {
    var el = e.target.closest ? e.target.closest("[data-act]") : null;
    var f = el && acts[el.getAttribute("data-act")];
    if (f) { e.preventDefault(); f(); }
  });
  document.addEventListener("submit", function (e) {
    if (e.target.getAttribute("data-act") === "search" && window.exSearch) window.exSearch(e);
  });

  // Docs: table of contents drawer (phones) and scrollspy.
  var toc = document.getElementById("toc");
  if (toc) {
    window.docsToc = function (open) {
      var o = open == null ? !toc.classList.contains("open") : open;
      toc.classList.toggle("open", o);
      document.querySelector(".toc-toggle").setAttribute("aria-expanded", String(o));
    };
    toc.addEventListener("click", function (e) { if (e.target.tagName === "A") window.docsToc(false); });
    var links = {};
    toc.querySelectorAll("a").forEach(function (a) { links[a.getAttribute("href").slice(1)] = a; });
    if ("IntersectionObserver" in window) {
      var spy = new IntersectionObserver(function (es) {
        es.forEach(function (en) {
          if (en.isIntersecting && links[en.target.id]) {
            toc.querySelectorAll("a.on").forEach(function (x) { x.classList.remove("on"); });
            links[en.target.id].classList.add("on");
          }
        });
      }, { rootMargin: "-20% 0px -70% 0px" });
      document.querySelectorAll(".docs-body section[id]").forEach(function (s) { spy.observe(s); });
    }
  }

  window.gk = {
    api: function (p) { return fetch(p, { headers: { accept: "application/json" } }).then(function (r) { if (!r.ok) throw new Error(r.status); return r.json(); }); },
    gnr: function (v, d) { return (Number(v || 0) / 1e8).toLocaleString("en-US", { minimumFractionDigits: d == null ? 2 : d, maximumFractionDigits: d == null ? 8 : d }); },
    rate: function (r) { r = Number(r || 0); return r >= 1e15 ? (r / 1e15).toFixed(2) + " PH/s" : r >= 1e12 ? (r / 1e12).toFixed(2) + " TH/s" : r >= 1e9 ? (r / 1e9).toFixed(2) + " GH/s" : r >= 1e6 ? (r / 1e6).toFixed(2) + " MH/s" : r >= 1e3 ? (r / 1e3).toFixed(1) + " kH/s" : r.toFixed(0) + " H/s"; },
    esc: function (s) { return String(s == null ? "" : s).replace(/[&<>"]/g, function (c) { return { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c]; }); },
    ago: function (t) { var s = Math.max(0, Math.floor(Date.now() / 1000 - t)); return s < 60 ? s + " s ago" : s < 3600 ? Math.floor(s / 60) + " min ago" : Math.floor(s / 3600) + " h ago"; },
    miners: function (st) {
      var m = document.getElementById("sMiners"), g = document.getElementById("sGpus");
      if (!m || !st) return;
      m.textContent = Number(st.estimated_miners || 0).toLocaleString("en-US");
      var eq = Number(st.gpu_equivalents || 0);
      if (g) g.textContent = "≈ " + (eq < 10 ? eq.toFixed(1) : Math.round(eq).toLocaleString("en-US")) + " GPU" + (eq >= 1.05 || eq < 0.95 ? "s" : "");
    }
  };

  // The GNR price: the offer board's market price, nothing else.
  gk.value = function (info) {
    if (!window.GKValue) return;
    var put = function (sel, text) { document.querySelectorAll(sel).forEach(function (el) { el.textContent = text; }); };
    var mk = gk.mk || {}, btc = Number(gk.btcUsd) || 0, sat = GKValue.btc;
    var usd = GKValue.market(mk.price, btc);
    var dollars = function (p) { var v = GKValue.market(p, btc); return v != null ? " (" + GKValue.usd(v) + ")" : ""; };
    var nOffers = (mk.asks || 0) + (mk.bids || 0);
    put("[data-mk-price]", mk.price ? sat(mk.price) + dollars(mk.price) : "no swaps yet");
    put("[data-mk-book]", (mk.best_ask ? sat(mk.best_ask) : "—") + " / " + (mk.best_bid ? sat(mk.best_bid) : "—"));
    put("[data-mk-ask]", mk.best_ask ? sat(mk.best_ask) : "—");
    put("[data-mk-bid]", mk.best_bid ? sat(mk.best_bid) : "—");
    put("[data-mk-last]", mk.last_price ? sat(mk.last_price) : "no swap yet");
    put("[data-mk-swaps]", (mk.trades || 0) + " done · " + (mk.pending || 0) + " in progress");
    put("[data-mk-usd]", usd != null ? GKValue.usd(usd) : mk.price ? sat(mk.price) : "no offers yet");
    put("[data-mk-sat]", mk.price ? sat(mk.price) + " per GNR" : "be the first to make an offer");
    put("[data-mk-src]", mk.source === "swaps" ? "median of swaps, swaps in progress and the order book"
      : mk.source === "pending" ? "median of " + mk.pending + " swap" + (mk.pending === 1 ? "" : "s") + " in progress and the order book"
      : mk.source === "offers" ? "from " + nOffers + " open offer" + (nOffers === 1 ? "" : "s") : "set by people on the offer board");
    var conf = Number(mk.confidence) || 0;
    put("[data-ix-conf]", mk.price ? (mk.depth || (conf >= 0.8 ? "deep" : conf >= 0.4 ? "moderate" : "thin")) + " · " + Math.round(conf * 100) + " %" : "—");
    document.querySelectorAll("[data-ix-bar]").forEach(function (el) { el.style.width = Math.max(2, Math.round(conf * 100)) + "%"; });
    put("[data-main]", usd != null ? GKValue.usd(usd) : mk.price ? sat(mk.price) : "—");
    put("[data-vlabel]", "GNR price");
    // What one GPU mines a day, and what that is worth at the market price.
    var g = info ? GKValue.perGpu(info.hashrate, info.next_subsidy) : null;
    if (g != null) {
      put("[data-gpu-gnr]", "≈ " + g.toLocaleString("en-US", { maximumFractionDigits: g < 10 ? 2 : 0 }) + " GNR a day");
      put("[data-gpu-value]", usd != null ? GKValue.usd(g * usd) + " a day at the market price" : "no market price yet");
    }
  };
  if (document.querySelector("[data-mk-usd], [data-main], [data-mk-price]")) {
    var loadMarket = function () {
      var info = gk.api("/v1/info").catch(function () { return null; });
      var mk = gk.api("/v1/swap/market").then(function (m) { gk.mk = m.market; }).catch(function () {});
      var btc = gk.api("/v1/btc-price").then(function (p) { gk.btcUsd = p.USD; }).catch(function () {});
      Promise.all([info, mk, btc]).then(function (r) { gk.value(r[0]); });
    };
    loadMarket();
    setInterval(function () { if (!document.hidden) loadMarket(); }, 60000);
  }

// Mining calculator (genoryn.io/mining): live block height and miners from the node.
  var calcEl = document.getElementById("mcCalc");
  if (calcEl && window.GKValue) {
    var liveInfo = {};
    var calc = GKValue.mountCalc(calcEl, function () {
      return { height: liveInfo.height, miners: liveInfo.stats && liveInfo.stats.estimated_miners };
    });
    var loadCalc = function () { gk.api("/v1/info").then(function (i) { liveInfo = i; calc.update(); }).catch(function () {}); };
    loadCalc();
    setInterval(function () { if (!document.hidden) loadCalc(); }, 60000);
  }
  var homeNet = document.getElementById("homeNet");
  if (homeNet && window.GKCharts) {
    var homeChart = null;
    var loadNet = function () {
      gk.api("/v1/stats?range=24h").then(function (s) {
        var series = (s.points || []).map(function (p) { return { t: p.t, v: p.hashrate, h: p.h }; });
        if (homeChart) homeChart.update(series);
        else homeChart = GKCharts.chart(document.getElementById("homeChart"), series, { kind: "area", fmt: GKCharts.fmtRate, label: "Network hashrate, last 24 hours", height: 170 });
        var sm = s.summary || {};
        document.getElementById("homeRate").textContent = gk.rate(sm.hashrate);
        document.getElementById("homeIssued").textContent = gk.gnr(sm.issued, 0);
        document.getElementById("homeLeft").textContent = gk.gnr(sm.remaining, 0);
        GKCharts.disc(document.getElementById("homeDisc"), Number(sm.issued || 0), Number(sm.max_supply || 21e14));
      }).catch(function () {});
    };
    loadNet();
    setInterval(function () { if (!document.hidden) loadNet(); }, 60000);
  }
  if (window.GKCharts) GKCharts.tilt(".tilt-card");

  var h = document.getElementById("sHeight");
  if (h) {
    var load = function () {
      gk.api("/v1/info").then(function (i) {
        h.textContent = Number(i.height).toLocaleString("en-US");
        document.getElementById("sSupply").textContent = gk.gnr(i.supply, 0);
        document.getElementById("sRate").textContent = gk.rate(i.hashrate);
        gk.miners(i.stats);
        var lt = document.getElementById("liveText");
        if (lt) lt.textContent = (i.network === "mainnet" ? "Mainnet" : "Testnet") + " is live · block " + Number(i.height).toLocaleString("en-US");
      }).catch(function () {
        var d = document.getElementById("liveDot");
        if (d) { d.classList.remove("ok", "live"); d.classList.add("warn"); }
        var lt = document.getElementById("liveText");
        if (lt) lt.textContent = "Network status unavailable";
      });
    };
    load();
    setInterval(function () { if (!document.hidden) load(); }, 30000);
  }
})();
