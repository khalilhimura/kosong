// Pulls the published guides and the installer into the site.
//
// The guides live in `guide/` and are read on GitHub as often as here, so they
// stay the source of truth and this copies them in at build time. Nothing in
// `src/pages/guide/` or `public/install.sh` is written by hand, and none of it
// is committed.
//
// The link rewriting is the part that matters. A guide links to files by
// relative path — `../SECURITY.md`, `providers.md` — which resolve on GitHub
// and would 404 here. Each one is pointed somewhere real.

import { mkdir, readdir, readFile, writeFile, copyFile, rm } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const repo = resolve(here, "../..");
const guideDir = join(repo, "guide");
const outDir = join(here, "../src/pages/guide");
const publicDir = join(here, "../public");

const BLOB = "https://github.com/khalilhimura/kosong/blob/main";

/** Anything the site does not host itself points back at the repository. */
function rewriteLinks(markdown) {
  return (
    markdown
      // Sibling guides become routes on this site.
      .replace(/\]\((troubleshooting|providers|course-outline)\.md\)/g, "](/guide/$1)")
      // Everything reached with `../` is a repository file.
      .replace(/\]\(\.\.\/([^)]+)\)/g, `](${BLOB}/$1)`)
      // The installer is served here, so keep it local.
      .replace(new RegExp(`\\]\\(${BLOB}/install\\.sh\\)`, "g"), "](/install.sh)")
  );
}

/** Matches a fenced code block, opening line to closing fence. */
const FENCE = /^```[^\n]*\n[\s\S]*?\n```/gm;

/** Deliberately loose: this asks "would Cloudflare rewrite it", not "is it valid". */
const EMAIL = /[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}/;

/**
 * Exempts code blocks from Cloudflare's Email Address Obfuscation.
 *
 * Scrape Shield rewrites anything email-shaped into a `[email protected]` link.
 * In prose that is the point of the feature, and those are left alone. In a
 * code block it corrupts a command the reader is meant to copy: `kosong login
 * --email you@example.com` reaches a reader without JavaScript as `kosong login
 * --email [email protected]`.
 *
 * The comment cannot go inside the fence. Markdown escapes it there and it
 * renders as literal text in the code block, which is worse than the problem.
 * So it wraps the fence at block level, where Astro passes it through to the
 * HTML untouched — verified, and re-verified on every build by
 * `check-email-exemptions.mjs`, since a comment silently stripped by some later
 * upgrade would look exactly like this working.
 *
 * This belongs here rather than in `guide/`. Those files are read on GitHub,
 * where `email_off` means nothing; the site's hosting is the site build's
 * problem.
 */
function exemptCodeBlockEmails(markdown) {
  return markdown.replace(FENCE, (fence) =>
    EMAIL.test(fence) ? `<!--email_off-->\n\n${fence}\n\n<!--/email_off-->` : fence,
  );
}

/** The first `# ` line, which every guide has. */
function titleOf(markdown, fallback) {
  const match = markdown.match(/^#\s+(.+)$/m);
  return match ? match[1].trim() : fallback;
}

/** The first paragraph, for the page description and the index card. */
function summaryOf(markdown) {
  const body = markdown.replace(/^#\s+.+$/m, "").trim();
  const paragraph = body.split(/\n\s*\n/).find((p) => p.trim() && !p.startsWith("#"));
  return paragraph ? paragraph.replace(/\s+/g, " ").replace(/[`*]/g, "").trim() : "";
}

/** YAML-safe double-quoted scalar. */
function quote(value) {
  return `"${value.replace(/\\/g, "\\\\").replace(/"/g, '\\"')}"`;
}

/**
 * The README's command table.
 *
 * The front page shows a curated subset of these rows, in its own order — but
 * the wording of each is the README's, read at build time. It was hand-copied
 * once, and drifted the first time a lesson was reworded: the README learned
 * that `site rollback` teaches an honest limit while the site still said
 * something vaguer, and nothing caught it.
 */
function commandsFrom(markdown) {
  const section = markdown.match(/^## Commands\s*$([\s\S]*?)^## /m);
  if (!section) throw new Error("README has no '## Commands' section");

  const rows = [];
  for (const line of section[1].split("\n")) {
    const match = line.match(/^\|(.+)\|(.+)\|(.+)\|\s*$/);
    if (!match) continue;
    const cells = match.slice(1, 4).map((cell) => cell.trim());
    // The header and the `|---|` rule beneath it.
    if (cells[0] === "Command") continue;
    if (cells.every((cell) => /^:?-+:?$/.test(cell))) continue;
    rows.push({
      name: cells[0].replace(/`/g, ""),
      does: cells[1],
      teaches: cells[2],
    });
  }

  if (!rows.length) throw new Error("README's command table parsed to nothing");
  return rows;
}

await rm(outDir, { recursive: true, force: true });
await mkdir(outDir, { recursive: true });
await mkdir(publicDir, { recursive: true });

const files = (await readdir(guideDir)).filter((name) => name.endsWith(".md"));
const collected = [];

for (const name of files) {
  const slug = name.replace(/\.md$/, "");
  const raw = await readFile(join(guideDir, name), "utf8");
  const title = titleOf(raw, slug);
  const summary = summaryOf(raw);

  const frontMatter = [
    "---",
    "layout: ../../layouts/Guide.astro",
    `title: ${quote(title)}`,
    `description: ${quote(summary.slice(0, 160))}`,
    "---",
    "",
  ].join("\n");

  await writeFile(
    join(outDir, name),
    frontMatter + exemptCodeBlockEmails(rewriteLinks(raw)),
    "utf8",
  );
  collected.push({ slug, title, summary });
}

// The installer is served from this site, so it has to be this site's copy.
await copyFile(join(repo, "install.sh"), join(publicDir, "install.sh"));

await writeFile(
  join(here, "../src/guides.json"),
  JSON.stringify(collected.sort((a, b) => a.slug.localeCompare(b.slug)), null, 2),
  "utf8",
);

const commands = commandsFrom(await readFile(join(repo, "README.md"), "utf8"));
await writeFile(
  join(here, "../src/commands.json"),
  JSON.stringify(commands, null, 2),
  "utf8",
);

console.log(
  `collected ${collected.length} guides, ${commands.length} commands, and install.sh`,
);
