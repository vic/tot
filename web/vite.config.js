import { defineConfig } from "vite";

// SharedArrayBuffer needs cross-origin isolation. A static host that cannot set these headers needs a service worker that adds them.
const isolation = { "Cross-Origin-Opener-Policy": "same-origin", "Cross-Origin-Embedder-Policy": "require-corp" };

export default defineConfig({
  server: { headers: isolation, fs: { allow: [".."] } },
  preview: { headers: isolation },
  worker: { format: "es" },
});
