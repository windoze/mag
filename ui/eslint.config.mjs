import js from "@eslint/js";
import prettier from "eslint-config-prettier";
import tseslint from "typescript-eslint";

export default tseslint.config(
  {
    ignores: [
      "**/coverage/**",
      "**/dist/**",
      "**/node_modules/**",
      "**/storybook-static/**",
      "packages/protocol/src/generated/**"
    ]
  },
  js.configs.recommended,
  ...tseslint.configs.recommended,
  {
    files: ["**/*.{ts,tsx}"]
  },
  prettier
);
