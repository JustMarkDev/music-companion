import { defineConfig } from "vite-plus";

export default defineConfig({
  clearScreen: false,
  server: {
    port: 1421,
    strictPort: true,
    host: "127.0.0.1",
    watch: {
      ignored: ["**/src-tauri/target/**"],
    },
  },
  preview: {
    port: 1421,
    strictPort: true,
    host: "127.0.0.1",
  },
  envPrefix: ["VITE_", "TAURI_"],
  build: {
    target: "es2020",
  },
  run: {
    tasks: {
      "tauri:dev": {
        command: "tauri dev",
        cache: false,
      },
      "tauri:build": {
        command: "tauri build --target x86_64-pc-windows-msvc",
        cache: false,
      },
    },
  },
});
