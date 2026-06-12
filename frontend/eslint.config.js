// Flat ESLint config for the frontend. Deliberately NOT the type-checked
// typescript-eslint variants — `tsc --noEmit` (npm run type-check) already
// covers type errors, and keeping lint syntax-only keeps it fast.
import js from "@eslint/js";
import tseslint from "typescript-eslint";
import lit from "eslint-plugin-lit";
import prettier from "eslint-config-prettier";
import { defineConfig, globalIgnores } from "eslint/config";

export default defineConfig([
  globalIgnores(["dist", "node_modules", "coverage"]),
  {
    files: ["**/*.ts"],
    extends: [
      js.configs.recommended,
      tseslint.configs.recommended,
      lit.configs["flat/recommended"],
      // Last: turns off anything that would fight Prettier.
      prettier,
    ],
    rules: {
      // The daemon config object is passed around as `any` by design for now
      // (its shape is owned by the Rust side); flag it, don't fail on it.
      "@typescript-eslint/no-explicit-any": "warn",
      // Unused code is a real error, but `_`-prefixed params/vars are the
      // documented way to say "intentionally unused" (callbacks, catch arms).
      "@typescript-eslint/no-unused-vars": [
        "error",
        {
          argsIgnorePattern: "^_",
          varsIgnorePattern: "^_",
          caughtErrorsIgnorePattern: "^_",
        },
      ],
    },
  },
]);
