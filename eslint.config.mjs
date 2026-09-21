import js from "@eslint/js";
import globals from "globals";
import tseslint from "typescript-eslint";
import reactHooks from "eslint-plugin-react-hooks";
import importPlugin from "eslint-plugin-import";

export default tseslint.config(
  {
    ignores: [
      "**/dist/**",
      "**/node_modules/**",
      "**/target/**",
      "**/.turbo/**",
      "apps/desktop/src-tauri/gen/**",
      "tools/project-graph/output/**",
    ],
  },

  js.configs.recommended,
  ...tseslint.configs.recommendedTypeChecked,

  {
    languageOptions: {
      parserOptions: {
        projectService: true,
        tsconfigRootDir: import.meta.dirname,
      },
    },
  },

  /* Renderer and packages. */
  {
    files: ["apps/**/*.{ts,tsx}", "packages/**/*.{ts,tsx}"],
    languageOptions: {
      globals: { ...globals.browser },
    },
    plugins: {
      "react-hooks": reactHooks,
      import: importPlugin,
    },
    settings: {
      "import/resolver": {
        typescript: {
          project: ["apps/*/tsconfig.json", "packages/*/tsconfig.json"],
          // One tsconfig per workspace package is the layout we want. The
          // resolver's "use references instead" advice is about speed, and at
          // this size the cure (composite project references) costs more than
          // it saves. Revisit if lint time becomes a CI problem.
          noWarnOnMultipleProjects: true,
        },
      },
    },
    rules: {
      ...reactHooks.configs.recommended.rules,

      /* The acceptance criterion in #10: removing a dependency from
         package.json that code still imports must make `pnpm verify` fail.
         This rule reads package.json directly, so it fails on the declaration
         rather than waiting for a reinstall to remove the symlink. */
      "import/no-extraneous-dependencies": [
        "error",
        {
          devDependencies: [
            "**/*.test.{ts,tsx}",
            "**/*.config.{ts,mts,mjs}",
            "**/vitest.setup.ts",
          ],
          optionalDependencies: false,
          peerDependencies: false,
        },
      ],
      "import/no-cycle": "error",

      "@typescript-eslint/consistent-type-imports": [
        "error",
        { prefer: "type-imports", fixStyle: "inline-type-imports" },
      ],
      "@typescript-eslint/no-unused-vars": [
        "error",
        { argsIgnorePattern: "^_", varsIgnorePattern: "^_" },
      ],

      /* CLAUDE.md section 8: no debug logging left behind. */
      "no-console": ["error", { allow: ["warn", "error"] }],
    },
  },

  /* Node-side tooling. */
  {
    files: [
      "tools/**/*.mjs",
      "*.config.{ts,mts,mjs}",
      "**/vite.config.ts",
      ".dependency-cruiser.cjs",
    ],
    languageOptions: {
      globals: { ...globals.node },
    },
    rules: {
      "no-console": "off",
    },
  },

  /* Plain .mjs and .cjs tooling is not in a tsconfig; do not type-check it.
     Without this the project service rejects the file outright — `was not
     found by the project service` — which fails the lint gate on a config file
     that is not TypeScript and never will be. */
  {
    files: ["**/*.mjs", "**/*.cjs"],
    ...tseslint.configs.disableTypeChecked,
  },
);
