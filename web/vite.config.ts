/// <reference types="vitest/config" />
import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import { svelteTesting } from "@testing-library/svelte/vite";

export default defineConfig({
  plugins: [svelte(), svelteTesting()],
  build: { target: "esnext" },
  worker: { format: "es" },
  test: {
    environment: "jsdom",
    include: ["src/**/*.test.ts"],
  },
});
