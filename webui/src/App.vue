<script setup lang="ts">
import { RouterView } from 'vue-router'
import NavBar from './components/NavBar.vue'
import ToastStack from './components/Toast.vue'
</script>

<template>
  <div class="h-screen overflow-hidden bg-bg text-foreground font-sans lg:flex">
    <NavBar />
    <main class="min-w-0 flex-1 overflow-hidden lg:pl-36">
      <div class="mx-auto h-full w-full max-w-5xl overflow-hidden px-4 pb-6 pt-4 lg:px-6 lg:py-6">
        <RouterView />
      </div>
    </main>
    <ToastStack />
  </div>
</template>
