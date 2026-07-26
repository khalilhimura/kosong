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

  await writeFile(join(outDir, name), frontMatter + rewriteLinks(raw), "utf8");
  collected.push({ slug, title, summary });
}

// The installer is served from this site, so it has to be this site's copy.
await copyFile(join(repo, "install.sh"), join(publicDir, "install.sh"));

await writeFile(
  join(here, "../src/guides.json"),
  JSON.stringify(collected.sort((a, b) => a.slug.localeCompare(b.slug)), null, 2),
  "utf8",
);

console.log(`collected ${collected.length} guides and install.sh`);
