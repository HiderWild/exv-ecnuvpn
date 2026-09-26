// 开发期从活动三维模型导出灰阶 SVG；产品只加载导出的矢量，不运行此脚本。
import { readFile, mkdir, writeFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import * as Three from 'three';
import { projectVectorScene } from './project-vector-scene.mjs';
import { Window } from 'happy-dom';
import ts from 'typescript';

const root = new URL('../', import.meta.url);
const window = new Window();
globalThis.window = window;
globalThis.document = window.document;
let captured;

// 导出使用相同建模代码，WebGL 环境贴图和阴影由离线适配器跳过。
class CaptureRenderer {
  shadowMap = {};
  setPixelRatio() {}
  setClearColor() {}
  setSize() {}
  dispose() {}
  render(scene, camera) { captured = { scene, camera }; }
}
class Environment extends Three.Scene { dispose() {} }
class EnvironmentGenerator {
  fromScene() { return { texture: null, dispose() {} }; }
  dispose() {}
}
const THREE = {
  ...Three,
  WebGLRenderer: CaptureRenderer,
  PMREMGenerator: EnvironmentGenerator,
  // SVG 只需要轮廓与少量明暗面，不输出高细分网格。
  TorusGeometry: class extends Three.TorusGeometry {
    constructor(radius, tube) { super(radius, tube, 4, 24); }
  },
  CylinderGeometry: class extends Three.CylinderGeometry {
    constructor(top, bottom, height, segments, heightSegments, open) { super(top, bottom, height, 24, 1, open); }
  },
};
function compile(source, dependencies) {
  const { outputText } = ts.transpileModule(source, { compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 } });
  const exports = {};
  new Function('require', 'exports', outputText)((name) => {
    if (!(name in dependencies)) throw new Error(`导出未支持的模型依赖：${name}`);
    return dependencies[name];
  }, exports);
  return exports;
}
const motion = compile(await readFile(new URL('src/product/scene-motion.ts', root), 'utf8'), {});
const model = compile(await readFile(new URL('src/product/connection-sculpture.ts', root), 'utf8'), {
  three: THREE,
  'three/addons/environments/RoomEnvironment.js': { RoomEnvironment: Environment },
  'three/addons/geometries/RoundedBoxGeometry.js': { RoundedBoxGeometry: class extends Three.BoxGeometry {
    constructor(width, height, depth) { super(width, height, depth); }
  } },
  './scene-motion': motion,
});
const output = new URL('src/assets/connection-vectors/', root);
await mkdir(output, { recursive: true });
for (const state of ['idle', 'working', 'connected']) {
  const sculpture = model.createConnectionSculpture(document.createElement('canvas'));
  sculpture.resize(900, 600);
  sculpture.setPresentation({ state, reduced: true, dark: false, accent: '#777777' });
  sculpture.render(0);
  const { scene, camera } = captured;
  scene.environment = null;
  const disposable = [];
  scene.traverse(object => {
    if (object.isLight) {
      object.color.set('#ffffff');
      if (object.groundColor) object.groundColor.set('#ffffff');
      object.intensity = object.isHemisphereLight ? 0.45 : 0.65;
    }
    if (!object.isMesh) return;
    if (object.material.isShaderMaterial) { object.visible = false; return; }
    const source = object.material;
    const gray = source.color.r * 0.2126 + source.color.g * 0.7152 + source.color.b * 0.0722;
    const material = new Three.MeshLambertMaterial({
      color: new Three.Color().setRGB(gray, gray, gray),
      transparent: source.transparent, opacity: source.opacity,
      side: source.side,
    });
    object.material = material;
    disposable.push(source);
  });
  const markup = projectVectorScene(scene, camera, 900, 600);
  const target = new URL(`${state}.svg`, output);
  await writeFile(target, markup + '\n');
  console.log(`${fileURLToPath(target)}: ${Buffer.byteLength(markup)} 字节`);
  sculpture.dispose();
  disposable.forEach(material => material.dispose());
}
await window.happyDOM.close();
