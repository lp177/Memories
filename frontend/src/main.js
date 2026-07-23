// Application entry point for Memories.
import { createApp } from "vue";
import App from "./App.vue";
import router from "./router.js";
import { initRipple } from "./ripple.js";
import "./style.css";

createApp(App).use(router).mount("#app");
initRipple();
