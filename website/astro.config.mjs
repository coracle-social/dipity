// The website: static pages, styled from the app's own tokens.
import {defineConfig} from "astro/config"
import tailwindcss from "@tailwindcss/vite"

export default defineConfig({
  vite: {
    plugins: [tailwindcss()],
    // Vite otherwise walks up to the app's tsconfig, which needs the app's dependencies installed.
    tsconfig: "./tsconfig.json",
  },
})
