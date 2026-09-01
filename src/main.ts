import { createApp } from "vue";
import { createPinia } from "pinia";
import App from "./App.vue";
import "./assets/styles/theme.css";
import "./assets/styles/components.css";
import { monoFontFamily } from "./utils/platformFonts";
import { beginStartupMeasure, markStartup } from "./utils/startupMetrics";

// Keep all code/file/log views on the same platform-specific monospace font
// as xterm. Vite replaces TAURI_ENV_PLATFORM at package build time.
markStartup("frontend-entry");
const finishMountMeasure = beginStartupMeasure("vue-create-and-mount");
document.documentElement.style.setProperty("--font-mono", monoFontFamily);

const app = createApp(App);
app.use(createPinia());
app.mount("#app");
finishMountMeasure();
