<script setup lang="ts">
import { RouterView } from 'vue-router'
import NavBar from './components/NavBar.vue'
import ToastStack from './components/Toast.vue'
</script>

<template>
  <div class="min-h-screen bg-bg text-foreground font-sans">
    <NavBar />
    <main class="max-w-5xl mx-auto px-4 pb-8">
      <RouterView />
    </main>
    <ToastStack />
  </div>
</template>
