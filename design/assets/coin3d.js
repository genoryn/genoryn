/* The Genoryn coin in real 3D (WebGL): a minted disc with an embossed mark on both faces and
   a reeded edge, lit like metal. Replaces the CSS coin wherever WebGL is available. */
(function () {
  var RING = "M128 9a119 119 0 1 0 0.01 0";
  var ARC = "M186 128A58 58 0 1 1 171.1 89.19";
  var BARS = "M134 113h67v30h-67zM106 31h14v54h-14zM106 171h14v54h-14zM136 31h14v54h-14zM136 171h14v54h-14z";

  /* Height map of one face: the ring and the mark raised, softened into a bevel. */
  function relief(size) {
    var a = document.createElement("canvas");
    a.width = a.height = size;
    var c = a.getContext("2d"), k = size / 256;
    c.fillStyle = "#000";
    c.fillRect(0, 0, size, size);
    c.scale(k, k);
    c.strokeStyle = c.fillStyle = "#fff";
    c.lineWidth = 11;
    c.stroke(new Path2D(RING));
    c.translate(128, 128); c.scale(0.74, 0.74); c.translate(-128, -128);
    c.lineWidth = 30;
    c.stroke(new Path2D(ARC));
    c.fill(new Path2D(BARS));
    var b = document.createElement("canvas");
    b.width = b.height = size;
    var d = b.getContext("2d");
    d.filter = "blur(" + Math.round(size / 160) + "px)";
    d.drawImage(a, 0, 0);
    return b;
  }

  function geometry(seg, h) {
    var v = [];
    // x y z | nx ny nz | u v | edge angle
    function push(x, y, z, nx, ny, nz, u, w, e, ang) { v.push(x, y, z, nx, ny, nz, u, w, e, ang); }
    for (var s = -1; s <= 1; s += 2) {
      for (var i = 0; i < seg; i++) {
        var a0 = i / seg * Math.PI * 2, a1 = (i + 1) / seg * Math.PI * 2;
        var p = [[0, 0], [Math.cos(a0), Math.sin(a0)], [Math.cos(a1), Math.sin(a1)]];
        if (s < 0) p = [p[0], p[2], p[1]];
        p.forEach(function (q) {
          // The back face reads the right way round when the coin has turned.
          push(q[0], q[1], s * h, 0, 0, s, 0.5 + 0.5 * q[0] * s, 0.5 - 0.5 * q[1], 0, 0);
        });
      }
    }
    for (var j = 0; j < seg; j++) {
      var b0 = j / seg * Math.PI * 2, b1 = (j + 1) / seg * Math.PI * 2;
      var c0 = Math.cos(b0), s0 = Math.sin(b0), c1 = Math.cos(b1), s1 = Math.sin(b1);
      push(c0, s0, h, c0, s0, 0, 0, 0, 1, b0); push(c0, s0, -h, c0, s0, 0, 0, 0, 1, b0); push(c1, s1, h, c1, s1, 0, 0, 0, 1, b1);
      push(c1, s1, h, c1, s1, 0, 0, 0, 1, b1); push(c0, s0, -h, c0, s0, 0, 0, 0, 1, b0); push(c1, s1, -h, c1, s1, 0, 0, 0, 1, b1);
    }
    return new Float32Array(v);
  }

  var VS = [
    "attribute vec3 p; attribute vec3 n; attribute vec2 t; attribute vec2 e;",
    "uniform mat4 mvp; uniform mat3 rot;",
    "varying vec3 vn; varying vec2 vt; varying vec2 ve; varying vec3 vw;",
    "void main(){ vn=n; vt=t; ve=e; vw=rot*p; gl_Position=mvp*vec4(p,1.0); }"
  ].join("\n");
  var FS = [
    "#ifdef GL_FRAGMENT_PRECISION_HIGH",
    "precision highp float;",
    "#else",
    "precision mediump float;",
    "#endif",
    "uniform sampler2D tex; uniform highp mat3 rot; uniform float px; uniform vec3 tint;",
    "varying vec3 vn; varying vec2 vt; varying vec2 ve; varying vec3 vw;",
    "float H(vec2 q){ return texture2D(tex,q).r; }",
    "void main(){",
    "  vec3 n=normalize(vn); float lift=0.0;",
    "  if(ve.x>0.5){",
    "    vec3 tg=vec3(-sin(ve.y),cos(ve.y),0.0);",
    "    n=normalize(n+tg*sin(ve.y*150.0)*0.45);",
    "  } else {",
    "    lift=H(vt);",
    "    float dx=H(vt+vec2(px,0.0))-H(vt-vec2(px,0.0));",
    "    float dy=H(vt+vec2(0.0,px))-H(vt-vec2(0.0,px));",
    "    float s=sign(vn.z);",
    "    n=normalize(vec3(-dx*s*3.2, dy*3.2, s));",
    "  }",
    "  vec3 N=normalize(rot*n); vec3 V=normalize(vec3(0.0,0.0,4.2)-vw);",
    "  vec3 L1=normalize(vec3(-0.5,0.8,0.9)); vec3 L2=normalize(vec3(0.9,-0.2,-0.6));",
    "  float d=max(dot(N,L1),0.0)*0.75+max(dot(N,L2),0.0)*0.25;",
    "  float sp=pow(max(dot(N,normalize(L1+V)),0.0),64.0)+0.5*pow(max(dot(N,normalize(L2+V)),0.0),32.0);",
    "  vec3 R=reflect(-V,N);",
    "  float env=smoothstep(-0.3,0.9,R.y)*0.55+0.25*smoothstep(0.85,1.0,abs(R.x));",
    "  float fr=pow(1.0-max(dot(N,V),0.0),3.0);",
    "  vec3 base=tint*mix(0.62,1.0,lift);",
    "  vec3 col=base*(0.3+d*0.72)+vec3(sp)*mix(0.55,0.95,lift)+base*env*0.6+vec3(fr*0.35);",
    "  gl_FragColor=vec4(col,1.0);",
    "}"
  ].join("\n");

  function mul(a, b) {
    var r = new Float32Array(16);
    for (var i = 0; i < 4; i++) for (var j = 0; j < 4; j++) {
      var s = 0;
      for (var k = 0; k < 4; k++) s += a[k * 4 + j] * b[i * 4 + k];
      r[i * 4 + j] = s;
    }
    return r;
  }
  function persp(fov, asp, near, far) {
    var f = 1 / Math.tan(fov / 2), nf = 1 / (near - far);
    return new Float32Array([f / asp, 0, 0, 0, 0, f, 0, 0, 0, 0, (far + near) * nf, -1, 0, 0, 2 * far * near * nf, 0]);
  }
  function rotation(ax, ay) {
    var cx = Math.cos(ax), sx = Math.sin(ax), cy = Math.cos(ay), sy = Math.sin(ay);
    // Ry then Rx, column-major 3×3
    return [cy, sx * sy, -cx * sy, 0, cx, sx, sy, -sx * cy, cx * cy];
  }

  function mount(el) {
    var size = el.offsetWidth || 112;
    var cv = document.createElement("canvas");
    var gl = cv.getContext("webgl", { alpha: true, antialias: true, premultipliedAlpha: true });
    if (!gl || !window.Path2D) return;
    var dpr = Math.min(2, window.devicePixelRatio || 1);
    cv.width = cv.height = Math.round(size * dpr);
    cv.className = "coin3d-gl";
    cv.setAttribute("aria-hidden", "true");

    function sh(type, src) {
      var s = gl.createShader(type);
      gl.shaderSource(s, src);
      gl.compileShader(s);
      return gl.getShaderParameter(s, gl.COMPILE_STATUS) ? s : null;
    }
    var vs = sh(gl.VERTEX_SHADER, VS), fs = sh(gl.FRAGMENT_SHADER, FS);
    if (!vs || !fs) return;
    var pr = gl.createProgram();
    gl.attachShader(pr, vs); gl.attachShader(pr, fs); gl.linkProgram(pr);
    if (!gl.getProgramParameter(pr, gl.LINK_STATUS)) return;
    gl.useProgram(pr);

    var data = geometry(128, 0.085), stride = 40;
    var buf = gl.createBuffer();
    gl.bindBuffer(gl.ARRAY_BUFFER, buf);
    gl.bufferData(gl.ARRAY_BUFFER, data, gl.STATIC_DRAW);
    [["p", 3, 0], ["n", 3, 12], ["t", 2, 24], ["e", 2, 32]].forEach(function (a) {
      var loc = gl.getAttribLocation(pr, a[0]);
      gl.enableVertexAttribArray(loc);
      gl.vertexAttribPointer(loc, a[1], gl.FLOAT, false, stride, a[2]);
    });
    var TEX = 512;
    var tex = gl.createTexture();
    gl.bindTexture(gl.TEXTURE_2D, tex);
    gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, gl.RGBA, gl.UNSIGNED_BYTE, relief(TEX));
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.LINEAR);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
    var U = {};
    ["mvp", "rot", "px", "tint", "tex"].forEach(function (k) { U[k] = gl.getUniformLocation(pr, k); });
    gl.uniform1f(U.px, 1.5 / TEX);
    gl.uniform1i(U.tex, 0);
    gl.enable(gl.DEPTH_TEST);
    gl.viewport(0, 0, cv.width, cv.height);
    var proj = persp(0.52, 1, 0.1, 20);

    el.textContent = "";
    el.classList.add("is-gl");
    el.appendChild(cv);

    var reduce = window.matchMedia && matchMedia("(prefers-reduced-motion: reduce)").matches;
    var tilt = { x: 0, y: 0 }, aim = { x: 0, y: 0 };
    var host = el.closest(".hero-stage") || el.parentNode;
    host.addEventListener("pointermove", function (ev) {
      var r = host.getBoundingClientRect();
      aim.x = ((ev.clientY - r.top) / r.height - 0.5) * 0.5;
      aim.y = ((ev.clientX - r.left) / r.width - 0.5) * 0.8;
    });
    host.addEventListener("pointerleave", function () { aim.x = aim.y = 0; });

    var visible = true;
    if ("IntersectionObserver" in window) {
      new IntersectionObserver(function (es) { visible = es[0].isIntersecting; if (visible) requestAnimationFrame(frame); }).observe(cv);
    }
    var t0 = performance.now();
    function frame(now) {
      var light = document.documentElement.getAttribute("data-theme") === "light";
      gl.uniform3fv(U.tint, light ? [0.56, 0.58, 0.62] : [0.82, 0.84, 0.88]);
      tilt.x += (aim.x - tilt.x) * 0.08;
      tilt.y += (aim.y - tilt.y) * 0.08;
      var spin = reduce ? -0.32 : ((now - t0) / 9000) * Math.PI * 2;
      var r3 = rotation(0.14 + tilt.x, spin + tilt.y);
      var model = new Float32Array([r3[0], r3[1], r3[2], 0, r3[3], r3[4], r3[5], 0, r3[6], r3[7], r3[8], 0, 0, 0, -4.2, 1]);
      gl.uniformMatrix4fv(U.mvp, false, mul(proj, model));
      gl.uniformMatrix3fv(U.rot, false, new Float32Array(r3));
      gl.clearColor(0, 0, 0, 0);
      gl.clear(gl.COLOR_BUFFER_BIT | gl.DEPTH_BUFFER_BIT);
      gl.drawArrays(gl.TRIANGLES, 0, data.length / 10);
      if (visible && !document.hidden && !reduce) requestAnimationFrame(frame);
    }
    document.addEventListener("visibilitychange", function () { if (!document.hidden) requestAnimationFrame(frame); });
    requestAnimationFrame(frame);
  }

  function start() { document.querySelectorAll(".coin3d").forEach(mount); }
  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", start); else start();
})();
