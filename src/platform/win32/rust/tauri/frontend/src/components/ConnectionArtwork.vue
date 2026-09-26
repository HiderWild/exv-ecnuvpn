<script setup lang="ts">
import { computed, defineAsyncComponent, defineComponent, h, inject } from "vue";
import { APPEARANCE_KEY } from "../product/appearance";
import type { ScenePresentation } from "../product/scene-motion";
import ConnectionStaticScene from "./ConnectionStaticScene.vue";

const props = defineProps<ScenePresentation>();
const appearance = inject(APPEARANCE_KEY, null);
// 动效只受产品设置管（appearance.motion / props.reduced），不读取系统级
// prefers-reduced-motion：三维雕塑即使系统「减少动态」开启也照常渲染。
const reduced = computed(() => props.reduced || appearance?.state.value.motion === "reduced");
// 加载门位于三维组件之外：初次静态模式不导入三维组件和 Three.js。
const Fallback = defineComponent({ setup: () => () => h(ConnectionStaticScene, { state: props.state, stopping: props.stopping }) });
const Sculpture = defineAsyncComponent({ loader: () => import("./ConnectionSculpture.vue"), loadingComponent: ConnectionStaticScene, errorComponent: Fallback, delay: 0 });
</script>

<template>
  <ConnectionStaticScene v-if="reduced" :state="state" :stopping="stopping" />
  <Sculpture v-else :state="state" :paused="paused" :stopping="stopping" />
</template>
