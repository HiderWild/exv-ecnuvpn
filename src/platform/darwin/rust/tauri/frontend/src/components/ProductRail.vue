<script setup lang="ts">
import { computed, inject } from "vue";
import ConnectionSidebar from "./ConnectionSidebar.vue";
import { PRODUCT_RUNTIME_KEY } from "../product/runtime";

const items = [{ key: "connect", label: "连接" }, { key: "logs", label: "日志" }, { key: "settings", label: "设置" }, { key: "about", label: "关于" }];
const props = defineProps<{ currentPage: string }>();
const emit = defineEmits<{ navigate: [page: "connect" | "logs" | "settings" | "about"] }>();
function navigate(page: string) { emit("navigate", page as "connect" | "logs" | "settings" | "about"); }
const runtime = inject(PRODUCT_RUNTIME_KEY, null);
const state = computed(() => runtime?.state.value ?? null);
</script>
<template>
  <aside class="product-rail" data-testid="product-rail">
    <div class="product-brand" aria-label="EXV for ECNU"><img src="../assets/exv-logo.svg" alt="产品 Logo" /><div><strong>EXV</strong><span>for ECNU</span></div></div>
    <nav aria-label="主导航"><button v-for="item in items" :key="item.key" type="button" :aria-current="props.currentPage === item.key ? 'page' : undefined" :class="{ active: currentPage === item.key }" @click="navigate(item.key)"><svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="1.6" aria-hidden="true"><path v-if="item.key === 'connect'" d="M9 7V3m6 4V3M7 7h10v4a5 5 0 0 1-5 5v5m-5-14v4a5 5 0 0 0 5 5"/><path v-else-if="item.key === 'logs'" d="M5 3h14v18H5zM8 7h8M8 11h8M8 15h5"/><path v-else-if="item.key === 'settings'" d="M4 7h16M4 17h16M9 4v6m6 4v6"/><g v-else><circle cx="12" cy="12" r="9"/><path d="M12 11v6m0-10v1"/></g></svg>{{ item.label }}</button></nav>
    <ConnectionSidebar v-if="state" :state="state" class="product-rail__connection" />
  </aside>
</template>
