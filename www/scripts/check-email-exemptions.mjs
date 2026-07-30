// Every email address in a code block must be exempt from Cloudflare's Email
// Address Obfuscation, with the markers *inside* the block.
//
// `exempt-emails.mjs` explains what the obfuscation does and why the placement
// is the whole problem. This proves the placement is still right, on the built
// HTML, on every build.
//
// # Why it insists the markers are inside the `<pre>`
//
// The first attempt at this wrapped the fence in Markdown, which puts the
// comments outside `<pre>`. Cloudflare ignores that and obfuscates anyway. A
// check that only asked "is this email inside some exempt region" passed that
// broken build happily, and the page went to production still serving a command
// nobody could copy. So the region has to be found *within* the block, which is
// the arrangement that was actually observed working against the live zone.
//
// What this still cannot prove is that Cloudflare honours it — that is a fact
// about someone else's edge, confirmed by fetching the deployed page and
// finding no `__cf_email__`. This checks the input to that, which is the half
// that can regress here.

import { readdir, readFile } from "node:fs/promises";
import { dirname, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const dist = resolve(here, "../dist");

/** Deliberately loose: this asks "would Cloudflare rewrite it", not "is it valid". */
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
let protectedCount = 0;

for await (const file of htmlFiles(dist)) {
  const html = await readFile(file, "utf8");

  for (const [block] of html.matchAll(PRE_BLOCK)) {
    // Only regions inside this block count. Markers outside it are the
    // arrangement Cloudflare discards, so they must not satisfy this.
    const exposed = block.replace(EXEMPT_REGION, "");
    const found = exposed.match(EMAIL);
    if (found) failures.push(`${relative(dist, file)}: ${found[0]}`);
    else protectedCount += (block.match(EXEMPT_REGION) ?? []).length;
  }
}

if (failures.length) {
  console.error(
    "An email address in a code block is not exempt from Cloudflare's\n" +
      "obfuscation, so it will be served as `[email protected]` and the command\n" +
      "cannot be copied:\n",
  );
  for (const failure of failures) console.error(`  ${failure}`);
  console.error(
    "\nThe markers must sit inside the <pre>, around the address itself.\n" +
      "Outside it, Cloudflare strips them and obfuscates anyway.",
  );
  process.exit(1);
}

console.log(`email exemptions verified: ${protectedCount} address(es) protected in code blocks`);
