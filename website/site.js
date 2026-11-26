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
  // Menus are longer in some languages. When the row does not fit, it first tightens step by
  // step (fit1..fit4 in site.css); only if it still does not fit does the header use the
  // drawer, as on smaller screens. The row is measured by its visible items: the closed
  // dropdown panels are laid out too and would count in scrollWidth.
  var navRow = nav.querySelector(".wrap"), navLinks = nav.querySelector(".nav-links"), navCta = nav.querySelector(".nav-cta");
  var FITS = ["fit1", "fit2", "fit3", "fit4"];
  function navOver() {
    if (!navRow || !navLinks || !navCta || getComputedStyle(navLinks).display === "none") return false;
    var row = navRow.getBoundingClientRect(), pad = parseFloat(getComputedStyle(navRow).paddingRight) || 0;
    var last = navLinks.lastElementChild.getBoundingClientRect().right, cta = navCta.getBoundingClientRect();
    return last > cta.left - 6 || cta.right > row.right - pad + 1;
  }
  function fitNav() {
    nav.classList.remove("nav-compact");
    FITS.forEach(function (c) { nav.classList.remove(c); });
    for (var i = 0; i < FITS.length && navOver(); i++) nav.classList.add(FITS[i]);
    if (navOver()) nav.classList.add("nav-compact");
  }
  fitNav();
  window.addEventListener("resize", fitNav);
  if (document.fonts && document.fonts.ready) document.fonts.ready.then(fitNav);
  var page = document.body.getAttribute("data-page");
  document.querySelectorAll(".nav-links a[data-page]").forEach(function (a) {
    if (a.getAttribute("data-page") === page) { a.classList.add("on", "rail"); a.setAttribute("aria-current", "page"); }
  });
  // Menus of the navbar: they open on hover and on focus by CSS; a click (or a touch)
  // pins one open, and a click elsewhere or Escape closes it.
  var menus = Array.prototype.slice.call(document.querySelectorAll(".nav-dd"));
  function closeMenus(except) {
    menus.forEach(function (d) {
      if (d === except) return;
      d.classList.remove("open");
      d.querySelector(".nav-dd-btn").setAttribute("aria-expanded", "false");
    });
  }
  menus.forEach(function (d) {
    var btn = d.querySelector(".nav-dd-btn");
    if ((" " + (d.getAttribute("data-pages") || "") + " ").indexOf(" " + page + " ") >= 0) btn.classList.add("on");
    btn.addEventListener("click", function (e) {
      e.preventDefault();
      var open = !d.classList.contains("open");
      closeMenus(d);
      d.classList.toggle("open", open);
      btn.setAttribute("aria-expanded", String(open));
    });
    d.addEventListener("keydown", function (e) {
      if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
      var items = Array.prototype.slice.call(d.querySelectorAll(".nav-menu a"));
      var at = items.indexOf(document.activeElement);
      e.preventDefault();
      if (!d.classList.contains("open")) { d.classList.add("open"); btn.setAttribute("aria-expanded", "true"); }
      items[(at + (e.key === "ArrowDown" ? 1 : -1) + items.length) % items.length].focus();
    });
  });
  document.addEventListener("click", function (e) {
    if (!(e.target.closest && e.target.closest(".nav-dd"))) closeMenus(null);
  });
  document.addEventListener("keydown", function (e) {
    if (e.key !== "Escape") return;
    var open = document.querySelector(".nav-dd.open, .nav-dd:focus-within");
    closeMenus(null);
    if (open && open.contains(document.activeElement)) open.querySelector(".nav-dd-btn").blur();
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

  // Search (Ctrl+K): every page by a few letters of its name, and the chain by a block height,
  // a hash or an address. What is typed stays in the page until a result is opened.
  if (window.GKCmd) {
    document.querySelectorAll("[data-cmdk-key]").forEach(function (k) { k.textContent = GKCmd.key; });
    var cmdPages = null;
    var cmdList = function () {
      if (cmdPages) return cmdPages;
      var hints = {}, seen = {};
      document.querySelectorAll(".nav-menu a[href]").forEach(function (a) {
        var em = a.querySelector("em");
        if (em && !hints[a.getAttribute("href")]) hints[a.getAttribute("href")] = em.textContent.trim();
      });
      cmdPages = [];
      document.querySelectorAll("#drawer > a[href]").forEach(function (a) {
        var href = a.getAttribute("href"), use = a.querySelector("use"), b = a.querySelector("b"), em = a.querySelector("em");
        if (seen[href]) return;
        seen[href] = 1;
        var label = b ? b.textContent : a.textContent;
        cmdPages.push({ label: label.replace(/\s+/g, " ").trim(), hint: em ? em.textContent.trim() : hints[href] || "", href: href, icon: use ? use.getAttribute("href") : "", words: href });
      });
      return cmdPages;
    };
    GKCmd.setup({
      slash: true,
      text: { title: T("Search"), placeholder: T("Search pages, blocks, transactions, addresses"), empty: T("Nothing found"), move: T("to move"), open: T("to open"), close: T("to close") },
      groups: function () {
        var themeBtn = document.querySelector(".nav-theme");
        return [
          { title: T("Pages"), items: cmdList() },
          { title: T("Actions"), items: [{ label: themeBtn ? themeBtn.getAttribute("aria-label") : "Theme", icon: "#n-theme", run: function () { window.fxToggleTheme(); } }] }
        ];
      },
      query: function (q) {
        q = q.replace(/\s+/g, "");
        if (q.length < 1 || q.length > 4000) return [];
        var hex = /^[0-9a-fA-F]{64}$/.test(q), num = /^\d{1,10}$/.test(q), addr = /^[a-z]*gnrs?1[a-z2-7]+$/.test(q.toLowerCase());
        if (!hex && !num && !addr && q.length < 24) return [];
        var short = q.length > 26 ? q.slice(0, 12) + "…" + q.slice(-10) : q;
        return [{ group: T("On the chain"), label: num ? T("Block #{h}", { h: GKI18N.num(Number(q)) }) : T("Search the chain for {q}", { q: short }),
          hint: addr ? T("Address") : hex ? T("Block, transaction or nullifier hash") : "", icon: "#n-explorer",
          href: GKI18N.path("/explorer") + "?q=" + encodeURIComponent(q) }];
      }
    });
  }

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
    gnr: function (v, d) { return GKI18N.num(Number(v || 0) / 1e8, { minimumFractionDigits: d == null ? 2 : d, maximumFractionDigits: d == null ? 8 : d }); },
    rate: function (r) { r = Number(r || 0); return r >= 1e15 ? (r / 1e15).toFixed(2) + " PH/s" : r >= 1e12 ? (r / 1e12).toFixed(2) + " TH/s" : r >= 1e9 ? (r / 1e9).toFixed(2) + " GH/s" : r >= 1e6 ? (r / 1e6).toFixed(2) + " MH/s" : r >= 1e3 ? (r / 1e3).toFixed(1) + " kH/s" : r.toFixed(0) + " H/s"; },
    esc: function (s) { return String(s == null ? "" : s).replace(/[&<>"']/g, function (c) { return { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]; }); },
    ago: function (t) {
      var s = Math.max(0, Math.floor(Date.now() / 1000 - t));
      return s < 60 ? T("{n} s ago", { n: s }) : s < 3600 ? T("{n} min ago", { n: Math.floor(s / 60) }) : T("{n} h ago", { n: Math.floor(s / 3600) });
    },
    // The difficulty of the next block. How many computers mine is known to nobody, so it
    // is never shown: the hashrate and the difficulty measure the work itself.
    difficulty: function (info) {
      var d = document.getElementById("sDiff");
      if (d && info && info.difficulty && window.GKCharts) d.textContent = GKCharts.fmtNum(Number(info.difficulty));
    }
  };
  // The language picked by hand wins; the browser's language is used once, on the home page.
  document.addEventListener("click", function (e) {
    var a = e.target.closest ? e.target.closest("a[data-lang]") : null;
    if (a) try { localStorage.setItem("gk-lang", a.getAttribute("data-lang")); } catch (x) {}
  });
  (function () {
    if (location.pathname !== "/" || /bot|crawl|spider|slurp|lighthouse/i.test(navigator.userAgent) || navigator.webdriver) return;
    var chosen = null, asked = null;
    try { chosen = localStorage.getItem("gk-lang"); asked = localStorage.getItem("gk-lang-auto"); } catch (x) { return; }
    var have = Array.prototype.map.call(document.querySelectorAll(".nav-lang-menu a[data-lang]"), function (a) { return a.getAttribute("data-lang"); });
    if (chosen) { if (chosen !== "en" && have.indexOf(chosen) >= 0) location.replace("/" + chosen + "/"); return; }
    if (asked) return;
    try { localStorage.setItem("gk-lang-auto", "1"); } catch (x) { return; }
    var want = (navigator.languages || [navigator.language || ""]).map(function (l) { return String(l).toLowerCase(); });
    for (var i = 0; i < want.length; i++) {
      var w = want[i], code = w.indexOf("zh") === 0 ? (/tw|hk|mo|hant/.test(w) ? "" : "zh-cn") : w.split("-")[0];
      if (code === "en") return;
      if (code && have.indexOf(code) >= 0) { location.replace("/" + code + "/"); return; }
    }
  })();

  // The state of the market as the board names it (pricing v2, 1.0.53).
  gk.marketStatus = function (s) {
    return ({ reliable: T("reliable market data"), moderate: T("moderate confidence"), low_liquidity: T("low liquidity"),
      low_confidence: T("low confidence"), stale: T("stale price"), insufficient: T("not enough trading data"), unavailable: T("price unavailable") })[s] || s;
  };
  // Cumulative GNR on offer by price, both sides, around the market price: an SVG (shared by
  // the market page).
  gk.depthChart = function (host, offers, mp, fmt) {
    if (!host) return;
    var side = function (s, asc) {
      return offers.filter(function (o) { return o.side === s && o.online !== false && o.gnr > 0; })
        .map(function (o) { return [o.sats / (o.gnr / 1e8), o.gnr / 1e8]; })
        .sort(function (a, b) { return asc ? a[0] - b[0] : b[0] - a[0]; });
    };
    var asks = side("sell_gnr", true), bids = side("buy_gnr", false);
    if (!asks.length && !bids.length) { host.innerHTML = "<p class=\"muted\">" + gk.esc(T("No offer is online right now.")) + "</p>"; return; }
    var ref = mp || (asks[0] && bids[0] ? (asks[0][0] + bids[0][0]) / 2 : (asks[0] || bids[0])[0]);
    var cum = function (l) { var a = 0; return l.filter(function (x) { return x[0] >= ref / 3 && x[0] <= ref * 3; }).map(function (x) { a += x[1]; return [x[0], a]; }); };
    var A = cum(asks), B = cum(bids), all = A.concat(B);
    if (!all.length) { host.innerHTML = "<p class=\"muted\">" + gk.esc(T("No offer is online near the market price.")) + "</p>"; return; }
    var ps = all.map(function (x) { return x[0]; }).concat([ref]);
    var pmin = Math.min.apply(null, ps), pmax = Math.max.apply(null, ps), vmax = Math.max.apply(null, all.map(function (x) { return x[1]; }));
    var W = Math.max(280, host.clientWidth), H = 190, pl = 6, pr = 6, pt = 10, pb = 24;
    var X = function (p) { var r = Math.log(pmax) - Math.log(pmin); return pl + (r > 0 ? (Math.log(p) - Math.log(pmin)) / r : 0.5) * (W - pl - pr); };
    var Y = function (v) { return pt + (1 - v / vmax) * (H - pt - pb); };
    var path = function (l, end) {
      if (!l.length) return "";
      var d = "M" + X(l[0][0]).toFixed(1) + "," + Y(0).toFixed(1), prev = 0;
      l.forEach(function (x) { d += "L" + X(x[0]).toFixed(1) + "," + Y(prev).toFixed(1) + "L" + X(x[0]).toFixed(1) + "," + Y(x[1]).toFixed(1); prev = x[1]; });
      return d + "L" + end + "," + Y(prev).toFixed(1) + "L" + end + "," + Y(0).toFixed(1) + "Z";
    };
    host.innerHTML = "<svg viewBox=\"0 0 " + W + " " + H + "\" width=\"100%\" height=\"" + H + "\" role=\"img\" aria-label=\"" + gk.esc(T("Market depth")) + "\">" +
      "<path class=\"dp-bid\" d=\"" + path(B, pl) + "\"/><path class=\"dp-ask\" d=\"" + path(A, W - pr) + "\"/>" +
      "<line class=\"dp-ref\" x1=\"" + X(ref).toFixed(1) + "\" x2=\"" + X(ref).toFixed(1) + "\" y1=\"" + pt + "\" y2=\"" + (H - pb) + "\"/>" +
      "<text class=\"gk-axis\" x=\"" + pl + "\" y=\"" + (H - 6) + "\">" + gk.esc(fmt(pmin)) + "</text>" +
      (X(ref) - pl > 110 && W - pr - X(ref) > 110 ? "<text class=\"gk-axis\" x=\"" + X(ref).toFixed(1) + "\" y=\"" + (H - 6) + "\" text-anchor=\"middle\">" + gk.esc(fmt(ref)) + "</text>" : "") +
      "<text class=\"gk-axis\" x=\"" + (W - pr) + "\" y=\"" + (H - 6) + "\" text-anchor=\"end\">" + gk.esc(fmt(pmax)) + "</text></svg>";
  };

  // The GNR price: the offer board's market price, nothing else.
  gk.value = function (info) {
    if (!window.GKValue) return;
    var put = function (sel, text) { document.querySelectorAll(sel).forEach(function (el) { el.textContent = text; }); };
    var mk = gk.mk || {}, btc = Number(gk.btcUsd) || 0, sat = GKValue.btc;
    var usd = GKValue.market(mk.price, btc);
    var dollars = function (p) { var v = GKValue.market(p, btc); return v != null ? " (" + GKValue.usd(v) + ")" : ""; };
    put("[data-mk-price]", mk.price ? sat(mk.price) + dollars(mk.price) : T("no swaps yet"));
    put("[data-mk-book]", (mk.best_ask ? sat(mk.best_ask) : "—") + " / " + (mk.best_bid ? sat(mk.best_bid) : "—"));
    put("[data-mk-ask]", mk.best_ask ? sat(mk.best_ask) : "—");
    put("[data-mk-bid]", mk.best_bid ? sat(mk.best_bid) : "—");
    put("[data-mk-last]", mk.last_price ? sat(mk.last_price) : T("no swap yet"));
    put("[data-mk-swaps]", T("{done} done · {running} in progress", { done: GKI18N.num(mk.trades || 0), running: GKI18N.num(mk.pending || 0) }));
    put("[data-mk-usd]", usd != null ? GKValue.usd(usd) : mk.price ? sat(mk.price) : T("no swaps yet"));
    put("[data-mk-sat]", mk.price ? T("{price} per GNR", { price: sat(mk.price) }) : T("be the first to make an offer"));
    var np = Number(mk.priced) || 0;
    put("[data-mk-src]", mk.price ? Tn(np, "average of {n} completed swap", "average of {n} completed swaps") : T("set by the first completed swap"));
    var conf = Number(mk.confidence) || 0;
    var depth = { deep: T("deep"), moderate: T("moderate"), thin: T("thin") };
    var dk = mk.depth || (conf >= 0.8 ? "deep" : conf >= 0.4 ? "moderate" : "thin");
    // From 1.0.53 the board names the state of the market and scores the price 0–100.
    put("[data-ix-conf]", mk.market_status ? gk.marketStatus(mk.market_status) + (mk.reference_price != null ? " · " + Number(mk.confidence_score || 0) + "/100" : "")
      : mk.price ? (depth[dk] || dk) + " · " + Math.round(conf * 100) + " %" : "—");
    document.querySelectorAll("[data-ix-bar]").forEach(function (el) { el.style.width = Math.max(2, Math.round(conf * 100)) + "%"; });
    put("[data-main]", usd != null ? GKValue.usd(usd) : mk.price ? sat(mk.price) : "—");
    put("[data-vlabel]", T("GNR price"));
    // What one GPU mines a day, and what that is worth at the market price.
    var g = info ? GKValue.perGpu(info.hashrate, info.next_subsidy) : null;
    if (g != null) {
      put("[data-gpu-gnr]", "≈ " + T("{gnr} GNR a day", { gnr: GKI18N.num(g, { maximumFractionDigits: g < 10 ? 2 : 0 }) }));
      put("[data-gpu-value]", usd != null ? T("{usd} a day at the market price", { usd: GKValue.usd(g * usd) }) : T("no market price yet"));
    }
  };
  // Home: the price of every completed swap, oldest first (one point per swap).
  var homePriceChart = null;
  gk.priceChart = function () {
    var host = document.getElementById("homePrice");
    if (!host || !window.GKCharts || !window.GKValue) return;
    var at = function (x) { return (x.claim && x.claim.time) || x.at; };
    var btc = Number(gk.btcUsd) || 0;
    var series = (gk.mkRecent || []).slice().sort(function (a, b) { return at(a) - at(b); })
      .map(function (x) { return { t: at(x), v: Number(x.price) || 0, h: x.claim ? x.claim.height : null }; });
    var opts = { kind: "area", straight: true, points: true, height: 190, label: T("Price per GNR of every completed swap"),
      fmt: function (v, axis) { var u = GKValue.market(v, btc); return axis ? GKCharts.fmtNum(v) + " sat" : GKValue.btc(v) + (u != null ? " · ≈ " + GKValue.usd(u) : ""); } };
    if (homePriceChart) homePriceChart.update(series, opts); else homePriceChart = GKCharts.chart(host, series, opts);
  };
  // Home: the latest completed swaps, each with its proof on the Genoryn chain.
  gk.swapFeed = function () {
    var list = document.getElementById("homeSwaps");
    if (!list || !window.GKValue) return;
    var put = function (id, t) { var el = document.getElementById(id); if (el) el.textContent = t; };
    var mk = gk.mk || {}, btcUsd = Number(gk.btcUsd) || 0, now = Date.now() / 1000;
    var at = function (x) { return (x.claim && x.claim.time) || x.at || 0; };
    var all = (gk.mkRecent || []).slice().sort(function (a, b) { return at(b) - at(a); });
    var done = all.filter(function (x) { return x.claim; });
    var g = 0, s = 0;
    done.forEach(function (x) { g += Number(x.gnr) || 0; s += Number(x.sats) || 0; });
    var gnr = function (v) { return GKI18N.num(v / 1e8, { maximumFractionDigits: v >= 1e10 ? 0 : 2 }); };
    var btc = function (v) { return GKI18N.num(v / 1e8, { minimumFractionDigits: 4, maximumFractionDigits: 8 }); };
    var ago = function (sec) { return sec >= 86400 ? Tn(Math.floor(sec / 86400), "{n} day", "{n} days") : sec >= 3600 ? T("{n} h", { n: Math.floor(sec / 3600) }) : T("{n} min", { n: Math.max(1, Math.floor(sec / 60)) }); };
    put("hsDone", GKI18N.num(done.length));
    put("hsGnr", done.length ? gnr(g) + " GNR" : "—");
    put("hsBtc", done.length ? btc(s) + " BTC" : "—");
    put("hsLast", all.length ? T("{t} ago", { t: ago(Math.max(0, now - at(all[0]))) }) : "—");
    put("hsRun", T("Swaps, 24 h") + ": " + T("{done} done · {running} in progress", { done: GKI18N.num(mk.trades || 0), running: GKI18N.num(mk.pending || 0) }));
    var explorer = GKI18N.path("/explorer");
    var e = gk.esc;
    list.innerHTML = all.length ? all.slice(0, 6).map(function (x) {
      var c = x.claim, p = Number(x.price) || 0, u = GKValue.market(p, btcUsd);
      return "<li class=\"hs-row\">" +
        "<span class=\"hs-pair\"><span class=\"hs-c\">" + GKValue.coin("gnr") + "<b>" + e(gnr(Number(x.gnr) || 0)) + "</b><em>GNR</em></span>" +
        "<i aria-hidden=\"true\">⇄</i><span class=\"hs-c\">" + GKValue.coin("btc") + "<b>" + e(btc(Number(x.sats) || 0)) + "</b><em>BTC</em></span></span>" +
        "<span class=\"hs-px\"><b>" + e(GKValue.btc(p)) + "</b>" + (u != null ? "<em>≈ " + e(GKValue.usd(u)) + "</em>" : "") + "</span>" +
        "<span class=\"hs-when\">" + e(T("{t} ago", { t: ago(Math.max(0, now - at(x))) })) + "</span>" +
        (c ? "<a class=\"hs-proof\" href=\"" + explorer + "#/tx/" + encodeURIComponent(String(c.txid || "")) + "\">" + e(T("Block #{h}", { h: GKI18N.num(Number(c.height) || 0) })) + " ↗</a>"
           : "<span class=\"hs-proof wait\">" + e(T("Reported")) + "</span>") +
        "</li>";
    }).join("") : "<li class=\"hs-empty muted\">" + e(T("No completed swaps yet.")) + "</li>";
  };
  if (document.querySelector("[data-mk-usd], [data-main], [data-mk-price]")) {
    var loadMarket = function () {
      var info = gk.api("/v1/info").catch(function () { return null; });
      var mk = gk.api("/v1/swap/market").then(function (m) { gk.mk = m.market; gk.mkRecent = m.recent || []; }).catch(function () {});
      var btc = gk.api("/v1/btc-price").then(function (p) { gk.btcUsd = p.USD; }).catch(function () {});
      Promise.all([info, mk, btc]).then(function (r) { gk.value(r[0]); gk.priceChart(); gk.swapFeed(); });
    };
    loadMarket();
    setInterval(function () { if (!document.hidden) loadMarket(); }, 60000);
  }

// Mining calculator (genoryn.io/mining): live block height and miners from the node.
  var calcEl = document.getElementById("mcCalc");
  if (calcEl && window.GKValue) {
    var liveInfo = {}, liveMk = {}, liveBtc = 0;
    var calc = GKValue.mountCalc(calcEl, function () {
      return { height: liveInfo.height, hashrate: liveInfo.hashrate,
        supply: liveInfo.supply, sat: liveMk.price, btcUsd: liveBtc };
    });
    var loadCalc = function () {
      Promise.all([
        gk.api("/v1/info").then(function (i) { liveInfo = i; }).catch(function () {}),
        gk.api("/v1/swap/market").then(function (m) { liveMk = m.market || {}; }).catch(function () {}),
        gk.api("/v1/btc-price").then(function (p) { liveBtc = Number(p.USD) || 0; }).catch(function () {})
      ]).then(function () {
        calc.update();
        var px = GKValue.market(liveMk.price, liveBtc);
        document.querySelectorAll("[data-usd-gnr]").forEach(function (el) {
          el.textContent = px != null ? "≈ " + GKValue.usd(Number(el.getAttribute("data-usd-gnr")) * px) : T("no market price yet");
        });
      });
    };
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
        else homeChart = GKCharts.chart(document.getElementById("homeChart"), series, { kind: "area", fmt: GKCharts.fmtRate, label: T("Network hashrate, last 24 hours"), height: 170 });
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
  // Home: charts with a time range, the supply disc and the latest blocks.
  var homeDash = document.getElementById("homeDash");
  if (homeDash && window.GKCharts) {
    var hk = function (k, v) { homeDash.querySelectorAll('[data-kpi="' + k + '"]').forEach(function (e) { e.textContent = v; }); };
    var hd = GKCharts.dashboard(homeDash, function (r) { return gk.api("/v1/stats?range=" + r); }, function (s, pts) {
      var last = pts[pts.length - 1] || {};
      var avg = function (k) { return pts.length ? pts.reduce(function (a, p) { return a + Number(p[k] || 0); }, 0) / pts.length : 0; };
      hk("hashrate", gk.rate(s.hashrate));
      hk("shares", T("{n} avg", { n: GKI18N.num(avg("shares"), { minimumFractionDigits: 1, maximumFractionDigits: 1 }) }));
      hk("block_time", T("{n} s avg", { n: avg("block_time").toFixed(0) }));
      hk("difficulty", GKCharts.fmtNum(last.difficulty || 0));
      hk("issued", gk.gnr(s.issued, 0) + " GNR");
      hk("remaining", gk.gnr(s.remaining, 0) + " GNR");
      GKCharts.disc(document.getElementById("homeDisc"), Number(s.issued || 0), Number(s.max_supply || 21e14));
    });
    setInterval(function () { if (!document.hidden) hd.reload(); }, 120000);
  }
  var homeBlocks = document.getElementById("homeBlocks");
  if (homeBlocks) {
    var loadBlocks = function () {
      gk.api("/v1/blocks?count=6").then(function (bs) {
        homeBlocks.innerHTML = bs.map(function (b) {
          var h = Number(b.height), tx = Number(b.txs) || 0, sh = Number(b.shares) || 0;
          return '<a href="' + GKI18N.path("/explorer") + "#/block/" + h + '"><b>#' + GKI18N.num(h) + "</b><span>" + gk.ago(b.timestamp) + "</span><span>" + Tn(sh, "{n} share", "{n} shares") +
            "</span><span>" + T("{n} tx", { n: tx }) + "</span><em>" + gk.gnr((Number(b.subsidy) || 0) + (Number(b.fees) || 0), 2) + " GNR</em></a>";
        }).join("");
      }).catch(function () {});
    };
    loadBlocks();
    setInterval(function () { if (!document.hidden) loadBlocks(); }, 30000);
  }
  if (window.GKCharts) GKCharts.tilt(".tilt-card");

  var h = document.getElementById("sHeight");
  if (h) {
    var load = function () {
      gk.api("/v1/info").then(function (i) {
        h.textContent = GKI18N.num(i.height);
        document.getElementById("sSupply").textContent = gk.gnr(i.supply, 0);
        document.getElementById("sRate").textContent = gk.rate(i.hashrate);
        gk.difficulty(i);
        var lt = document.getElementById("liveText");
        if (lt) lt.textContent = (i.network === "mainnet" ? T("Mainnet is live · block {h}", { h: GKI18N.num(i.height) }) : T("Testnet is live · block {h}", { h: GKI18N.num(i.height) }));
      }).catch(function () {
        var d = document.getElementById("liveDot");
        if (d) { d.classList.remove("ok", "live"); d.classList.add("warn"); }
        var lt = document.getElementById("liveText");
        if (lt) lt.textContent = T("Network status unavailable");
      });
    };
    load();
    setInterval(function () { if (!document.hidden) load(); }, 30000);
  }
})();
