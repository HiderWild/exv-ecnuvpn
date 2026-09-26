import { createServer } from "node:http";
import { mkdir, writeFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { randomBytes } from "node:crypto";

// 开发期导出，运行产品时只读取生成的图片。先启动 npm run dev -- --port 1430。
const output = fileURLToPath(new URL("../src/assets/connection-stills/", import.meta.url));
const token = randomBytes(16).toString("hex");
const names = new Set();
for (const theme of ["light", "dark"]) for (const state of ["idle", "working", "connected"]) for (const accent of ["azure", "violet", "jade", "amber"]) names.add(`${theme}-${state}-${accent}.webp`);
const remaining = new Set(names);
await mkdir(output, { recursive: true });
const html = `<!doctype html><html lang="zh"><meta charset="utf-8"><title>EXV 三态静态素材导出</title><body><h1>正在导出三维场景</h1><pre id="status"></pre><canvas></canvas><script type="module">
import { createConnectionSculpture } from 'http://127.0.0.1:1430/src/product/connection-sculpture.ts';
const canvas = document.querySelector('canvas');
const scene = createConnectionSculpture(canvas);
try {
 scene.resize(1200,800);
 for (const theme of ['light','dark']) for (const state of ['idle','working','connected']) for (const [accent,color] of Object.entries({azure:'#388cf0',violet:'#8d77e6',jade:'#24a68e',amber:'#cd953a'})) {
  scene.setPresentation({state,dark:theme==='dark',accent:color,reduced:true});
  scene.render(0);
  // 必须在 render 同一任务内复制，避免 WebGL 默认清空绘图缓冲。
  const copy = document.createElement('canvas'); copy.width=canvas.width; copy.height=canvas.height;
  copy.getContext('2d').drawImage(canvas,0,0);
  const blob = await new Promise(resolve=>copy.toBlob(resolve,'image/webp',.92));
  const name = theme+'-'+state+'-'+accent+'.webp';
  const result = await fetch('/export/'+name,{method:'POST',headers:{'X-Export-Token':'${token}'},body:blob});
  if(!result.ok) throw new Error(name+' 导出失败');
  document.querySelector('#status').textContent += name+String.fromCharCode(10);
 }
 document.querySelector('h1').textContent='24 张静态素材导出完成';
} catch(error) { document.querySelector('h1').textContent='导出失败';document.querySelector('#status').textContent+=String(error); }
finally { scene.dispose(); }
</script></body></html>`;
const server = createServer(async (req,res) => {
 try {
  if(req.method==='GET' && req.url==='/') { res.setHeader('Content-Type','text/html; charset=utf-8');res.end(html);return; }
  const name = req.url?.replace('/export/','');
  if(req.method!=='POST' || !req.url?.startsWith('/export/') || !names.has(name) || req.headers['x-export-token']!==token) {res.writeHead(404).end();return;}
  const chunks=[];let size=0;
  for await(const chunk of req) {size+=chunk.length;if(size>4_000_000) throw new Error('图片过大');chunks.push(chunk);}
  const data=Buffer.concat(chunks);
  if(data.toString('ascii',0,4)!=='RIFF'||data.toString('ascii',8,12)!=='WEBP') throw new Error('仅接受 WebP');
  await writeFile(new URL('../src/assets/connection-stills/'+name,import.meta.url),data);
  remaining.delete(name);res.end('ok');console.log(name, data.length);
  if(!remaining.size) server.close();
 } catch(error) {res.writeHead(500).end(String(error));}
});
server.listen(1431,'127.0.0.1',()=>console.log('打开 http://127.0.0.1:1431/ 导出静态素材；完成后服务自动退出。'));
