import js from "@eslint/js";
import { defineConfig, globalIgnores } from "eslint/config";
import reactHooks from "eslint-plugin-react-hooks";
import tseslint from "typescript-eslint";

export default defineConfig([
  // `src/wasm` is the output of `wasm-bindgen` (kb-wasm's JavaScript glue), made by the build and never edited;
  // `scripts` are Node.js scripts (the build of that glue), outside the browser project the rules type-check.
  globalIgnores(["dist", "src/wasm", "scripts"]),
  js.configs.recommended,
  tseslint.configs.recommendedTypeChecked,
  reactHooks.configs.flat.recommended,
  {
    languageOptions: {
      parserOptions: { projectService: true, tsconfigRootDir: import.meta.dirname },
    },
    rules: {
      // verbatimModuleSyntax: type-only imports must be erased completely.
      "@typescript-eslint/consistent-type-imports": "error",
      "@typescript-eslint/no-import-type-side-effects": "error",
    },
  },
  {
    files: ["**/*.js"],
    extends: [tseslint.configs.disableTypeChecked],
  },
]);
