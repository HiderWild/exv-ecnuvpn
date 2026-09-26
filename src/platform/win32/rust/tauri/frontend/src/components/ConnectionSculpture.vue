<script setup lang="ts">
import { computed, inject, onBeforeUnmount, onMounted, ref, watch } from "vue";
import ConnectionStaticScene from "./ConnectionStaticScene.vue";
import { APPEARANCE_KEY } from "../product/appearance";
import type { ScenePresentation } from "../product/scene-motion";
import type { createConnectionSculpture } from "../product/connection-sculpture";

const props = defineProps<ScenePresentation>();
const appearance = inject(APPEARANCE_KEY, null);
const host = ref<HTMLElement | null>(null);
const canvas = ref<HTMLCanvasElement | null>(null);
const ready = ref(false);
const systemDark = ref(false);
const dark = computed(() => appearance?.state.value.theme === "dark" || ((appearance?.state.value.theme ?? "system") === "system" && systemDark.value));
// 动效只受产品设置管（appearance.motion / props.reduced），不读取系统级
// prefers-reduced-motion；系统「减少动态」开启不再停帧或切静态。
const reduced = computed(() => props.reduced || appearance?.state.value.motion === "reduced");
const accent = computed(() => ({ azure: "#388cf0", violet: "#8d77e6", jade: "#24a68e", amber: "#cd953a" }[appearance?.state.value.accent ?? "azure"]));
let sculpture: ReturnType<typeof createConnectionSculpture> | undefined;
let resizeObserver: ResizeObserver | undefined;
let frame = 0;
let lastFrame = 0;
let disposed = false;
let removeMediaListeners = () => {};

function stop() { cancelAnimationFrame(frame); frame = 0; lastFrame = 0; }
function draw(now: number) {
  frame = 0;
  // 工作循环限制为约 30 fps；稳定画面与隐藏窗口不持续占用 GPU。
  if (lastFrame && now - lastFrame < 32) { frame = requestAnimationFrame(draw); return; }
  const delta = lastFrame ? Math.min((now - lastFrame) / 1000, 0.1) : 1 / 30;
  lastFrame = now;
  if (sculpture?.render(delta)) frame = requestAnimationFrame(draw);
}
function refresh() {
  if (!sculpture) return;
  stop();
  sculpture.setPresentation({ state: props.state, stopping: props.stopping, paused: props.paused, reduced: reduced.value, dark: dark.value, accent: accent.value });
  frame = requestAnimationFrame(draw);
}
function loseContext(event: Event) {
  event.preventDefault();
  stop();
  ready.value = false;
  sculpture?.dispose();
  sculpture = undefined;
}
async function initialize() {
  if (!canvas.value || disposed) return;
  if (!("WebGLRenderingContext" in window) && !("WebGL2RenderingContext" in window)) return;
  try {
    const { createConnectionSculpture } = await import("../product/connection-sculpture");
    if (disposed || !canvas.value || sculpture) return;
    sculpture = createConnectionSculpture(canvas.value);
    if (host.value) sculpture.resize(host.value.clientWidth, host.value.clientHeight);
    ready.value = true;
    refresh();
  } catch {
    // 图形能力不可用时保留同构静态场景；连接操作不依赖 WebGL。
    sculpture?.dispose();
    sculpture = undefined;
    ready.value = false;
  }
}
watch([() => props.state, () => props.stopping, () => props.paused, dark, reduced, accent], refresh);
onMounted(() => {
  const darkMedia = window.matchMedia?.("(prefers-color-scheme: dark)");
  const updateMedia = () => { systemDark.value = darkMedia?.matches ?? false; };
  updateMedia();
  darkMedia?.addEventListener("change", updateMedia);
  removeMediaListeners = () => { darkMedia?.removeEventListener("change", updateMedia); };
  if (typeof ResizeObserver !== "undefined") {
    resizeObserver = new ResizeObserver(() => {
      if (host.value && sculpture) { sculpture.resize(host.value.clientWidth, host.value.clientHeight); refresh(); }
    });
    if (host.value) resizeObserver.observe(host.value);
  }
  void initialize();
});
onBeforeUnmount(() => { disposed = true; stop(); resizeObserver?.disconnect(); removeMediaListeners(); sculpture?.dispose(); });
</script>

<template>
  <div ref="host" class="sculpture" :class="{ 'sculpture--dark': dark }" :data-renderer="ready ? 'webgl' : 'static'" data-testid="connection-sculpture" aria-hidden="true">
    <canvas ref="canvas" :class="{ 'sculpture__canvas--ready': ready }" @webglcontextlost="loseContext" @webglcontextrestored="initialize" />
    <ConnectionStaticScene v-if="!ready" :state="state" :stopping="stopping" class="sculpture__fallback" />
  </div>
</template>

<style scoped>
.sculpture { position:relative; width:100%; height:100%; min-height:0; isolation:isolate; }
.sculpture::before { content:''; position:absolute; inset:12% 5% 8%; z-index:-1; background:radial-gradient(ellipse at 53% 55%, color-mix(in srgb, var(--accent) 6%, transparent), transparent 68%); pointer-events:none; }
.sculpture canvas,.sculpture__fallback { position:absolute; inset:0; width:100%; height:100%; display:block; }
.sculpture canvas { opacity:0; }
.sculpture .sculpture__canvas--ready { opacity:1; }
.sculpture__fallback { max-width:920px; margin:auto; }
</style>
