import { createRouter, createWebHashHistory } from 'vue-router'
import { routeRecords } from '../windows'

const router = createRouter({
  history: createWebHashHistory(),
  routes: routeRecords,
})

export default router
