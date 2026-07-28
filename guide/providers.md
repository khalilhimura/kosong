# GitHub and Cloudflare, and what kosong will not do with them

`kosong` publishes to services you own accounts with. This page says exactly
what it asks them to do, what it refuses to do, and why the refusals are
deliberate rather than unfinished.

The short version: **kosong never receives your GitHub or Cloudflare
credentials.** They stay in `gh` and `wrangler`, which you install and sign in
yourself. kosong only ever asks those tools to act on your behalf, and only in
ways listed on this page.

---

## Why it works this way

If kosong held a GitHub token, then trusting kosong would mean trusting it with
everything that token can reach — every repository, every organisation, for as
long as the token lives. You would have to take that on faith.

Instead, the credential stays where you already put it. `gh` and `wrangler` are
tools you can inspect, sign out of, and use without kosong. This has a cost:
kosong cannot do anything those tools cannot, which is why
[`kosong site rollback`](#rollback-really-does-not-roll-back) sends you to a
dashboard instead of doing it for you. That cost is the point.

There is no code path in kosong that accepts a provider credential. The types
that describe provider operations have nowhere to put one.

---

## Everything kosong can ask GitHub to do

Four operations. Not four categories — four.

| Operation | What runs | Changes anything? |
|---|---|---|
| Check sign-in | `gh auth status` | No |
| Look up a repository | `gh repo view <owner/name>` | No |
| Create a repository | `gh repo create <name> --private --source . --push` | **Yes** |
| Update from the remote | `gh repo sync` | **Yes** |

## Everything kosong can ask Cloudflare to do

| Operation | What runs | Changes anything? |
|---|---|---|
| Check which account | `wrangler whoami` | No |
| List Pages projects | `wrangler pages project list` | No |
| Make a Pages project | `wrangler pages project create <name> --production-branch main` | **Yes** |
| List deployments | `wrangler pages deployment list --project-name <name>` | No |
| Deploy | `wrangler pages deploy <dir> --project-name <name>` | **Yes** |

Anything that changes something outside your computer shows you the exact
program, the exact arguments, the folder it runs in, and what will change —
then asks. `--dry-run` shows that plan without running anything.

```bash
kosong site publish --dry-run
```

---

## What kosong deliberately will not do

### Run arbitrary commands

There is no `kosong gh <anything>`. The operations above are a closed list in
the source code, not a filter over strings you type. Adding one means writing
it, reviewing it, and testing it by name.

If you want the rest of `gh`, you already have it — run `gh` yourself. That is
the whole point of the credential staying there.

### Run a shell

kosong invokes a program and its arguments separately, always. There is no
`sh -c` anywhere in the codebase, so a `;` in a repository name is a character
in a name, not a command.

### Accept a credential

No operation takes a token, key, or password. `kosong doctor` and `kosong gh
auth`/`kosong cf whoami` ask the provider tools whether *they* are signed in,
and report yes or no.

### Commit files it did not create

`kosong site publish` refuses when the site folder has changes kosong did not
make. Commit or stash them first. A publish that swept up whatever happened to
be in the folder would eventually publish something you did not mean to.

### Rollback, really

`kosong site rollback` lists your deployment history and tells you where to
click. It does not roll back.

This is not laziness. **Wrangler has no command for rolling back a Cloudflare
Pages deployment** — `wrangler pages deployment` offers `list`, `create`,
`tail`, and `delete`, and the top-level `wrangler rollback` is for Workers, not
Pages. Cloudflare documents Pages rollback as a dashboard action.

Cloudflare's REST API can do it, but calling it would mean kosong holding a
Cloudflare API token, which is the one thing this whole design exists to avoid.
Deleting the newer deployment and calling that a rollback would be worse than
being honest.

If Wrangler ships a Pages rollback command, this changes.

---

## Signing in to the provider tools

kosong never does this for you. Each tool has its own way:

```bash
gh auth login
wrangler login
```

Then confirm kosong can see it:

```bash
kosong doctor
```

Neither tool is needed for local work. `kosong doctor` reports a missing one as
a warning, never a failure — you can write, preview, and own a page with
neither installed.

---

## What you can do without kosong

Everything, which is the test that matters:

| Thing | Works without kosong because |
|---|---|
| Your page | It is a Markdown file |
| Your site folder | It is an ordinary Astro project |
| Its history | It is an ordinary git repository — `git log` works |
| Your GitHub repo | You created it under your account with `gh` |
| Your deployment | It is a Cloudflare Pages project on your account |

If you delete `kosong` tomorrow, none of the above stops working, and nothing
on this list needs kosong to change it.

---

The implementer-facing version of this boundary, including what it would take
to add an operation, is [`spec/extension-boundaries.md`](../spec/extension-boundaries.md).
