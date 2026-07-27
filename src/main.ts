import { createApp } from "vue";
import { createPinia } from "pinia";
import App from "./App.vue";
import "./assets/styles/theme.css";
import "./assets/styles/components.css";
import { monoFontFamily } from "./utils/platformFonts";

// Keep all code/file/log views on the same platform-specific monospace font
// as xterm. Vite replaces TAURI_ENV_PLATFORM at package build time.
document.documentElement.style.setProperty("--font-mono", monoFontFamily);

const app = createApp(App);
app.use(createPinia());
app.mount("#app");
