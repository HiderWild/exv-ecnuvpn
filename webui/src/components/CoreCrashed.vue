<template>
  <div class="core-crashed-overlay">
    <div class="core-crashed-card">
      <div class="icon">&#9888;</div>
      <h2>内核意外退出</h2>
      <p>VPN 核心进程已终止，VPN 连接可能已中断。</p>
      <p class="exit-code" v-if="exitCode !== null">退出码: {{ exitCode }}</p>
      <div class="actions">
        <button class="btn-primary" @click="$emit('restart')">重启内核</button>
        <button class="btn-secondary" @click="$emit('quit')">退出程序</button>
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
defineProps<{ exitCode: number | null }>()
defineEmits<{ restart: []; quit: [] }>()
</script>

<style scoped>
.core-crashed-overlay {
  position: fixed; inset: 0; z-index: 9999;
  display: flex; align-items: center; justify-content: center;
  background: rgba(0, 0, 0, 0.85);
}
.core-crashed-card {
  background: #1f2937; border-radius: 12px; padding: 40px;
  text-align: center; max-width: 420px; color: #f3f4f6;
}
.icon { font-size: 48px; margin-bottom: 16px; }
h2 { font-size: 20px; margin-bottom: 12px; }
p { font-size: 14px; color: #9ca3af; margin-bottom: 8px; }
.exit-code { font-family: monospace; color: #f87171; }
.actions { margin-top: 24px; display: flex; gap: 12px; justify-content: center; }
.btn-primary {
  background: #3b82f6; color: white; border: none;
  padding: 10px 24px; border-radius: 8px; cursor: pointer; font-size: 14px;
}
.btn-primary:hover { background: #2563eb; }
.btn-secondary {
  background: #374151; color: #d1d5db; border: 1px solid #4b5563;
  padding: 10px 24px; border-radius: 8px; cursor: pointer; font-size: 14px;
}
.btn-secondary:hover { background: #4b5563; }
</style>
