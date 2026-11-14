/* genoryn.io/coins: every Krypton coin on chain, read from the node's /v1/assets, with the
   project's signed verified list (the wallet checks its signature; this page labels). */
(function () {
  "use strict";
  var el = function (id) { return document.getElementById(id); };
  var esc = function (s) { return String(s == null ? "" : s).replace(/[&<>"]/g, function (c) { return { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c]; }); };
  var state = { coins: [], verified: {}, markets: {}, filter: "all", active: false, height: null };
  var units = function (n, d) {
    n = Number(n || 0); var s = (n / Math.pow(10, d || 0)).toLocaleString(undefined, { maximumFractionDigits: d || 0 });
    return s;
  };
  var cls = function (r) {
    if (/^Fixed supply|^Holders can burn/.test(r)) return "good";
    if (/any amount|stop all transfers|Not verified|impersonation|Not backed|depends on|Custodial/.test(r)) return "alert";
    return "";
  };
  var mark = function (k) { return k === "good" ? "✓" : k === "alert" ? "!" : "·"; };
  function risks(c, v) {
    return (c.risks || []).filter(function (r) { return !(v && /Not verified|impersonation/.test(r)); });
  }
  function avatar(c, v) {
    if (v && /^coins\/[a-z0-9-]+\.svg$/.test(v.logo || "")) return '<span class="cn-av"><img src="/assets/' + v.logo + '" alt="" width="44" height="44"></span>';
    return '<span class="cn-av">' + esc(String(c.ticker || "?").slice(0, 3)) + "</span>";
  }
  function render() {
    var q = (el("cnQ").value || "").trim().toLowerCase();
    var list = state.coins.filter(function (c) {
      var v = state.verified[c.id];
      if (state.filter === "verified" && !v) return false;
      return !q || String(c.ticker).toLowerCase().indexOf(q) >= 0 || String(c.name).toLowerCase().indexOf(q) >= 0;
    }).sort(function (a, b) {
      var va = state.verified[a.id] ? 1 : 0, vb = state.verified[b.id] ? 1 : 0;
      return vb - va || Number(b.outstanding) - Number(a.outstanding);
    });
    el("cnState").textContent = state.active ? state.coins.length + " coin" + (state.coins.length === 1 ? "" : "s") + " on chain"
      : (state.height ? "Krypton coins activate at block " + Number(state.height).toLocaleString() : "Krypton coins activate with an announced release");
    if (!list.length) {
      el("cnList").innerHTML = '<p class="muted">' + (state.coins.length ? "No coin matches." : (state.active ? "No coins yet: be the first to issue one." : "No coins yet: they can be deployed from the activation block on.")) + "</p>";
      return;
    }
    el("cnList").innerHTML = list.map(function (c) {
      var v = state.verified[c.id], m = state.markets[c.id], d = Number(c.decimals || 0);
      var price = m ? (m.price * Math.pow(10, d) / 1e8).toLocaleString(undefined, { maximumSignificantDigits: 6 }) + " GNR" : null;
      return '<article class="cn-item">' + avatar(c, v) +
        '<div><div class="cn-name"><b>' + esc(c.ticker) + "</b><span>" + esc(c.name) + "</span>" +
        (v ? '<span class="cn-vf">✓ Verified' + (v.issuer ? " · " + esc(v.issuer) : "") + "</span>" : '<span class="cn-uv">Unverified</span>') +
        (c.paused ? '<span class="cn-uv">Paused</span>' : "") + "</div>" +
        '<span class="cn-id mono">' + esc(String(c.id).slice(0, 18)) + "… · since block " + Number(c.height || 0).toLocaleString() + "</span></div>" +
        '<div class="cn-amt"><b>' + units(c.outstanding, d) + '</b><span>in circulation' + (Number(c.burned) ? " · " + units(c.burned, d) + " burned" : "") + "</span>" +
        (price ? '<span class="cn-px">' + price + " · average of " + m.swaps + " swap" + (m.swaps === 1 ? "" : "s") + "</span>" : "") + "</div>" +
        '<div class="cn-risks">' + risks(c, v).map(function (r) { var k = cls(r); return '<span class="cn-risk ' + k + '"><i>' + mark(k) + "</i>" + esc(r) + "</span>"; }).join("") + "</div></article>";
    }).join("");
  }
  function load() {
    var a = gk.api("/v1/assets").catch(function () { return { assets: [] }; });
    var v = fetch("/verified-assets.json", { headers: { accept: "application/json" } }).then(function (r) { return r.ok ? r.json() : { coins: [] }; }).catch(function () { return { coins: [] }; });
    var m = gk.api("/v1/swap/coins").catch(function () { return { markets: {} }; });
    Promise.all([a, v, m]).then(function (r) {
      state.coins = r[0].assets || []; state.active = r[0].active === true; state.height = r[0].assets_height;
      if (el("cnsCount")) el("cnsCount").textContent = state.coins.length.toLocaleString();
      gk.api("/v1/info").then(function (i) {
        if (!el("cnsAct")) return;
        if (state.active) { el("cnsAct").textContent = "Active"; el("cnsActL").textContent = "Since block " + Number(state.height || 0).toLocaleString(); return; }
        var left = Number(state.height || 0) - Number(i.height || 0);
        el("cnsAct").textContent = left > 0 ? left.toLocaleString() + " blocks" : "Now";
        el("cnsActL").textContent = "Until coins activate at block " + Number(state.height || 0).toLocaleString() + (left > 0 ? " (about " + Math.max(1, Math.round(left / 60)) + " h)" : "");
      }).catch(function () {});
      state.verified = {};
      (r[1].coins || []).forEach(function (c) { if (c.network === "mainnet") state.verified[c.id] = c; });
      state.markets = r[2].markets || {};
      render();
      document.querySelectorAll("[data-cn-supply]").forEach(function (td) {
        var c = state.coins.filter(function (x) { return x.id === td.getAttribute("data-cn-supply"); })[0];
        if (c) td.textContent = units(c.outstanding, Number(c.decimals || 0)) + " " + c.ticker;
      });
    });
  }
  document.querySelectorAll("#cnF button").forEach(function (b) {
    b.onclick = function () {
      document.querySelectorAll("#cnF button").forEach(function (x) { x.classList.toggle("on", x === b); });
      state.filter = b.dataset.f; render();
    };
  });
  el("cnQ").addEventListener("input", render);
  load();
  setInterval(load, 60000);
})();
