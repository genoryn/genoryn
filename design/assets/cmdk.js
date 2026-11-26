/* Search and commands: Ctrl+K (or Cmd+K) opens a panel that finds a page or an action by a few
   letters. Shared by the website, the wallet and Genoryn Talks; ~3 KB, no dependencies.

   The page describes itself with GKCmd.setup({...}):
     groups()   -> [{ title, items: [{ label, hint, icon, href, run, words }] }]
     query(q)   -> extra items for what was typed (e.g. "block 1200"), listed first
     enabled()  -> false while the panel must not open (e.g. a locked wallet)
     text       -> { placeholder, empty, open, move, close, title }
   An item either goes to href or calls run(); nothing else happens here. The panel never
   sends what was typed anywhere: matching is done in the page. */
(function () {
  var cfg = null, box = null, input = null, list = null, shown = [], at = 0, opener = null;
  var SEARCH = '<svg viewBox="0 0 24 24" aria-hidden="true"><circle cx="11" cy="11" r="6.5"/><path d="m16 16 4.5 4.5"/></svg>';

  function esc(s) { return String(s == null ? "" : s).replace(/[&<>"']/g, function (c) { return { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]; }); }
  function norm(s) { return String(s || "").toLowerCase().normalize("NFKD").replace(/[\u0300-\u036f]/g, ""); }
  function txt(k, d) { return (cfg && cfg.text && cfg.text[k]) || d; }

  function build() {
    box = document.createElement("div");
    box.className = "cmdk";
    box.hidden = true;
    box.setAttribute("role", "dialog");
    box.setAttribute("aria-modal", "true");
    box.setAttribute("aria-label", txt("title", "Search"));
    box.innerHTML = '<div class="cmdk-veil" data-cmdk="close"></div><div class="cmdk-panel">' +
      '<div class="cmdk-in">' + SEARCH + '<input type="text" role="combobox" aria-expanded="true" aria-controls="cmdkList" aria-autocomplete="list" autocomplete="off" autocapitalize="off" spellcheck="false" maxlength="140"><span class="kbd">Esc</span></div>' +
      '<div class="cmdk-list" id="cmdkList" role="listbox"></div>' +
      '<div class="cmdk-foot"><span><span class="kbd">↑</span><span class="kbd">↓</span><span data-t="move"></span></span><span><span class="kbd">↵</span><span data-t="open"></span></span><span><span class="kbd">Esc</span><span data-t="close"></span></span></div></div>';
    document.body.appendChild(box);
    input = box.querySelector("input");
    list = box.querySelector(".cmdk-list");
    input.placeholder = txt("placeholder", "Search");
    input.setAttribute("aria-label", txt("placeholder", "Search"));
    box.querySelector('[data-t="move"]').textContent = txt("move", "move");
    box.querySelector('[data-t="open"]').textContent = txt("open", "open");
    box.querySelector('[data-t="close"]').textContent = txt("close", "close");
    input.addEventListener("input", function () { at = 0; render(); });
    box.addEventListener("click", function (e) {
      if (e.target.getAttribute && e.target.getAttribute("data-cmdk") === "close") { close(); return; }
      var it = e.target.closest ? e.target.closest(".cmdk-it") : null;
      if (it) { e.preventDefault(); go(Number(it.getAttribute("data-i"))); }
    });
    list.addEventListener("pointermove", function (e) {
      var it = e.target.closest ? e.target.closest(".cmdk-it") : null;
      if (it && Number(it.getAttribute("data-i")) !== at) { at = Number(it.getAttribute("data-i")); mark(); }
    });
    box.addEventListener("keydown", function (e) {
      if (e.key === "ArrowDown" || (e.key === "Tab" && !e.shiftKey)) { e.preventDefault(); at = shown.length ? (at + 1) % shown.length : 0; mark(true); }
      else if (e.key === "ArrowUp" || (e.key === "Tab" && e.shiftKey)) { e.preventDefault(); at = shown.length ? (at - 1 + shown.length) % shown.length : 0; mark(true); }
      else if (e.key === "Enter") { e.preventDefault(); go(at); }
      else if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); close(); }
    });
  }

  // Every word typed must be found in the item; a match at the start of the name ranks first.
  function score(it, words) {
    var name = norm(it.label), all = name + " " + norm(it.hint) + " " + norm(it.words), s = 0;
    for (var i = 0; i < words.length; i++) {
      var w = words[i], p = all.indexOf(w);
      if (p < 0) return -1;
      var n = name.indexOf(w);
      s += n === 0 ? 0 : n > 0 ? (name.charAt(n - 1) === " " ? 1 : 3) : 6;
    }
    return s;
  }

  function render() {
    var q = input.value.trim(), words = norm(q).split(/\s+/).filter(Boolean);
    var groups = cfg.groups ? cfg.groups() : [];
    var html = "";
    shown = [];
    var add = function (title, items) {
      if (!items.length) return;
      html += '<div class="cmdk-h" role="presentation">' + esc(title) + "</div>";
      items.forEach(function (it) {
        var i = shown.length;
        shown.push(it);
        var icon = it.icon ? '<span class="cmdk-ic"><svg viewBox="0 0 24 24" aria-hidden="true"><use href="' + esc(it.icon) + '"/></svg></span>' : '<span class="cmdk-ic">' + SEARCH + "</span>";
        html += '<div class="cmdk-it" role="option" id="cmdk-' + i + '" data-i="' + i + '" aria-selected="false">' + icon +
          '<span class="cmdk-t"><b>' + esc(it.label) + "</b>" + (it.hint ? "<small>" + esc(it.hint) + "</small>" : "") + "</span>" +
          (it.tag ? "<em>" + esc(it.tag) + "</em>" : "") + "</div>";
      });
    };
    if (q && cfg.query) {
      var extra = cfg.query(q) || [];
      if (extra.length) add(extra[0].group || txt("title", "Search"), extra);
    }
    groups.forEach(function (g) {
      var items = g.items || [];
      if (words.length) {
        items = items.map(function (it) { return { it: it, s: score(it, words) }; })
          .filter(function (x) { return x.s >= 0; })
          .sort(function (a, b) { return a.s - b.s; })
          .map(function (x) { return x.it; });
      }
      add(g.title, items.slice(0, 40));
    });
    list.innerHTML = html || '<div class="cmdk-none">' + esc(txt("empty", "Nothing found")) + "</div>";
    if (at >= shown.length) at = 0;
    mark();
    list.scrollTop = 0;
  }

  function mark(scroll) {
    var items = list.querySelectorAll(".cmdk-it");
    for (var i = 0; i < items.length; i++) items[i].setAttribute("aria-selected", String(i === at));
    if (items[at]) {
      input.setAttribute("aria-activedescendant", items[at].id);
      if (scroll) items[at].scrollIntoView({ block: "nearest" });
    } else input.removeAttribute("aria-activedescendant");
  }

  function go(i) {
    var it = shown[i];
    if (!it) return;
    close(true);
    if (it.run) it.run();
    else if (it.href) window.location.assign(it.href);
  }

  function open() {
    if (!cfg || (cfg.enabled && !cfg.enabled())) return;
    if (!box) build();
    if (!box.hidden) return;
    opener = document.activeElement;
    input.value = "";
    at = 0;
    box.hidden = false;
    render();
    input.focus();
  }
  function close(keep) {
    if (!box || box.hidden) return;
    box.hidden = true;
    if (!keep && opener && opener.focus) try { opener.focus(); } catch (e) {}
    opener = null;
  }

  document.addEventListener("keydown", function (e) {
    if (!cfg) return;
    var k = (e.key || "").toLowerCase();
    if (k === "k" && (e.ctrlKey || e.metaKey) && !e.altKey && !e.shiftKey) {
      e.preventDefault();
      if (box && !box.hidden) close(); else open();
      return;
    }
    // "/" opens it too, unless something is being typed.
    if (k === "/" && cfg.slash && !e.ctrlKey && !e.metaKey && !e.altKey) {
      var a = document.activeElement, tag = a && a.tagName;
      if (tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT" || (a && a.isContentEditable)) return;
      e.preventDefault();
      open();
    }
  });
  document.addEventListener("click", function (e) {
    var b = e.target.closest ? e.target.closest("[data-cmdk-open]") : null;
    if (b) { e.preventDefault(); open(); }
  });

  window.GKCmd = {
    setup: function (c) { cfg = c; if (box) { box.remove(); box = null; } },
    open: open,
    close: function () { close(); },
    // The key to show next to the search button: the Mac has its own.
    key: /Mac|iPhone|iPad/.test(navigator.platform || "") ? "⌘K" : "Ctrl K"
  };
})();
