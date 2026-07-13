import { resolve } from "node:path";
import { defineConfig } from "vite";

export default defineConfig({
  define: {
    "process.env": "process.env"
  },
  build: {
    emptyOutDir: true,
    minify: false,
    outDir: "dist-electron",
    rollupOptions: {
      external: ["electron", "node:path", "node:url"],
      input: {
        "approval-preload": resolve(__dirname, "src/preload/approval.ts"),
        main: resolve(__dirname, "src/main/electronMain.ts"),
        preload: resolve(__dirname, "src/preload/index.ts")
      },
      output: {
        chunkFileNames: "chunks/[name]-[hash].cjs",
        entryFileNames: "[name].cjs",
        format: "cjs"
      }
    },
    target: "node20"
  }
});
