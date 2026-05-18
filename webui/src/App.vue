<script setup lang="ts">
import { RouterView } from 'vue-router'
import NavBar from './components/NavBar.vue'
import ToastStack from './components/Toast.vue'
</script>

<template>
  <div class="min-h-screen bg-bg text-foreground font-sans flex">
    <NavBar />
    <main class="flex-1 min-w-0 overflow-y-auto h-screen px-6 py-6">
      <RouterView />
    </main>
    <ToastStack />
  </div>
</template>