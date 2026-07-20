import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

export default defineConfig({
  plugins: [react()],
  build: {
    emptyOutDir: false,
    outDir: "dist"
  },
  test: {
    environment: "jsdom"
  }
});
