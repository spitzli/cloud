import js from "@eslint/js";
import { defineConfig } from "eslint/config";
import tseslint from "typescript-eslint";
import reactHooks from "eslint-plugin-react-hooks";
import globals from "globals";

export default defineConfig(
  {
    ignores: ["dist/**"],
  },
  {
    linterOptions: {
      noInlineConfig: true,
      reportUnusedDisableDirectives: "error",
    },
  },
  {
    files: ["**/*.{ts,tsx}"],
    extends: [
      js.configs.recommended,
      tseslint.configs.strictTypeChecked,
      tseslint.configs.stylisticTypeChecked,
      reactHooks.configs.flat.recommended,
    ],
    languageOptions: {
      parserOptions: {
        projectService: true,
        tsconfigRootDir: import.meta.dirname,
      },
      globals: { ...globals.browser, ...globals.node },
    },
    rules: {
      "@typescript-eslint/ban-ts-comment": [
        "error",
        {
          "ts-expect-error": true,
          "ts-ignore": true,
          "ts-nocheck": true,
          "ts-check": false,
        },
      ],
      "@typescript-eslint/switch-exhaustiveness-check": "error",
      "@typescript-eslint/consistent-type-imports": "error",
      "no-else-return": ["error", { allowElseIf: false }],
      "no-restricted-syntax": [
        "error",
        {
          selector: 'IfStatement[alternate.type="IfStatement"]',
          message:
            "Use guard clauses or an exhaustive switch instead of an else-if chain.",
        },
      ],
      "max-depth": ["error", 3],
      complexity: ["error", { max: 20, variant: "modified" }],
    },
  },
  {
    // JSX counts conditional rendering and optional props as branches, too.
    files: ["**/*.tsx"],
    rules: { complexity: ["error", { max: 40, variant: "modified" }] },
  },
  {
    files: ["**/*.mjs"],
    extends: [js.configs.recommended],
    languageOptions: { globals: { ...globals.browser, ...globals.node } },
    rules: {
      "no-else-return": ["error", { allowElseIf: false }],
      "max-depth": ["error", 3],
    },
  },
);
