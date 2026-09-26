import * as THREE from "three";
import { RoundedBoxGeometry } from "three/addons/geometries/RoundedBoxGeometry.js";
import { RoomEnvironment } from "three/addons/environments/RoomEnvironment.js";
import { advanceSceneMotion, type ScenePresentation } from "./scene-motion";

export interface SculpturePresentation extends ScenePresentation { dark: boolean; accent: string }

/** 固定相机的小型产品场景。几何、灯光与动作分开控制，不依赖网络模型或贴图。 */
export function createConnectionSculpture(canvas: HTMLCanvasElement) {
  const renderer = new THREE.WebGLRenderer({ canvas, alpha: true, antialias: true, powerPreference: "low-power" });
  const scene = new THREE.Scene();
  const resources = new Set<{ dispose(): void }>([renderer]);
  let disposed = false;
  function own<T extends { dispose(): void }>(resource: T): T { resources.add(resource); return resource; }
  function release(resource: { dispose(): void }) { resource.dispose(); resources.delete(resource); }
  function dispose() {
    if (disposed) return;
    disposed = true;
    scene.traverse(object => {
      if (object instanceof THREE.Mesh) {
        resources.add(object.geometry);
        (Array.isArray(object.material) ? object.material : [object.material]).forEach(material => resources.add(material));
      }
    });
    resources.forEach(resource => resource.dispose());
    resources.clear();
  }
  // 工厂还未返回时也负责清理；外层此时没有可调用 dispose 的场景句柄。
  try {
  renderer.setPixelRatio(Math.min(window.devicePixelRatio || 1, 1.75));
  renderer.setClearColor(0x000000, 0);
  renderer.toneMapping = THREE.ACESFilmicToneMapping;
  renderer.toneMappingExposure = 1.05;
  renderer.shadowMap.enabled = true;
  renderer.shadowMap.type = THREE.PCFSoftShadowMap;

  const camera = new THREE.OrthographicCamera(-5, 5, 3.3, -3.3, 0.1, 60);
  camera.position.set(7.6, 5.8, 11);
  camera.lookAt(0, 1.05, 0);
  const environment = own(new RoomEnvironment());
  const pmrem = own(new THREE.PMREMGenerator(renderer));
  const environmentTarget = own(pmrem.fromScene(environment, 0.025));
  scene.environment = environmentTarget.texture;
  release(environment);
  release(pmrem);

  const ambient = new THREE.HemisphereLight(0xe5efff, 0xa4aebc, 2.3);
  scene.add(ambient);
  const key = new THREE.DirectionalLight(0xfff5e9, 4.2);
  own(key.shadow);
  key.position.set(-3, 8, 5);
  key.castShadow = true;
  key.shadow.mapSize.set(1024, 1024);
  Object.assign(key.shadow.camera, { left: -6, right: 6, top: 5, bottom: -5, near: 0.5, far: 25 });
  key.shadow.bias = -0.001;
  key.shadow.normalBias = 0.035;
  key.shadow.radius = 4;
  scene.add(key);
  const rim = new THREE.DirectionalLight(0x86b8ff, 2.4);
  rim.position.set(4, 4, -5);
  scene.add(rim);

  const shell = new THREE.MeshStandardMaterial({ color: 0xdce3ec, metalness: 0.48, roughness: 0.27 });
  const edge = new THREE.MeshStandardMaterial({ color: 0x8998ac, metalness: 0.65, roughness: 0.26 });
  const face = new THREE.MeshStandardMaterial({ color: 0xf1f5fa, metalness: 0.24, roughness: 0.32 });
  const inset = new THREE.MeshStandardMaterial({ color: 0x24364b, metalness: 0.38, roughness: 0.4 });
  const screen = new THREE.MeshStandardMaterial({ color: 0x172b44, metalness: 0.15, roughness: 0.3 });
  const baseMat = new THREE.MeshStandardMaterial({ color: 0xe4e9f0, metalness: 0.35, roughness: 0.32 });
  const glow = new THREE.MeshStandardMaterial({ color: 0x218bee, emissive: 0x2680e8, emissiveIntensity: 0.25, metalness: 0, roughness: 0.4, envMapIntensity: 0.15 });
  const lamp = new THREE.MeshStandardMaterial({ color: 0x93a6be, emissive: 0x366bb0, emissiveIntensity: 0.05, roughness: 0.4, envMapIntensity: 0.1 });
  const glass = new THREE.MeshPhysicalMaterial({ color: 0x8bc9ff, metalness: 0.15, roughness: 0.18, transparent: true, opacity: 0.22, depthWrite: false, clearcoat: 1, side: THREE.DoubleSide });
  const railMat = new THREE.MeshStandardMaterial({ color: 0xc1ccda, metalness: 0.72, roughness: 0.28 });
  const signalMat = new THREE.MeshBasicMaterial({ color: 0x8bd5ff, transparent: true, opacity: 0.9 });
  [shell, edge, face, inset, screen, baseMat, glow, lamp, glass, railMat, signalMat].forEach(own);

  function box(parent: THREE.Object3D, w: number, h: number, d: number, x: number, y: number, z: number, material: THREE.Material, radius = 0.08) {
    const mesh = new THREE.Mesh(new RoundedBoxGeometry(w, h, d, 3, Math.min(radius, Math.min(w, h, d) / 2)), material);
    mesh.position.set(x, y, z);
    mesh.castShadow = true;
    mesh.receiveShadow = true;
    parent.add(mesh);
    return mesh;
  }
  function ring(parent: THREE.Object3D, x: number, radius: number, thickness: number, material: THREE.Material) {
    const mesh = new THREE.Mesh(new THREE.TorusGeometry(radius, thickness, 10, 56), material);
    mesh.rotation.y = Math.PI / 2;
    mesh.position.set(x, 1.25, 0);
    parent.add(mesh);
    return mesh;
  }
  function cylinder(parent: THREE.Object3D, length: number, radius: number, material: THREE.Material) {
    const mesh = new THREE.Mesh(new THREE.CylinderGeometry(radius, radius, length, 48, 1, true), material);
    mesh.rotation.z = Math.PI / 2;
    parent.add(mesh);
    return mesh;
  }

  const world = new THREE.Group();
  scene.add(world);
  // 单一基座把三个主要部件放在同一个物理空间中。
  box(world, 7.3, 0.2, 3.1, 0, 0.12, 0, edge, 0.1);
  box(world, 7.3, 0.16, 3.1, 0, 0.24, 0, baseMat, 0.1);
  box(world, 6.75, 0.022, 0.035, 0, 0.24, 1.556, railMat, 0.01);
  for (const x of [-2.9, 2.9]) for (const z of [-1, 1]) box(world, 0.45, 0.11, 0.45, x, 0.02, z, inset, 0.05);

  // 客户端终端：屏幕、支架、底座各自保留厚度，屏幕图形不承载状态文案。
  const terminal = new THREE.Group();
  terminal.position.set(-2.35, 0.34, 0.35);
  world.add(terminal);
  box(terminal, 1.85, 0.16, 1.15, 0, 0.12, 0.08, shell);
  box(terminal, 0.24, 0.72, 0.26, 0, 0.5, -0.12, edge, 0.055);
  const display = new THREE.Group();
  display.position.set(0, 1.24, -0.09);
  display.rotation.x = -0.09;
  terminal.add(display);
  box(display, 1.95, 1.36, 0.18, 0, 0, 0, shell, 0.09);
  box(display, 1.76, 1.15, 0.045, 0, 0.025, 0.106, screen, 0.06);
  box(display, 0.045, 0.025, 0.014, 0, -0.615, 0.104, lamp, 0.012);
  // 抽象窗口和路径图：只表达这是连接终端，不伪装真实统计。
  box(display, 1.30, 0.78, 0.014, 0, 0.025, 0.135, inset, 0.04);
  for (let i = 0; i < 3; i++) box(display, 0.035, 0.035, 0.012, -0.55 + i * 0.075, 0.32, 0.15, edge, 0.01);
  box(display, 0.5, 0.025, 0.014, 0, 0.015, 0.16, glow, 0.01);
  box(display, 0.22, 0.24, 0.025, -0.37, 0.015, 0.16, glow, 0.035);
  box(display, 0.22, 0.24, 0.025, 0.37, 0.015, 0.16, glow, 0.035);
  box(display, 0.5, 0.024, 0.014, 0, -0.23, 0.16, edge, 0.01);
  // 与通道相接的终端接口。
  box(world, 0.35, 0.8, 0.8, -1.23, 1.25, 0, shell, 0.09);
  ring(world, -1.04, 0.30, 0.06, edge);

  // 机柜：保持少量有分量的模块，通风孔用凹色条而非大量几何细孔。
  const cabinet = new THREE.Group();
  cabinet.position.set(2.3, 0.34, -0.2);
  world.add(cabinet);
  box(cabinet, 1.62, 2.72, 1.58, 0, 1.4, 0, shell, 0.13);
  box(cabinet, 1.43, 2.43, 0.07, 0, 1.42, 0.803, inset, 0.065);
  for (let i = 0; i < 4; i++) {
    const y = 0.57 + i * 0.56;
    box(cabinet, 1.27, 0.46, 0.11, 0, y, 0.85, face, 0.055);
    box(cabinet, 0.075, 0.18, 0.022, -0.48, y, 0.917, lamp, 0.025);
    for (let j = 0; j < 5; j++) box(cabinet, 0.055, 0.12, 0.018, -0.20 + j * 0.105, y, 0.916, inset, 0.015);
    box(cabinet, 0.12, 0.06, 0.016, 0.46, y, 0.916, edge, 0.012);
  }
  box(cabinet, 0.045, 1.77, 1.12, 0.82, 1.39, -0.04, edge, 0.015);
  for (let i = 0; i < 7; i++) box(cabinet, 0.024, 0.025, 0.73, 0.85, 1.0 + i * 0.13, -0.02, inset, 0.009);
  box(world, 0.22, 0.85, 0.85, 1.44, 1.25, 0, edge, 0.07);
  ring(world, 1.30, 0.30, 0.055, railMat);

  // 两端伸出的半通道在成功前始终保留间隙；扫描不是完成百分比。
  const leftTube = cylinder(world, 1.16, 0.285, glass);
  const rightTube = cylinder(world, 1.16, 0.285, glass);
  const innerLeft = cylinder(world, 1.16, 0.065, glow);
  const innerRight = cylinder(world, 1.16, 0.065, glow);
  const ribs = Array.from({ length: 5 }, (_, i) => ring(world, -1 + i * 0.56, 0.292, 0.015, railMat));
  const scanner = ring(world, 0, 0.305, 0.032, signalMat);
  const seam = ring(world, 0.13, 0.3, 0.026, glow);

  // 小型盾牌为通道附属，不再增加一个同等重要的场景主体。
  const shield = new THREE.Group();
  shield.position.set(0.17, 1.72, 0.36);
  const shape = new THREE.Shape();
  shape.moveTo(0, 0.38); shape.lineTo(0.3, 0.25); shape.lineTo(0.28, -0.08);
  shape.quadraticCurveTo(0.22, -0.30, 0, -0.43);
  shape.quadraticCurveTo(-0.22, -0.30, -0.28, -0.08);
  shape.lineTo(-0.3, 0.25); shape.closePath();
  const badge = new THREE.Mesh(new THREE.ExtrudeGeometry(shape, { depth: 0.07, bevelEnabled: true, bevelSize: 0.025, bevelThickness: 0.025, bevelSegments: 3, steps: 1 }), face);
  badge.castShadow = true;
  shield.add(badge);
  const tickCurve = new THREE.CatmullRomCurve3([new THREE.Vector3(-0.14, 0, 0.11), new THREE.Vector3(-0.04, -0.10, 0.11), new THREE.Vector3(0.15, 0.13, 0.11)]);
  const tick = new THREE.Mesh(new THREE.TubeGeometry(tickCurve, 16, 0.025, 8, false), glow);
  shield.add(tick);
  world.add(shield);

  const groundMaterial = new THREE.ShaderMaterial({
    transparent: true, depthWrite: false,
    uniforms: { opacity: { value: 0.18 } },
    vertexShader: "varying vec2 vUv; void main(){vUv=uv;gl_Position=projectionMatrix*modelViewMatrix*vec4(position,1.);}",
    fragmentShader: "varying vec2 vUv; uniform float opacity; void main(){vec2 p=(vUv-.5)*2.;float a=1.-smoothstep(.3,1.,length(p));gl_FragColor=vec4(.08,.12,.2,a*opacity);}",
  });
  const ground = new THREE.Mesh(new THREE.PlaneGeometry(9.2, 5.2), groundMaterial);
  ground.rotation.x = -Math.PI / 2;
  ground.position.y = -0.065;
  scene.add(ground);

  let input: SculpturePresentation = { state: "idle", dark: false, accent: "#3989ee" };
  let pose = { extension: 0, phase: 0 };
  function setPresentation(next: SculpturePresentation) {
    input = next;
    const dark = next.dark;
    shell.color.set(dark ? 0x445a76 : 0xdce3ec);
    face.color.set(dark ? 0x66809e : 0xf1f5fa);
    baseMat.color.set(dark ? 0x26374d : 0xe4e9f0);
    edge.color.set(dark ? 0x718aa8 : 0x8998ac);
    railMat.color.set(dark ? 0x7891b1 : 0xc1ccda);
    inset.color.set(dark ? 0x111f30 : 0x24364b);
    glow.color.set(next.accent);
    glow.emissive.set(next.accent);
    glass.color.copy(glow.color);
    signalMat.color.copy(glow.color).lerp(new THREE.Color(0xffffff), 0.4);
    lamp.color.set(next.state === "connected" ? 0x65d9b1 : next.state === "working" && !next.stopping ? next.accent : dark ? 0x738ba7 : 0x93a6be);
    lamp.emissive.copy(lamp.color);
    lamp.emissiveIntensity = next.state === "idle" || next.stopping ? 0 : 0.65;
    ambient.intensity = dark ? 0.9 : 1.1;
    key.intensity = dark ? 2.6 : 3.0;
    scene.environmentIntensity = dark ? 0.7 : 0.85;
    groundMaterial.uniforms.opacity.value = dark ? 0.4 : 0.2;
  }
  function resize(width: number, height: number) {
    if (disposed || width <= 0 || height <= 0) return;
    renderer.setSize(width, height, false);
    const aspect = width / height;
    const halfWidth = Math.max(4.4, 2.8 * aspect);
    camera.left = -halfWidth; camera.right = halfWidth;
    camera.top = halfWidth / aspect; camera.bottom = -halfWidth / aspect;
    camera.updateProjectionMatrix();
  }
  function render(delta = 1 / 30) {
    if (disposed) return false;
    const previous = pose.extension;
    pose = advanceSceneMotion(pose, input, delta);
    const amount = 0.12 + pose.extension * 0.88;
    for (const [tube, core, start, direction] of [[leftTube, innerLeft, -1.03, 1], [rightTube, innerRight, 1.29, -1]] as const) {
      tube.scale.y = amount; core.scale.y = amount;
      tube.position.set(start + direction * 1.16 * amount / 2, 1.25, 0);
      core.position.copy(tube.position);
    }
    ribs.forEach((rib, index) => {
      const distance = Math.min(index, 4 - index) / 2;
      rib.visible = distance < amount;
    });
    const working = input.state === "working" && !input.stopping;
    scanner.visible = working;
    scanner.position.x = -0.9 + pose.phase * 2.05;
    scanner.scale.setScalar(1 + Math.sin(pose.phase * Math.PI) * 0.035);
    seam.visible = input.state === "connected";
    shield.visible = input.state === "connected";
    shield.scale.setScalar(0.88 + pose.extension * 0.12);
    glass.opacity = input.state === "idle" || input.stopping ? 0.10 : 0.38;
    glow.emissiveIntensity = input.state === "idle" || input.stopping ? 0 : input.dark ? 0.6 : 0.3;
    renderer.render(scene, camera);
    return !input.paused && !input.reduced && (working || pose.extension !== previous);
  }
  setPresentation(input);
  return { setPresentation, resize, render, dispose };
  } catch (error) {
    dispose();
    throw error;
  }
}
