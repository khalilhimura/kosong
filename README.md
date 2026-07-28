# kosong

Open a terminal. Write one page. Publish it. Learn what happened.

`kosong` helps you make, understand, and publish one Markdown file. It is not a
website builder that hides the work — every step leaves you with something
ordinary you own: a Markdown file, a Git repository, and a static website.

**Everything local works with no account and no network.**

---

## Install

```bash
curl -fsSL https://kosong.thefutureissolo.com/install.sh | sh
```

Piping a script into your shell deserves suspicion. This one downloads a fixed
release and refuses to install anything whose checksum does not match — you can
[read it first](install.sh), or [install manually from a release][releases] and
verify it yourself:

```bash
shasum -a 256 -c SHA256SUMS --ignore-missing
```

## Your first page

```bash
kosong start      # make a page
kosong edit       # open it in your editor
kosong preview    # look at it, on your own computer
kosong status     # see where things stand
```

Nothing above touches the network or asks who you are.

## Publishing, when you want to

```bash
kosong login            # a six-digit code by email; no password, ever
kosong sync             # keep a private copy on the server
kosong site init        # turn your page into a publishable folder
kosong site publish     # build it and put it online
```

Every command that changes something outside your computer tells you exactly
what it will run, where, and what will change — then asks. Add `--dry-run` to
see the plan without doing anything.

## Commands

| Command | What it does | What it teaches |
|---|---|---|
| `kosong start` | Set things up, one step at a time | — |
| `kosong new` | Create a page | A file is a durable object you can move |
| `kosong edit` | Open your editor | The terminal opens editors; it is not one |
| `kosong show` | Display the page | — |
| `kosong preview` | Serve it locally | A local server is a temporary website |
| `kosong status` | Report current state | Software can describe itself |
| `kosong doctor` | Check your setup | Prerequisites can be checked, not guessed |
| `kosong login` / `logout` | Sign in by email code | — |
| `kosong delete-account` | Delete your account and its stored page | Deleting an account is not deleting your files |
| `kosong sync` | Keep a private remote copy | — |
| `kosong gh` / `kosong cf` | Ask GitHub or Cloudflare about your setup | — |
| `kosong site init` | Make a publishable folder | Git tracks a folder's history |
| `kosong site publish` | Build and deploy | A place must exist before files go in it. Deployment moves files, not magic |
| `kosong site rollback` | See past versions, and where to restore one | An honest limit beats a convincing wrong answer |

`kosong status --json` and `kosong doctor --json` produce stable machine-readable
output. `--quiet` suppresses prose but never errors. `NO_COLOR` is respected.

## Guides

| Guide | For |
|---|---|
| [Troubleshooting](guide/troubleshooting.md) | A command failed and you want the next step |
| [GitHub and Cloudflare](guide/providers.md) | What kosong asks them to do, and what it refuses to |
| [Course outline](guide/course-outline.md) | The five modules and what each one leaves you able to do |

## What you end up owning

| Thing | Where | Works without kosong |
|---|---|---|
| Your page | `kosong.md` | Yes — any Markdown editor |
| Your site | `<name>/` | Yes — an ordinary Astro project |
| Its history | `<name>/.git` | Yes — ordinary Git |
| The built files | `<name>/dist` | Yes — plain HTML |

If you delete `kosong` tomorrow, all four still work.

## The file format

`kosong.md` is a conformant [Open Knowledge Format][okf] v0.1 document —
Google Cloud's vendor-neutral specification for knowledge as Markdown with YAML
front matter. `kosong` does not define a format of its own; it adopts OKF and
adds one namespaced `kosong:` block.

```markdown
---
type: Page
title: My First Site
description: A page published with kosong.
tags: [kosong]
timestamp: 2026-07-26T10:30:00+08:00
kosong:
  profile: 1
  id: 01J7ZQ8F3K9XG2VW6M4T1B5NRY
  slug: my-first-site
  visibility: private
  created_at: 2026-07-26T10:30:00+08:00
---

# My First Site
```

Any OKF-aware tool can read this without knowing what `kosong` is. See
[`spec/okf-profile-v1.md`](spec/okf-profile-v1.md).

## Security, stated plainly

kosong uses HTTPS, encryption at rest, private storage, verified-email sign-in,
hashed single-use codes, short-lived access tokens, and rotating refresh tokens.

**It is not end-to-end encrypted.** The service operator could read a synced
document. If that matters to you, do not sync — everything local works without
it. See [SECURITY.md](SECURITY.md) for the full boundary.

kosong never receives your GitHub or Cloudflare credentials. Those stay in `gh`
and `wrangler`, and kosong only ever asks them whether you are signed in.

## Two things kosong deliberately will not do

**It never runs a shell.** Every external tool is invoked as an executable plus
separate arguments, so a `;` in a name is a character, not a command. There is
no `sh -c` anywhere in the codebase.

**It cannot roll back a Cloudflare Pages deployment.** Wrangler has no command
for it, and doing it through Cloudflare's API would mean holding your Cloudflare
credentials — which kosong is built never to do. `kosong site rollback` shows
your deployment history and tells you exactly where to click instead.

## Building from source

```bash
cargo build --release      # the CLI
cargo test --workspace     # 259 tests
cargo clippy --workspace --all-targets -- -D warnings
```

Repository layout:

```
crates/kosong-core/   document, workspace, render, process adapters
crates/kosong-cli/    the kosong binary
apps/api/             Cloudflare Worker: auth and sync
templates/site/       the bundled Astro template
spec/                 format profile, CLI contract, threat model, telemetry
guide/                troubleshooting, provider boundaries, course outline
```

## Licence

Apache-2.0. See [LICENSE](LICENSE).

[okf]: https://github.com/GoogleCloudPlatform/knowledge-catalog/blob/main/okf/SPEC.md
[releases]: https://github.com/khalilhimura/kosong/releases
