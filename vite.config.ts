import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import { readFileSync } from 'fs';
const pkg = JSON.parse(readFileSync('./package.json', 'utf-8'));

// https://vitejs.dev/config/
export default defineConfig({
  plugins: [react()],
  define: {
    __APP_VERSION__: JSON.stringify(pkg.version),
  },

  // Tauri expects a fixed port, fail if it's already in use.
  //
  // 5173 and not 1420: Windows reserves whole blocks of ports for Hyper-V
  // (which is what WSL runs on), and 1420 sits inside one of them — 1386-1485
  // on this machine. Nothing is listening there; the OS simply refuses the
  // bind, and Vite dies with `EACCES: permission denied ::1:1420`, which
  // reads like a firewall problem and is not one. 5173 is outside every
  // reserved range and is Vite's own default.
  //
  // To check the ranges on a given machine:
  //   netsh interface ipv4 show excludedportrange protocol=tcp
  server: {
    port: 5173,
    strictPort: true,
    watch: {
      ignored: ["**/src-tauri/**"],
    },
  },
  // to make use of `TAURI_DEBUG` and other env variables
  envPrefix: ["VITE_", "TAURI_"],
  build: {
    // Tauri supports es2021
    target: process.env.TAURI_PLATFORM == "windows" ? "chrome105" : "safari13",
    // don't minify for debug builds
    minify: !process.env.TAURI_DEBUG ? "esbuild" : false,
    // produce sourcemaps for debug builds
    sourcemap: !!process.env.TAURI_DEBUG,
    rollupOptions: {
      output: {
        inlineDynamicImports: true,
      },
    },
  },
});
