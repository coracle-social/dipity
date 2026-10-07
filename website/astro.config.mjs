// The website: static pages, styled from the app's own tokens.
import {defineConfig} from "astro/config"
import tailwindcss from "@tailwindcss/vite"

export default defineConfig({
  vite: {plugins: [tailwindcss()]},
})
