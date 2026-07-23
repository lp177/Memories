import { createRouter, createWebHashHistory } from "vue-router";
import { sessionToken } from "./api.js";
import AdminView from "./views/AdminView.vue";
import ExplorerView from "./views/ExplorerView.vue";
import LoginView from "./views/LoginView.vue";
import ResetView from "./views/ResetView.vue";
import SettingsView from "./views/SettingsView.vue";

const router = createRouter({
  history: createWebHashHistory(),
  routes: [
    { path: "/login", name: "login", component: LoginView },
    { path: "/reset", name: "reset", component: ResetView },
    // The explorer path mirrors the memory tree: /#/x/project/user/...
    { path: "/x/:path(.*)*", name: "explorer", component: ExplorerView },
    { path: "/settings", name: "settings", component: SettingsView },
    { path: "/admin", name: "admin", component: AdminView },
    { path: "/", redirect: "/x/" },
    { path: "/:catchall(.*)", redirect: "/x/" },
  ],
});

router.beforeEach((to) => {
  const open = to.name === "login" || to.name === "reset";
  if (!open && !sessionToken()) {
    return { name: "login" };
  }
  return true;
});

export default router;
