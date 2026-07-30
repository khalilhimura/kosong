// Every email-shaped string in a code block must be exempt from Cloudflare's
// Email Address Obfuscation.
//
// Scrape Shield rewrites anything email-shaped in the response body into a
// `[email protected]` link plus a decode script. In prose that is the whole
// point of the feature. In a code block it silently corrupts a command the
// reader is meant to copy: `kosong login --email you@example.com` reaches
// anyone without JavaScript as `kosong login --email [email protected]`, on the
// troubleshooting page, which is where people land when something is already
// going wrong.
//
// `collect.mjs` wraps those fences in `<!--email_off-->`. This checks the
// wrapping survived all the way to the HTML, because that is the part that can
// break without anyone touching it — a remark or Astro upgrade that starts
// stripping comments would leave the site serving a broken command with every
// other check still green. It was found by comparing served bytes against the
// local build, which is not something anyone does twice.
//
// Run against `dist/` after `astro build`; `npm run build` does it.

import { readdir, readFile } from "node:fs/promises";
import { dirname, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const dist = resolve(here, "../dist");

/** Deliberately loose: this is a "would Cloudflare rewrite it" test, not validation. */
const EMAIL = /[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}/;

const EXEMPT_REGION = /<!--email_off-->[\s\S]*?<!--\/email_off-->/g;
const PRE_BLOCK = /<pre[\s\S]*?<\/pre>/g;

async function* htmlFiles(dir) {
  for (const entry of await readdir(dir, { withFileTypes: true })) {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) yield* htmlFiles(path);
    else if (entry.name.endsWith(".html")) yield path;
  }
}

const failures = [];
let exempted = 0;

for await (const file of htmlFiles(dist)) {
  const html = await readFile(file, "utf8");

  exempted += (html.match(EXEMPT_REGION) ?? []).length;

  // Whatever is already inside an exempt region is, by definition, fine. What
  // is left is what Cloudflare will rewrite.
  const unprotected = html.replace(EXEMPT_REGION, "");

  for (const [block] of unprotected.matchAll(PRE_BLOCK)) {
    const found = block.match(EMAIL);
    if (found) failures.push(`${relative(dist, file)}: ${found[0]}`);
  }
}

if (failures.length) {
  console.error(
    "An email address in a code block is not exempt from Cloudflare's\n" +
      "obfuscation, so it will be served as `[email protected]` and the command\n" +
      "cannot be copied:\n",
  );
  for (const failure of failures) console.error(`  ${failure}`);
  console.error("\nWrap the fence in collect.mjs, or check the comment survived the build.");
  process.exit(1);
}

console.log(`email exemptions: ${exempted} code block(s) protected, none exposed`);
