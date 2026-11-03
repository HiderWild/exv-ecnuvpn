<script setup lang="ts">
import type { ProductConnectionAction } from "../product/connection-action";
defineProps<{ action: ProductConnectionAction }>();
</script>

<template>
  <svg viewBox="0 0 24 24" aria-hidden="true">
    <path v-if="action.label === '断开'" d="M12 3v9M6.4 5.9a9 9 0 1 0 11.2 0" />
    <path v-else-if="action.label === '取消'" d="m6 6 12 12M18 6 6 18" />
    <path v-else d="M4 12h16m-6-6 6 6-6 6" />
  </svg>
  <span>{{ action.label === '断开' ? '断开连接' : action.label === '取消' ? '取消连接' : action.label }}</span>
</template>

<style scoped>
svg { width:17px; height:17px; flex:none; fill:none; stroke:currentColor; stroke-width:2.3; stroke-linecap:round; stroke-linejoin:round; }
</style>
