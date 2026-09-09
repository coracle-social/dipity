import js from "@eslint/js"
import ts from "typescript-eslint"
import svelte from "eslint-plugin-svelte"
import tailwind from "eslint-plugin-better-tailwindcss"
import prettier from "eslint-config-prettier"
import globals from "globals"
import svelteConfig from "./svelte.config.js"

// The conventions in docs/ui.md that a machine can check, rather than a reviewer.

/** Utilities that hard-code a value the design system already owns. */
const RESTRICTED_CLASSES = [
  {
    pattern:
      "^-?(bg|text|border|ring|outline|fill|stroke|shadow|divide|from|via|to|accent|caret|decoration|placeholder)-(inherit|current|transparent|black|white|slate|gray|zinc|neutral|stone|red|orange|amber|yellow|lime|green|emerald|teal|cyan|sky|blue|indigo|violet|purple|fuchsia|pink|rose)(-\\d{2,3})?(/\\d+)?$",
    message:
      "Use a semantic token (bg-card, text-muted-foreground, border-border, …) rather than a palette colour. Both themes are defined in src/app.css; a raw colour is only correct in one of them.",
  },
  {
    pattern: "^ease-(linear|in|out|in-out)$",
    message: "Use `ease-clay` for entrances and settles, `ease-exit` for dismissals.",
  },
]

// A bracket group followed by `:` selects something; one that is not invents a value.
const ARBITRARY_VALUE = {
  pattern: "\\[[^\\]]*\\](?![^\\s]*:)",
  message:
    "No arbitrary values outside src/lib/components. Compose an existing component, or add the value to the theme in src/app.css so it has a name. See docs/ui.md#the-composition-rule.",
}

/** Runes are compiler syntax, and the compiler only runs on `.svelte`. */
const RUNES_ARE_COMPONENT_ONLY = {
  selector: "Identifier[name=/^\\$(state|derived|effect|props|bindable|inspect|host)$/]",
  message:
    "Runes belong in .svelte components. Shared reactive state goes in a welshman store, which works in both plain modules and components. See docs/ui.md#runes-stay-in-components.",
}

export default ts.config(
  {
    ignores: [
      "dist/",
      "dist-ssr/",
      "core/",
      "ios/",
      "android/",
      "ref/",
      ".local/",
      "node_modules/",
    ],
  },

  js.configs.recommended,
  ts.configs.recommended,
  svelte.configs.recommended,
  prettier,
  svelte.configs.prettier,

  {
    languageOptions: {
      globals: {...globals.browser, ...globals.node},
    },
    rules: {
      "no-undef": "off", // TypeScript already does this.
      "@typescript-eslint/no-unused-vars": [
        "error",
        {argsIgnorePattern: "^_", varsIgnorePattern: "^_"},
      ],
    },
  },

  {
    files: ["**/*.svelte", "**/*.svelte.ts", "**/*.svelte.js"],
    languageOptions: {
      parserOptions: {
        projectService: true,
        extraFileExtensions: [".svelte"],
        parser: ts.parser,
        svelteConfig,
      },
    },
  },

  // ------------------------------------------------------------- tailwind --
  {
    files: ["**/*.svelte", "**/*.ts"],
    plugins: {"better-tailwindcss": tailwind},
    settings: {
      "better-tailwindcss": {
        // Resolves the real token set, so `bg-clay` fails while `bg-card` and `ease-clay` pass.
        entryPoint: "src/app.css",
      },
    },
    rules: {
      "better-tailwindcss/no-unknown-classes": "error",
      "better-tailwindcss/no-conflicting-classes": "error",
      "better-tailwindcss/no-duplicate-classes": "error",
      "better-tailwindcss/no-restricted-classes": ["error", {restrict: RESTRICTED_CLASSES}],
    },
  },

  // ------------------------------------------------------------ our code --
  {
    files: ["src/**/*.svelte"],
    rules: {
      // Nostr content is attacker-controlled, and this is the one remotely exploitable mistake.
      "svelte/no-at-html-tags": "error",
      "svelte/require-each-key": "error",
      "svelte/no-useless-mustaches": "error",
      "svelte/prefer-const": "error",
      "svelte/no-target-blank": "error",

      "no-restricted-syntax": [
        "error",
        {
          selector: "SvelteStyleElement",
          message:
            "No component-scoped CSS. Style with Tailwind utilities; if a value is missing, add it to the theme in src/app.css so both themes and every other component get it too. See docs/ui.md#no-style-blocks.",
        },
      ],
    },
  },

  // A browser's stand-in for the shell. Reaching for it from the view is a path that never runs.
  {
    files: ["src/**"],
    ignores: ["src/lib/dev/**", "src/lib/core.ts"],
    rules: {
      "no-restricted-imports": [
        "error",
        {
          patterns: [
            {
              group: ["**/dev/*", "$lib/dev/*"],
              message:
                "src/lib/dev is the simulated core `just dev` runs against, and src/lib/core.ts is the only thing that loads it. A view that reaches for it has a code path that only ever runs in a browser. See docs/ui.md#the-browser-has-a-core.",
            },
          ],
        },
      ],
    },
  },

  // Runes are compiler syntax; in a plain module they are an undefined global.
  {
    files: ["src/**/*.ts", "src/**/*.js"],
    rules: {"no-restricted-syntax": ["error", RUNES_ARE_COMPONENT_ONLY]},
  },

  // Feature code composes components; components own the pixels. Split by directory.
  {
    files: ["src/**"],
    ignores: ["src/lib/components/**"],
    rules: {
      "better-tailwindcss/no-restricted-classes": [
        "error",
        {restrict: [...RESTRICTED_CLASSES, ARBITRARY_VALUE]},
      ],
    },
  },

  // ---------------------------------------------------------- vendored ui --
  {
    files: ["src/lib/components/ui/**"],
    rules: {
      "better-tailwindcss/no-unknown-classes": "off",
      "better-tailwindcss/no-restricted-classes": "off",
      "better-tailwindcss/no-conflicting-classes": "off",
      "better-tailwindcss/no-duplicate-classes": "off",
      "@typescript-eslint/no-explicit-any": "off",
      "no-restricted-syntax": "off",
    },
  },

  // ------------------------------------------------------------- tooling --
  {
    files: ["*.config.{js,ts}", "eslint.config.js", "svelte.config.js"],
    languageOptions: {globals: globals.node},
  },
)
