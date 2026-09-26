<script setup lang="ts">
import { computed } from "vue";
import type { ScenePresentation } from "../product/scene-motion";
import idle from "../assets/connection-vectors/idle.svg";
import working from "../assets/connection-vectors/working.svg";
import connected from "../assets/connection-vectors/connected.svg";
const props = defineProps<ScenePresentation>();
const displayState = computed(() => props.stopping ? "idle" : props.state);
// 中性灰矢量不随强调色变化；深浅主题复用同一份几何和灰阶。
const sources = { idle, working, connected };
const source = computed(() => sources[displayState.value]);
</script>

<template>
  <img class="static-scene" data-testid="connection-static-scene" :data-state="displayState" :src="source" alt="" aria-hidden="true" width="1200" height="800" draggable="false">
</template>

<style scoped>
.static-scene { display:block; width:100%; height:100%; min-height:0; object-fit:contain; object-position:center; margin:auto; }
</style>
