# kosong

Open a terminal. Write one page. Publish it. Learn what happened.

`kosong` helps you make, understand, and publish one Markdown file. It is not a
website builder that hides the work — every step leaves you with something
ordinary you own: a Markdown file, a Git repository, and a static website.

**Everything local works with no account and no network.**

## Install

```bash
npm install -g kosong
```

Or run it without installing:

```bash
npx kosong start
```

`npm install kosong` without `-g` puts the command in `./node_modules/.bin`
rather than on your `PATH`, which is rarely what you want for a tool you
intend to type.

If you would rather not involve Node at all, the standalone installer fetches a
checksum-verified binary and no wrapper:

```bash
curl -fsSL https://kosong.thefutureissolo.com/install.sh | sh
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

## What this package contains

A small launcher, and one dependency carrying the binary for your platform.
npm picks it using the `os`, `cpu`, and `libc` fields; the others are skipped
and never downloaded. There is no install script, so this works offline, under
`--ignore-scripts`, and behind a proxy that inspects TLS.

Published for macOS (arm64, x86_64) and glibc Linux (x86_64, arm64). Windows
and musl are not built yet — on those, the install fails with a message saying
so rather than installing something that cannot run.

Full documentation: <https://github.com/khalilhimura/kosong>

## Licence

Apache-2.0.
