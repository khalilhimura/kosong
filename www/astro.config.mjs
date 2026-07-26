// @ts-check
import { defineConfig } from "astro/config";

export default defineConfig({
  site: "https://kosong.thefutureissolo.com",
  // Static output, served by Cloudflare Pages. No adapter and no server: the
  // site makes the same claim the product does, that what you get is files.
  output: "static",
  build: {
    format: "directory",
  },
  markdown: {
    // No syntax theme. Every code block in the guides is a shell command, and
    // a highlighter would drag in a second palette that fights this one — the
    // light theme even kept a white background in dark mode. The site's own
    // styling does the work instead.
    syntaxHighlight: false,
  },
});
