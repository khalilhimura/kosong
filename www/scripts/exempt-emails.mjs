// Exempts email addresses inside code blocks from Cloudflare's Email Address
// Obfuscation.
//
// Scrape Shield rewrites anything email-shaped in the response body into a
// `[email protected]` link plus a decode script. In prose that is the whole
// point of the feature and those are left alone. In a code block it corrupts a
// command the reader is meant to copy: `kosong login --email you@example.com`
// reaches anyone without JavaScript as `kosong login --email [email protected]`,
// on the troubleshooting page, which is where people land when something has
// already gone wrong. With JavaScript it decodes correctly, which is how it
// survived — the failure is invisible in a browser.
//
// # Why this runs on the HTML and not on the Markdown
//
// Cloudflare documents `<!--email_off-->` for this. Where it goes is the whole
// problem, and both plausible placements were tried against the live zone:
//
//   - Wrapping the fence in Markdown puts the comments *outside* `<pre>`.
//     Cloudflare strips them and obfuscates anyway. Measured, not assumed:
//     the page still served `__cf_email__` with the markers gone.
//   - Putting them inside the `<code>`, either side of the address, is
//     honoured — the address is served in clear and no decode script is
//     injected.
//
// The second cannot be written in Markdown at all: inside a fence the comment
// is escaped and renders as literal text in the code block, which is worse than
// the problem. So it has to happen after Astro has produced the HTML.
//
// Comments are not rendered and are not part of a node's text content, so this
// is invisible on screen and changes nothing about what a reader copies.
//
// Run after `astro build`; `npm run build` does it, followed by the check that
// proves it landed.

import { readdir, readFile, writeFile } from "node:fs/promises";
import { dirname, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const dist = resolve(here, "../dist");

/** Deliberately loose: this asks "would Cloudflare rewrite it", not "is it valid". */
const EMAIL = /[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}/g;

const PRE_BLOCK = /<pre[\s\S]*?<\/pre>/g;

async function* htmlFiles(dir) {
  for (const entry of await readdir(dir, { withFileTypes: true })) {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) yield* htmlFiles(path);
    else if (entry.name.endsWith(".html")) yield path;
  }
}

let exempted = 0;

for await (const file of htmlFiles(dist)) {
  const html = await readFile(file, "utf8");

  const updated = html.replace(PRE_BLOCK, (block) => {
    // Already done — this build step is safe to run twice.
    if (block.includes("email_off")) return block;
    return block.replace(EMAIL, (address) => {
      exempted += 1;
      return `<!--email_off-->${address}<!--/email_off-->`;
    });
  });

  if (updated !== html) {
    await writeFile(file, updated, "utf8");
    console.log(`  exempted in ${relative(dist, file)}`);
  }
}

console.log(`email exemptions: ${exempted} address(es) in code blocks`);
