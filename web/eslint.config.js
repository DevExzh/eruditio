import js from "@eslint/js";
import ts from "typescript-eslint";
import svelte from "eslint-plugin-svelte";
import globals from "globals";

export default ts.config(
  // --- Global ignores ---
  {
    ignores: ["node_modules/", "dist/", "src/lib/wasm/"],
  },

  // --- Base JS recommended rules ---
  js.configs.recommended,

  // --- TypeScript recommended rules ---
  ...ts.configs.recommended,

  // --- Svelte recommended rules (flat config) ---
  ...svelte.configs["flat/recommended"],

  // --- Browser globals for all source files ---
  {
    languageOptions: {
      globals: {
        ...globals.browser,
      },
    },
  },

  // --- Svelte file overrides: use TypeScript parser for <script> blocks ---
  {
    files: ["**/*.svelte"],
    languageOptions: {
      parserOptions: {
        parser: ts.parser,
      },
    },
  },

  // --- Worker file: add Web Worker globals ---
  {
    files: ["src/lib/worker.ts"],
    languageOptions: {
      globals: {
        ...globals.worker,
      },
    },
  },

  // --- Test files: add vitest globals ---
  {
    files: ["src/**/*.test.ts"],
    languageOptions: {
      globals: {
        ...globals.node,
      },
    },
  },

  // --- Rule overrides ---
  {
    rules: {
      // Allow unused vars prefixed with _ (common pattern for intentional ignores)
      "@typescript-eslint/no-unused-vars": [
        "error",
        {
          argsIgnorePattern: "^_",
          varsIgnorePattern: "^_",
        },
      ],
      // The WASM bindings use `any` types in their API surface
      "@typescript-eslint/no-explicit-any": "off",
    },
  },
);
