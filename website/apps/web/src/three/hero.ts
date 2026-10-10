import * as THREE from 'three';
import { RoomEnvironment } from 'three/examples/jsm/environments/RoomEnvironment.js';

export interface HeroHandle {
  setTheme(theme: 'dark' | 'light'): void;
  pulse(height?: number): void;
  dispose(): void;
}

const DEG = Math.PI / 180;

/** Brand geometry on the 256 grid (scripts/brand/render_logo.py), re-centred with y up. */
function signShapes(): THREE.Shape[] {
  const R = 58;
  const S = 30;
  const outer = R + S / 2;
  const inner = R - S / 2;
  const end = -318 * DEG;
  const ring = new THREE.Shape();
  ring.moveTo(outer, 0);
  ring.absarc(0, 0, outer, 0, end, true);
  ring.lineTo(inner * Math.cos(end), inner * Math.sin(end));
  ring.absarc(0, 0, inner, end, 0, false);
  ring.lineTo(outer, 0);

  const rect = (x0: number, y0: number, x1: number, y1: number) => {
    const s = new THREE.Shape();
    s.moveTo(x0, y0);
    s.lineTo(x1, y0);
    s.lineTo(x1, y1);
    s.lineTo(x0, y1);
    s.lineTo(x0, y0);
    return s;
  };
  // crossbar: x 134..201, y 113..143 -> X 6..73, Y -15..15
  const bar = rect(6, -15, 73, 15);
  // ticks: x 106..120 / 136..150, y 31..85 / 171..225
  const ticks = [rect(-22, 43, -8, 97), rect(8, 43, 22, 97), rect(-22, -97, -8, -43), rect(8, -97, 22, -43)];
  return [ring, bar, ...ticks];
}

export function createHero(container: HTMLElement, initialTheme: 'dark' | 'light', reducedMotion: boolean, textEdge: () => number = () => 0): HeroHandle {
  const renderer = new THREE.WebGLRenderer({ antialias: true, alpha: true, powerPreference: 'high-performance' });
  renderer.setPixelRatio(Math.min(window.devicePixelRatio, 2));
  renderer.outputColorSpace = THREE.SRGBColorSpace;
  renderer.toneMapping = THREE.ACESFilmicToneMapping;
  renderer.toneMappingExposure = 1.05;
  container.appendChild(renderer.domElement);

  const scene = new THREE.Scene();
  const pmrem = new THREE.PMREMGenerator(renderer);
  const env = pmrem.fromScene(new RoomEnvironment(), 0.04).texture;
  scene.environment = env;

  const camera = new THREE.PerspectiveCamera(32, 1, 0.1, 100);
  camera.position.set(0, 0, 11);

  const root = new THREE.Group();
  scene.add(root);

  // --- The sign -----------------------------------------------------------
  const geo = new THREE.ExtrudeGeometry(signShapes(), {
    depth: 26,
    bevelEnabled: true,
    bevelThickness: 4,
    bevelSize: 2.4,
    bevelSegments: 6,
    curveSegments: 96,
  });
  geo.center();
  geo.scale(1 / 46, 1 / 46, 1 / 46);
  const signMat = new THREE.MeshPhysicalMaterial({ color: 0xf2f2f2, metalness: 1, roughness: 0.22, clearcoat: 1, clearcoatRoughness: 0.15, envMapIntensity: 1.3 });
  const sign = new THREE.Mesh(geo, signMat);
  root.add(sign);

  // Soft disc shadow / halo behind the sign
  const haloMat = new THREE.MeshBasicMaterial({ color: 0xffffff, transparent: true, opacity: 0.05, depthWrite: false });
  const halo = new THREE.Mesh(new THREE.RingGeometry(2.25, 2.3, 128), haloMat);
  halo.position.z = -0.8;
  root.add(halo);
  const halo2 = new THREE.Mesh(new THREE.RingGeometry(3.05, 3.065, 128), haloMat.clone());
  halo2.position.z = -1.2;
  root.add(halo2);

  // --- Orbiting chain of blocks ---------------------------------------------
  const chain = new THREE.Group();
  chain.rotation.x = 72 * DEG;
  chain.rotation.y = -12 * DEG;
  root.add(chain);
  const N = 28;
  const ORBIT = 3.4;
  const boxGeo = new THREE.BoxGeometry(0.2, 0.2, 0.2);
  const edgeGeo = new THREE.EdgesGeometry(boxGeo);
  const blockMat = new THREE.MeshPhysicalMaterial({ color: 0xdddddd, metalness: 0.9, roughness: 0.3, transparent: true, opacity: 0.9 });
  const edgeMat = new THREE.LineBasicMaterial({ color: 0xffffff, transparent: true, opacity: 0.6 });
  const blocks: Array<{ mesh: THREE.Mesh; edge: THREE.LineSegments; flash: number }> = [];
  for (let i = 0; i < N; i++) {
    const a = (i / N) * Math.PI * 2;
    const mesh = new THREE.Mesh(boxGeo, blockMat.clone());
    mesh.position.set(Math.cos(a) * ORBIT, Math.sin(a) * ORBIT, 0);
    mesh.rotation.set(Math.random() * 3, Math.random() * 3, 0);
    const edge = new THREE.LineSegments(edgeGeo, edgeMat.clone());
    edge.scale.setScalar(1.35);
    mesh.add(edge);
    chain.add(mesh);
    blocks.push({ mesh, edge, flash: 0 });
  }
  const linkPts: THREE.Vector3[] = [];
  for (let i = 0; i <= 256; i++) {
    const a = (i / 256) * Math.PI * 2;
    linkPts.push(new THREE.Vector3(Math.cos(a) * ORBIT, Math.sin(a) * ORBIT, 0));
  }
  const linkMat = new THREE.LineDashedMaterial({ color: 0xffffff, transparent: true, opacity: 0.22, dashSize: 0.08, gapSize: 0.06 });
  const link = new THREE.Line(new THREE.BufferGeometry().setFromPoints(linkPts), linkMat);
  link.computeLineDistances();
  chain.add(link);

  // --- Particle field -------------------------------------------------------
  const COUNT = window.innerWidth < 700 ? 700 : 1600;
  const pos = new Float32Array(COUNT * 3);
  for (let i = 0; i < COUNT; i++) {
    const r = 4 + Math.random() * 9;
    const t = Math.random() * Math.PI * 2;
    const p = Math.acos(2 * Math.random() - 1);
    pos[i * 3] = r * Math.sin(p) * Math.cos(t);
    pos[i * 3 + 1] = r * Math.sin(p) * Math.sin(t) * 0.7;
    pos[i * 3 + 2] = r * Math.cos(p) - 4;
  }
  const dustGeo = new THREE.BufferGeometry();
  dustGeo.setAttribute('position', new THREE.BufferAttribute(pos, 3));
  const dustMat = new THREE.PointsMaterial({ color: 0xffffff, size: 0.022, transparent: true, opacity: 0.55, depthWrite: false, sizeAttenuation: true });
  const dust = new THREE.Points(dustGeo, dustMat);
  scene.add(dust);

  // --- Lights ---------------------------------------------------------------
  const key = new THREE.DirectionalLight(0xffffff, 2.2);
  key.position.set(4, 5, 6);
  scene.add(key);
  const rim = new THREE.DirectionalLight(0xffffff, 1.4);
  rim.position.set(-6, -2, -4);
  scene.add(rim);
  scene.add(new THREE.AmbientLight(0xffffff, 0.25));

  // --- Theme ----------------------------------------------------------------
  const applyTheme = (t: 'dark' | 'light') => {
    const dark = t === 'dark';
    signMat.color.set(dark ? 0xf2f2f2 : 0x1a1a1a);
    signMat.metalness = dark ? 1 : 0.85;
    signMat.roughness = dark ? 0.22 : 0.3;
    signMat.envMapIntensity = dark ? 1.3 : 0.9;
    const ink = dark ? 0xffffff : 0x0b0b0b;
    haloMat.color.set(ink);
    (halo2.material as THREE.MeshBasicMaterial).color.set(ink);
    haloMat.opacity = dark ? 0.06 : 0.1;
    (halo2.material as THREE.MeshBasicMaterial).opacity = dark ? 0.08 : 0.12;
    linkMat.color.set(ink);
    linkMat.opacity = dark ? 0.22 : 0.3;
    dustMat.color.set(ink);
    dustMat.opacity = dark ? 0.5 : 0.35;
    for (const b of blocks) {
      (b.mesh.material as THREE.MeshPhysicalMaterial).color.set(dark ? 0xd8d8d8 : 0x222222);
      (b.edge.material as THREE.LineBasicMaterial).color.set(ink);
    }
    renderer.toneMappingExposure = dark ? 1.05 : 0.95;
  };
  applyTheme(initialTheme);

  // --- Layout / interaction ------------------------------------------------
  let w = 1;
  let h = 1;
  const layout = () => {
    w = container.clientWidth || 1;
    h = container.clientHeight || 1;
    renderer.setSize(w, h, false);
    camera.aspect = w / h;
    camera.updateProjectionMatrix();
    const halfH = Math.tan((camera.fov / 2) * DEG) * camera.position.z;
    const halfW = halfH * camera.aspect;
    const pxPerUnit = w / (2 * halfW);
    const textRight = Math.min(w * 0.62, Math.max(0, textEdge()));
    const free = w - textRight;
    const side = w >= 760 && free > 260;
    container.dataset.mode = side ? 'side' : 'top';
    if (side) {
      // Orbit radius is 3.4 units; keep the whole ring inside the free area right of the text.
      const ringPx = Math.min(free * 0.4, h * 0.4);
      const scale = ringPx / (3.55 * pxPerUnit);
      const centerPx = textRight + free * 0.56;
      root.position.set((centerPx - w / 2) / pxPerUnit, 0, 0);
      root.scale.setScalar(scale);
    } else {
      root.position.set(0, halfH * 0.38, 0);
      root.scale.setScalar(Math.min(0.62, halfW / 4.2));
    }
  };
  layout();
  const ro = new ResizeObserver(layout);
  ro.observe(container);
  // Web fonts can change the text width after first layout.
  void document.fonts?.ready.then(layout);

  const target = { x: 0, y: 0 };
  const onPointer = (e: PointerEvent) => {
    target.x = (e.clientX / window.innerWidth) * 2 - 1;
    target.y = (e.clientY / window.innerHeight) * 2 - 1;
  };
  window.addEventListener('pointermove', onPointer, { passive: true });

  let visible = true;
  const io = new IntersectionObserver(([e]) => (visible = !!e?.isIntersecting));
  io.observe(container);

  let pulseIdx = 0;
  let spin = 0;
  let raf = 0;
  const clock = new THREE.Clock();
  const tick = () => {
    raf = requestAnimationFrame(tick);
    if (!visible || document.hidden) return;
    const dt = Math.min(clock.getDelta(), 0.05);
    const t = clock.elapsedTime;
    sign.rotation.y += ((target.x * 0.5 + Math.sin(t * 0.35) * 0.35) - sign.rotation.y) * 0.04;
    sign.rotation.x += ((target.y * 0.25 + Math.cos(t * 0.3) * 0.08) - sign.rotation.x) * 0.04;
    sign.position.y = Math.sin(t * 0.8) * 0.06;
    spin += dt * 0.12;
    chain.rotation.z = spin;
    for (const b of blocks) {
      b.mesh.rotation.x += dt * 0.4;
      b.mesh.rotation.y += dt * 0.3;
      if (b.flash > 0) {
        b.flash = Math.max(0, b.flash - dt * 0.9);
        const s = 1 + b.flash * 1.6;
        b.mesh.scale.setScalar(s);
        (b.edge.material as THREE.LineBasicMaterial).opacity = 0.6 + b.flash * 0.4;
      }
    }
    dust.rotation.y = t * 0.015;
    dust.rotation.x = Math.sin(t * 0.05) * 0.05;
    halo.scale.setScalar(1 + Math.sin(t * 1.2) * 0.012);
    renderer.render(scene, camera);
  };

  if (reducedMotion) renderer.render(scene, camera);
  else tick();

  return {
    setTheme(t) {
      applyTheme(t);
      if (reducedMotion) renderer.render(scene, camera);
    },
    pulse() {
      const b = blocks[pulseIdx % N]!;
      b.flash = 1;
      pulseIdx += 3;
      spin += (Math.PI * 2) / N;
    },
    dispose() {
      cancelAnimationFrame(raf);
      ro.disconnect();
      io.disconnect();
      window.removeEventListener('pointermove', onPointer);
      scene.traverse((o) => {
        const m = o as THREE.Mesh;
        m.geometry?.dispose?.();
        const mat = m.material as THREE.Material | THREE.Material[] | undefined;
        if (Array.isArray(mat)) mat.forEach((x) => x.dispose());
        else mat?.dispose?.();
      });
      env.dispose();
      pmrem.dispose();
      renderer.dispose();
      renderer.domElement.remove();
    },
  };
}
