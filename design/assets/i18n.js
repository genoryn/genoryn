/* Translations for the website and the wallet. English is the source text and the fallback:
     T("Looking up…")                      a sentence
     T("Block {h} found", { h: 12 })       with values
     Tn(n, "{n} share", "{n} shares")      with a plural (the language's own plural forms)
     TE(...) / TH(...)                     the same, escaped for HTML (TH: **bold** allowed)
     TR(message)                           a message of the wallet's Rust code, by template
   A catalog maps the English text to a translation; a plural entry maps "one|other" to the
   forms of Intl.PluralRules (one, few, many, other...). Numbers follow the page's language.
   The wallet embeds its catalog in the page (#gk-i18n-data); the site loads /assets/i18n/<lang>.js. */
(function () {
  var lang = document.documentElement.getAttribute("lang") || "en";
  var cat = {}, rules = null, dom = {}, rust = [];
  function setRules() { try { rules = new Intl.PluralRules(lang); } catch (e) { rules = null; } }
  function fill(s, v) {
    return v ? String(s).replace(/\{(\w+)\}/g, function (m, k) { return v[k] != null ? v[k] : m; }) : String(s);
  }
  setRules();
  window.GKI18N = {
    get lang() { return lang; },
    add: function (o) { for (var k in o) if (Object.prototype.hasOwnProperty.call(o, k)) cat[k] = o[k]; },
    num: function (v, opts) { var n = Number(v); try { return n.toLocaleString(lang, opts); } catch (e) { return n.toLocaleString("en-US", opts); } },
    /* A path of this site in the page's language: /mining -> /de/mining. */
    path: function (p) {
      var pre = document.documentElement.getAttribute("data-path-prefix") || "";
      return pre && /^\/(?!assets\/|v1\/|talks)/.test(p) ? pre + (p === "/" ? "/" : p) : p;
    }
  };
  window.T = function (s, v) { var x = cat[s]; return fill(typeof x === "string" && x ? x : s, v); };
  var esc = function (x) { return String(x).replace(/[&<>"]/g, function (c) { return { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c]; }); };
  /* T() escaped for HTML. */
  window.TE = function (s, v) { return esc(window.T(s, v)); };
  /* T() for HTML with emphasis: escaped, then **bold** becomes <b>bold</b>. */
  window.TH = function (s, v) { return esc(window.T(s, v)).replace(/\*\*([^*]+)\*\*/g, "<b>$1</b>"); };
  window.Tn = function (n, one, other, v) {
    v = v || {};
    if (v.n == null) v.n = GKI18N.num(n);
    var x = cat[one + "|" + other];
    if (x && typeof x === "object") {
      var form = rules ? rules.select(Number(n)) : "other";
      var s = x[form] || x.other;
      if (s) return fill(s, v);
    }
    return fill(Number(n) === 1 ? one : other, v);
  };

  /* Messages of the Rust code: whole, by template ("share at height {1}"), or part by part
     ("board: offer expired"). Values inside them stay as they are; unknown text stays English. */
  var tpl = null;
  function templates() {
    if (tpl) return tpl;
    tpl = rust.map(function (p) {
      var parts = p[0].split(/\{\d+\}/), names = p[0].match(/\{\d+\}/g) || [];
      var re = "^" + parts.map(function (x) { return x.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"); }).join("([\\s\\S]+?)") + "$";
      return { re: new RegExp(re), names: names, to: p[1], plain: !names.length, en: p[0] };
    });
    return tpl;
  }
  function tr(m, depth) {
    if (!rust.length || typeof m !== "string" || !m) return m;
    var all = templates();
    for (var i = 0; i < all.length; i++) {
      var t = all[i];
      if (t.plain ? t.en === m : t.re.test(m)) {
        if (t.plain) return t.to;
        var g = m.match(t.re), out = t.to;
        t.names.forEach(function (name, j) { out = out.split(name).join(depth < 2 ? tr(g[j + 1], depth + 1) : g[j + 1]); });
        return out;
      }
    }
    var c = m.indexOf(": ");
    if (c > 0 && depth < 3) return tr(m.slice(0, c), depth + 1) + ": " + tr(m.slice(c + 2), depth + 1);
    return m;
  }
  window.TR = function (m) { return tr(m, 0); };

  /* The page's own text (the wallet): each block with its inline markup as numbered
     placeholders, cut exactly as website/i18n.py cuts pages in DOM mode, so both find the same
     English texts. Elements are moved, never recreated: listeners and references stay. */
  var INLINE = /^(A|ABBR|B|BDI|BR|CITE|CODE|DATA|DFN|EM|I|IMG|KBD|MARK|Q|S|SAMP|SMALL|SPAN|STRONG|SUB|SUP|TIME|U|VAR|WBR)$/;
  var RAW = /^(SVG|SCRIPT|STYLE|PRE|TEXTAREA|TEMPLATE)$/;
  var OPAQUE = /^(BR|CODE|IMG|KBD|SAMP|VAR|WBR)$/;
  var LETTERS = /[\p{L}\p{Nl}\p{No}]{2,}/u;
  var ATTRS = ["title", "alt", "placeholder", "aria-label", "data-label"];
  function tag(n) { return n.nodeType === 1 ? n.tagName.toUpperCase() : ""; }
  function raw(n) { return n.nodeType === 1 && RAW.test(tag(n)); }
  function noTr(n) { return n.nodeType === 1 && (n.getAttribute("translate") === "no" || /\b(mono|num|notranslate)\b/.test(n.getAttribute("class") || "")); }
  function textOf(n) {
    if (n.nodeType === 3) return n.nodeValue;
    if (n.nodeType !== 1 || raw(n)) return "";
    var s = "";
    for (var c = n.firstChild; c; c = c.nextSibling) s += textOf(c);
    return s;
  }
  function opaque(n) { return raw(n) || OPAQUE.test(tag(n)) || noTr(n) || !LETTERS.test(textOf(n)); }
  function inline(n) { return n.nodeType === 3 || raw(n) || INLINE.test(tag(n)); }
  function kids(n) { var a = []; for (var c = n.firstChild; c; c = c.nextSibling) if (c.nodeType === 1 || c.nodeType === 3) a.push(c); return a; }
  function blank(n) { return n.nodeType === 3 && !/\S/.test(n.nodeValue); }
  var escText = function (s) { return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;"); };
  var unescText = function (s) { return s.replace(/&lt;/g, "<").replace(/&gt;/g, ">").replace(/&amp;/g, "&"); };
  function segment(nodes) {
    var parts = [], k = 0;
    function walk(ns) {
      return ns.map(function (n) {
        if (n.nodeType === 3) return escText(n.nodeValue);
        var i = k++;
        parts[i] = n;
        return opaque(n) ? "<" + i + "/>" : "<" + i + ">" + walk(kids(n)) + "</" + i + ">";
      }).join("");
    }
    var src = walk(nodes).replace(/\s+/g, " ").trim();
    return { src: src, parts: parts, nodes: nodes };
  }
  /* Puts the translation in place of the segment's nodes; null if it does not fit. */
  function apply(seg, t) {
    var toks = t.split(/(<\d+>|<\/\d+>|<\d+\/>)/), frag = document.createDocumentFragment(), stack = [frag], used = {};
    for (var i = 0; i < toks.length; i++) {
      var s = toks[i], m;
      if (!s) continue;
      if ((m = /^<(\d+)\/>$/.exec(s))) { var o = seg.parts[m[1]]; if (!o || used[m[1]]) return null; used[m[1]] = 1; stack[stack.length - 1].appendChild(o); }
      else if ((m = /^<(\d+)>$/.exec(s))) { var p = seg.parts[m[1]]; if (!p || used[m[1]]) return null; used[m[1]] = 1; while (p.firstChild) p.removeChild(p.firstChild); stack[stack.length - 1].appendChild(p); stack.push(p); }
      else if ((m = /^<\/(\d+)>$/.exec(s))) { if (stack[stack.length - 1] !== seg.parts[m[1]]) return null; stack.pop(); }
      else stack[stack.length - 1].appendChild(document.createTextNode(unescText(s)));
    }
    return stack.length === 1 ? frag : null;
  }
  function translateRun(ns) {
    while (ns.length && (blank(ns[0]) || ns[0].nodeType === 1 && opaque(ns[0]))) ns.shift();
    while (ns.length && (blank(ns[ns.length - 1]) || ns[ns.length - 1].nodeType === 1 && opaque(ns[ns.length - 1]))) ns.pop();
    if (!ns.length || !LETTERS.test(ns.filter(function (n) { return n.nodeType === 3 || !opaque(n); }).map(textOf).join(""))) return;
    var bare = ns.some(function (n) { return n.nodeType === 3 && /\S/.test(n.nodeValue); });
    if (!bare) { ns.forEach(function (n) { if (n.nodeType === 1 && !opaque(n)) translateBlock(n); }); return; }
    var seg = segment(ns), t = dom[seg.src];
    if (!t) return;
    // Nodes of the segment with their whitespace, before the translation is built from them.
    var first = ns[0], parent = first.parentNode, mark = document.createTextNode("");
    parent.insertBefore(mark, first);
    var keep = [];
    for (var n = first; n; n = n.nextSibling) { keep.push(n); if (n === ns[ns.length - 1]) break; }
    keep.forEach(function (x) { parent.removeChild(x); });
    var frag = apply(seg, t);
    if (!frag) { keep.forEach(function (x) { parent.insertBefore(x, mark); }); parent.removeChild(mark); return; }
    parent.insertBefore(frag, mark);
    parent.removeChild(mark);
  }
  function translateBlock(el) {
    if (el.nodeType !== 1 || raw(el) || noTr(el)) return;
    var run = [];
    kids(el).forEach(function (c) {
      if (inline(c)) run.push(c);
      else { translateRun(run); run = []; translateBlock(c); }
    });
    translateRun(run);
  }
  GKI18N.translatePage = function (root) {
    if (lang === "en" || !root) return;
    for (var a = 0; a < ATTRS.length; a++) {
      root.querySelectorAll("[" + ATTRS[a] + "]").forEach(function (el) {
        if (el.closest("[translate=no]")) return;
        var v = el.getAttribute(ATTRS[a]).replace(/\s+/g, " ").trim(), t = dom[v];
        if (t) el.setAttribute(ATTRS[a], t);
      });
    }
    translateBlock(root);
  };

  var data = document.getElementById("gk-i18n-data");
  if (data) {
    try {
      var d = JSON.parse(data.textContent);
      dom = d.dom || {}; rust = d.rust || [];
      GKI18N.add(d.js || {});
    } catch (e) {}
  }
})();
