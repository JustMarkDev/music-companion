import { defineConfig } from "vite-plus";

export default defineConfig({
  clearScreen: false,
  fmt: {
    // `src-tauri/vendor` is the pinned MediaRemote adapter submodule, which must
    // stay byte-identical to upstream.
    ignorePatterns: ["src-tauri/gen/**", "src-tauri/vendor/**"],
  },
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
        command: "tauri build",
        cache: false,
      },
      "tauri:build:windows": {
        command: "tauri build --target x86_64-pc-windows-msvc",
        cache: false,
      },
      "tauri:build:macos": {
        command: "tauri build --target universal-apple-darwin",
        cache: false,
      },
    },
  },
});
