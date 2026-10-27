import js from "@eslint/js";
import { defineConfig, globalIgnores } from "eslint/config";
import reactHooks from "eslint-plugin-react-hooks";
import tseslint from "typescript-eslint";

export default defineConfig([
  globalIgnores(["dist"]),
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
    // The vendored loro-prosemirror (src/editor/loro-binding, see VENDORED.md) stays as
    // close to upstream as possible so that diffs against it remain reviewable; these
    // rules flag upstream's style (untyped `toJSON()` values, redundant casts, unused
    // `_` parameters), not defects. Everything else applies to it as to the rest.
    files: ["src/editor/loro-binding/**/*.ts"],
    rules: {
      "@typescript-eslint/no-unsafe-argument": "off",
      "@typescript-eslint/no-unsafe-assignment": "off",
      "@typescript-eslint/no-unnecessary-type-assertion": "off",
      "@typescript-eslint/no-unused-vars": ["error", { argsIgnorePattern: "^_", varsIgnorePattern: "^_" }],
      "prefer-const": "off",
    },
  },
  {
    files: ["**/*.js"],
    extends: [tseslint.configs.disableTypeChecked],
  },
]);
