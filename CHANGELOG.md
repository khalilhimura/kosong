# Changelog

What changed, in the order it changed, in language a user of `kosong` can act
on. Anything that alters the [CLI contract](spec/cli-v1.md) says so explicitly.

This project follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).
Before `1.0.0`, a minor version may change behaviour; the CLI contract's stable
surface — exit codes, `--json` shapes at their schema version, documented flags
and environment variables — will not change without being named here.

## Unreleased

### Added

- `kosong delete-account` deletes your account and the page stored on the
  server. Nothing on your computer is removed, and a published site stays
  online; the command says so before it asks. `--dry-run` shows the effect
  without performing it, and confirmation defaults to no, so an unattended
  script must pass `--yes`.
- `kosong doctor` now states its lesson: every line it prints is a question you
  can ask your computer yourself.

### Fixed

- When the server refuses a `kosong sync --push` because the page changed
  underneath it, the saved `server-version.md` now holds the version that is
  actually on the server. It previously held the copy read *before* the push —
  a version no longer on the server — so a merge based on it would have
  discarded the other machine's work without ever displaying it. If the
  server's version cannot be read back, kosong now says so instead of writing
  an empty file and presenting it as the server's.

### Changed

- Repository URLs point at `khalilhimura/kosong`. `install.sh` built its
  download URL from the old path, so the published installer would have fetched
  from somewhere that does not exist.
- CI and release workflows use `actions/checkout@v7` and `actions/setup-node@v7`,
  ahead of GitHub's removal of Node.js 20 from the runners.

### Documentation

- `LICENSE` — the Apache-2.0 text the manifests have always referred to.
- [`guide/troubleshooting.md`](guide/troubleshooting.md) — what to do when a
  command fails, keyed by what you actually saw.
- [`guide/providers.md`](guide/providers.md) — what kosong will and will not
  ask GitHub and Cloudflare to do, and why that boundary exists.
- [`guide/course-outline.md`](guide/course-outline.md) — the five course
  modules mapped to real commands and the artefacts they produce.

<!--
Releases below this line. Each gets its own section when a tag is cut:

## [0.1.0] — YYYY-MM-DD
-->
