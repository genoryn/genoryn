/* Ambient background, theme toggle and scroll reveal. ~2 KB, no dependencies.
   fx-head.html sets data-theme and data-fx before first paint; this file runs after load. */
(function () {
  var root = document.documentElement;
  var base = (document.currentScript && document.currentScript.getAttribute("data-base")) || "";
  var ver = (document.currentScript && document.currentScript.getAttribute("data-v")) || "";
  var reduce = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  var low = root.getAttribute("data-fx") === "low";

  // ---- Ambient background: two layers, opacity cross-fade, decoded before showing ----
  var layer = document.createElement("div");
  layer.className = "fx-bg";
  layer.setAttribute("aria-hidden", "true");
  var a = new Image(), b = new Image();
  a.alt = b.alt = "";
  a.decoding = b.decoding = "async";
  var grain = document.createElement("div");
  grain.className = "grain";
  layer.appendChild(a); layer.appendChild(b); layer.appendChild(grain);
  document.body.prepend(layer);
  var idx = new Date().getHours() % 4; // a different opening scene each hour
  var front = a, back = b;
  function theme() { return root.getAttribute("data-theme") === "light" ? "light" : "dark"; }
  function src(i) { return base + "bg/" + theme() + "-" + (i % 4 + 1) + ".webp" + (ver ? "?v=" + ver : ""); }
  function fadeTo(i) {
    back.src = src(i);
    var p = back.decode ? back.decode() : Promise.resolve();
    return p.then(function () {
      back.classList.add("on");
      front.classList.remove("on");
      var t = front; front = back; back = t;
    }).catch(function () {});
  }
  // Fade in after first paint so the background never blocks LCP.
  window.requestAnimationFrame(function () { setTimeout(function () { fadeTo(idx); }, 120); });
  setInterval(function () {
    if (document.hidden || reduce || low || window.fxBusy) return;
    idx++;
    fadeTo(idx);
  }, 45000);

  // ---- Theme toggle: cross-fade to the other theme's image set ----
  window.fxToggleTheme = function () {
    var t = theme() === "light" ? "dark" : "light";
    root.setAttribute("data-theme", t);
    try { localStorage.setItem("gk-theme", t); } catch (e) {}
    fadeTo(idx);
  };

  // ---- Scroll reveal (once) ----
  var els = document.querySelectorAll(".reveal");
  if (reduce || !("IntersectionObserver" in window)) {
    els.forEach(function (e) { e.classList.add("in"); });
  } else {
    var io = new IntersectionObserver(function (entries) {
      entries.forEach(function (en, k) {
        if (en.isIntersecting) {
          setTimeout(function () { en.target.classList.add("in"); }, Math.min(k, 6) * 70);
          io.unobserve(en.target);
        }
      });
    }, { rootMargin: "0px 0px -8% 0px" });
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
})();
