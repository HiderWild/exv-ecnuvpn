<script setup lang="ts">
import { computed, inject } from "vue";
import ConnectionSidebar from "./ConnectionSidebar.vue";
import { PRODUCT_RUNTIME_KEY } from "../product/runtime";

const items = [{ key: "connect", label: "连接" }, { key: "logs", label: "日志" }, { key: "settings", label: "设置" }, { key: "about", label: "关于 EXV" }];
const props = defineProps<{ currentPage: string }>();
const emit = defineEmits<{ navigate: [page: "connect" | "logs" | "settings" | "about"] }>();
function navigate(page: string) { emit("navigate", page as "connect" | "logs" | "settings" | "about"); }
const runtime = inject(PRODUCT_RUNTIME_KEY, null);
const state = computed(() => runtime?.state.value ?? null);
</script>
<template>
  <aside class="product-rail" data-testid="product-rail">
    <div class="product-brand" aria-label="EXV for ECNU"><img src="../assets/exv-logo.svg" alt="产品 Logo" /><div><strong>EXV</strong><span>for ECNU</span></div></div>
    <nav aria-label="主导航"><button v-for="item in items" :key="item.key" type="button" :aria-label="item.label" :title="item.label" :aria-current="props.currentPage === item.key ? 'page' : undefined" :class="{ active: currentPage === item.key }" @click="navigate(item.key)"><svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="1.6" aria-hidden="true"><path v-if="item.key === 'connect'" d="M12 8V4m0 4-6 5m6-5 6 5M4 14h4v5H4zm12 0h4v5h-4zM10 2h4v4h-4z"/><path v-else-if="item.key === 'logs'" d="M5 3h14v18H5zM8 7h8M8 11h8M8 15h5"/><path v-else-if="item.key === 'settings'" d="M4 7h16M4 17h16M9 4v6m6 4v6"/><g v-else><circle cx="12" cy="12" r="9"/><path d="M12 11v6m0-10v1"/></g></svg><span class="product-rail__nav-text">{{ item.label }}</span></button></nav>
    <ConnectionSidebar v-if="state" :state="state" class="product-rail__connection" />
  </aside>
</template>
