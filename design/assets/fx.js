/* Page background, theme toggle, scroll reveal and the light that follows the cursor over
   cards. ~2 KB, no dependencies. fx-head.html sets data-theme and data-fx before first paint;
   this file runs after load. Nothing is fetched: the background is drawn by fx.css. */
(function () {
  var root = document.documentElement;
  var reduce = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  var low = root.getAttribute("data-fx") === "low";

  // ---- Background layer: the mesh and the grid are CSS, the grain is one small tile ----
  var layer = document.createElement("div");
  layer.className = "fx-bg";
  layer.setAttribute("aria-hidden", "true");
  var grain = document.createElement("div");
  grain.className = "grain";
  layer.appendChild(grain);
  document.body.prepend(layer);

  // ---- Theme toggle ----
  function theme() { return root.getAttribute("data-theme") === "light" ? "light" : "dark"; }
  window.fxToggleTheme = function () {
    var t = theme() === "light" ? "dark" : "light";
    root.setAttribute("data-theme", t);
    try { localStorage.setItem("gk-theme", t); } catch (e) {}
    try { window.dispatchEvent(new CustomEvent("fx-theme", { detail: t })); } catch (e) {}
  };

  // ---- Scroll reveal (once) ----
  var els = document.querySelectorAll(".reveal");
  if (reduce || !("IntersectionObserver" in window)) {
    els.forEach(function (e) { e.classList.add("in"); });
  } else {
    var io = new IntersectionObserver(function (entries) {
      entries.forEach(function (en, k) {
        if (en.isIntersecting) {
          setTimeout(function () { en.target.classList.add("in"); }, Math.min(k, 6) * 60);
          io.unobserve(en.target);
        }
      });
    }, { rootMargin: "0px 0px -6% 0px" });
    els.forEach(function (e) { io.observe(e); });
  }

  // Pause drifting decorations while they are off-screen.
  var drifters = document.querySelectorAll(".drift");
  if (drifters.length && "IntersectionObserver" in window) {
    var io2 = new IntersectionObserver(function (entries) {
      entries.forEach(function (en) { en.target.style.animationPlayState = en.isIntersecting ? "running" : "paused"; });
    });
    drifters.forEach(function (d) { io2.observe(d); });
  }

  // ---- A soft light follows the cursor across a card and along its edge ----
  // One listener for the whole page; only the card under the pointer is touched, once a frame.
  if (!reduce && !low && window.matchMedia("(hover: hover) and (pointer: fine)").matches) {
    var lit = null, pending = null, queued = false;
    var paint = function () {
      queued = false;
      var e = pending;
      if (!e) return;
      var t = e.target, card = t && t.closest ? t.closest(".glass") : null;
      if (lit && lit !== card) { lit.style.removeProperty("--mx"); lit.style.removeProperty("--my"); }
      lit = card;
      if (!card) return;
      var r = card.getBoundingClientRect();
      card.style.setProperty("--mx", (e.clientX - r.left).toFixed(0) + "px");
      card.style.setProperty("--my", (e.clientY - r.top).toFixed(0) + "px");
    };
    document.addEventListener("pointermove", function (e) {
      pending = e;
      if (!queued) { queued = true; window.requestAnimationFrame(paint); }
    }, { passive: true });
    document.addEventListener("pointerleave", function () {
      if (lit) { lit.style.removeProperty("--mx"); lit.style.removeProperty("--my"); lit = null; }
    });
  }
})();
