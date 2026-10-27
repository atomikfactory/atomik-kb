import js from "@eslint/js";
import { defineConfig, globalIgnores } from "eslint/config";
import reactHooks from "eslint-plugin-react-hooks";
import tseslint from "typescript-eslint";

export default defineConfig([
  globalIgnores(["dist", "src-tauri"]),
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
