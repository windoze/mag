import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

export default defineConfig({
  plugins: [react()],
  build: {
    lib: {
      entry: "src/index.ts",
      formats: ["es"],
      name: "MagUi"
    },
    rollupOptions: {
      external: ["react", "react-dom", "react/jsx-runtime"]
    }
  },
  test: {
    environment: "jsdom"
  }
});
