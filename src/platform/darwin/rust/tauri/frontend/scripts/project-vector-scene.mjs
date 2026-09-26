import { Vector3, Color } from 'three';

// SVG 没有深度缓冲。用 BSP 切分相交面并按相机远近绘制，避免大面盖住前方细节。
const EPSILON = 1e-5;
function partition(polygons) {
  if (!polygons.length) return null;
  const pivot = polygons[Math.floor(polygons.length / 2)];
  const normal = pivot.normal;
  const distance = normal.dot(pivot.vertices[0]);
  const front = [], back = [], coplanar = [];
  for (const polygon of polygons) {
    const distances = polygon.vertices.map(v => normal.dot(v) - distance);
    const positive = distances.some(d => d > EPSILON);
    const negative = distances.some(d => d < -EPSILON);
    if (!positive && !negative) { coplanar.push(polygon); continue; }
    if (!negative) { front.push(polygon); continue; }
    if (!positive) { back.push(polygon); continue; }
    const frontVertices = [], backVertices = [];
    for (let i = 0; i < polygon.vertices.length; i++) {
      const j = (i + 1) % polygon.vertices.length;
      const a = polygon.vertices[i], b = polygon.vertices[j];
      const da = distances[i], db = distances[j];
      if (da >= -EPSILON) frontVertices.push(a);
      if (da <= EPSILON) backVertices.push(a);
      if ((da > EPSILON && db < -EPSILON) || (da < -EPSILON && db > EPSILON)) {
        const intersection = a.clone().lerp(b, da / (da - db));
        frontVertices.push(intersection); backVertices.push(intersection);
      }
    }
    if (frontVertices.length >= 3) front.push({ ...polygon, vertices: frontVertices });
    if (backVertices.length >= 3) back.push({ ...polygon, vertices: backVertices });
  }
  return { normal, distance, coplanar, front: partition(front), back: partition(back) };
}

export function projectVectorScene(scene, camera, width, height) {
  scene.updateMatrixWorld(true);
  camera.updateMatrixWorld(true);
  const towardCamera = camera.getWorldDirection(new Vector3()).negate();
  const light = new Vector3(-3, 8, 5).normalize();
  const polygons = [];
  scene.traverseVisible(object => {
    if (!object.isMesh) return;
    const geometry = object.geometry;
    const positions = geometry.attributes.position;
    const indices = geometry.index;
    const material = object.material;
    for (let i = 0; i < (indices?.count ?? positions.count); i += 3) {
      const vertices = [0, 1, 2].map(offset => new Vector3()
        .fromBufferAttribute(positions, indices ? indices.getX(i + offset) : i + offset)
        .applyMatrix4(object.matrixWorld));
      const normal = vertices[1].clone().sub(vertices[0]).cross(vertices[2].clone().sub(vertices[0]));
      if (normal.lengthSq() < 1e-14) continue;
      normal.normalize();
      if (normal.dot(towardCamera) <= 0) continue;
      const shade = 0.52 + 0.48 * Math.max(0, normal.dot(light));
      const gray = material.color.r * shade;
      const color = new Color().setRGB(gray, gray, gray).getHexString();
      polygons.push({ vertices, normal, color, opacity: material.transparent ? material.opacity : 1 });
    }
  });
  const ordered = [];
  function walk(node) {
    if (!node) return;
    const front = node.normal.dot(camera.position) > node.distance;
    walk(front ? node.back : node.front);
    ordered.push(...node.coplanar);
    walk(front ? node.front : node.back);
  }
  walk(partition(polygons));
  let paths = '', previousStyle = '', pendingPath = '';
  function flush() {
    if (pendingPath) paths += `<path ${previousStyle} d="${pendingPath}"/>`;
    pendingPath = '';
  }
  for (const polygon of ordered) {
    const points = polygon.vertices.map(v => {
      const p = v.clone().project(camera);
      return `${(p.x * width / 2).toFixed(1)},${(-p.y * height / 2).toFixed(1)}`;
    });
    const style = `fill="#${polygon.color}" stroke="#${polygon.color}"${polygon.opacity < 1 ? ` opacity="${polygon.opacity}"` : ''}`;
    if (style !== previousStyle) { flush(); previousStyle = style; }
    pendingPath += `M${points.join('L')}Z`;
  }
  flush();
  return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="${-width / 2} ${-height / 2} ${width} ${height}" width="${width}" height="${height}"><ellipse cx="0" cy="125" rx="300" ry="60" fill="#777" opacity=".12"/><g stroke-width=".6" stroke-linejoin="round">${paths}</g></svg>`;
}
