<script setup lang="ts">
import { RouterView } from 'vue-router'
import NavBar from './components/NavBar.vue'
import ToastStack from './components/Toast.vue'
</script>

<template>
  <div class="min-h-screen bg-bg text-foreground font-sans lg:flex">
    <NavBar />
    <main class="min-w-0 flex-1 lg:pl-72">
      <div class="mx-auto w-full max-w-6xl px-4 pb-8 pt-4 lg:px-8 lg:py-8">
        <RouterView />
      </div>
    </main>
    <ToastStack />
  </div>
</template>