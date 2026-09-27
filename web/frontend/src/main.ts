import { createApp } from 'vue'
import { createPinia } from 'pinia'
import { createRouter, createWebHistory } from 'vue-router'
import App from './App.vue'
import './style.css'

const router = createRouter({
  history: createWebHistory(),
  routes: [
    { path: '/', redirect: '/overview' },
    { path: '/login', component: () => import('./pages/OverviewPage.vue') },
    { path: '/overview', component: () => import('./pages/OverviewPage.vue') },
    { path: '/hosts', component: () => import('./pages/HostsPage.vue') },
    { path: '/cluster', component: () => import('./pages/ClusterPage.vue') },
    { path: '/metrics', component: () => import('./pages/MetricsPage.vue') },
    { path: '/processes', component: () => import('./pages/ProcessesPage.vue') },
    { path: '/scores', component: () => import('./pages/ScoresPage.vue') },
    { path: '/profiles', component: () => import('./pages/ProfilesPage.vue') },
    { path: '/alerts', component: () => import('./pages/AlertsPage.vue') },
    { path: '/users', component: () => import('./pages/UsersPage.vue') }
  ]
})

createApp(App)
  .use(createPinia())
  .use(router)
  .mount('#app')
