// @ts-check
import { defineConfig } from "astro/config";

/**
 * Build configuration for a kosong site.
 *
 * Deliberately minimal. This file belongs to the user once `kosong site init`
 * copies it, and a beginner opening it should be able to read the whole thing.
 */
export default defineConfig({
  // Plain static output. Cloudflare Pages serves the `dist` folder directly,
  // with no adapter and no server runtime involved.
  output: "static",

  // Written as files rather than one bundle, so a curious user can open
  // `dist/index.html` and recognise their own page in it.
  build: {
    format: "file",
    inlineStylesheets: "always",
  },

  // No telemetry, no external fonts, no analytics. Nothing in the built
  // output calls anywhere the user did not ask it to.
  devToolbar: { enabled: false },
});
