# Course outline

The supplemental course, module by module. This is the curriculum — what each
module teaches, which commands it is built on, and what the learner has to be
able to do to have finished it. The delivered lessons live wherever the course
is eventually hosted; this file is the contract they are written against.

**The course is supplementary and always will be.** Creating, editing,
previewing, and owning a page and a site must never require it. If a module
turns out to be teaching something a beginner cannot do without paying, that is
a product bug, not a course feature — fix the product.

Each module ends with an artefact the learner can point at afterwards. A module
that ends in understanding alone has not finished.

---

## Module 1 — The terminal is a place

**Built on:** installation, `kosong start`, `kosong new`, `kosong show`

You are not learning a language. You are learning that there is a *here*, that
programs run *in* it, and that nothing you type is irreversible by accident.

- A shell prompt is a place: `pwd`, `ls`, `cd`
- A program is a file your shell finds by searching a list of folders — which is
  why `command not found` is usually about `PATH`, not about you
- `kosong start` makes a real file in a real folder
- `kosong show` proves the file is just text

**Finished when:** the learner can open a terminal, find the folder their page
is in, and explain what `kosong new` put on disk.

**Artefact:** `kosong.md`

---

## Module 2 — One file is an asset

**Built on:** `kosong edit`, `kosong preview`, `kosong status`

The file is the thing you own. Everything else in the course is a service
performed on it.

- Front matter and body: metadata versus writing
- The file is [Open Knowledge Format](../spec/okf-profile-v1.md) — a documented,
  vendor-neutral format, not a kosong invention
- `EDITOR` is a setting you control, and kosong opens *your* editor
- `kosong preview` runs a web server on your own machine and nowhere else
- `kosong status` is a program describing its own state

**Finished when:** the learner can edit the page in an editor of their choosing,
preview it, and say why the preview is not on the internet.

**Artefact:** an edited page, previewed locally.

---

## Module 3 — Git is history

**Built on:** `kosong site init`

Git is not a backup service and not GitHub. It is a record of what changed, when.

- What `git init` actually created — a folder called `.git`, and nothing else
- A commit is a snapshot with a message and an author
- `git log`, `git status`, `git diff` on the repository kosong just made
- Why kosong refuses to commit files it did not create
- GitHub is a place to *put* a repository, not what a repository is

**Finished when:** the learner can read `git log` on their own site folder and
explain what each commit contains.

**Artefact:** a git repository with real history.

---

## Module 4 — Publish without surrendering ownership

**Built on:** `kosong site publish`, `kosong gh`, `kosong cf`

Deployment moves files to a computer that answers requests. It is not magic, and
it is not a transfer of ownership.

- A static site is files; a host is a computer that serves them
- Building: source in, `dist/` out
- Why `--dry-run` exists, and reading a plan before approving it
- Why kosong never holds your GitHub or Cloudflare credentials — see
  [providers](providers.md)
- What you would do if kosong vanished mid-project (answer: the same commands,
  yourself)

**Finished when:** the learner has a live URL, and can name which account it
lives under and how they would deploy again without kosong.

**Artefact:** a published site.

---

## Module 5 — Inspect, recover, and own the stack

**Built on:** `kosong doctor`, `kosong status --json`, `kosong sync`,
`kosong site rollback`, `kosong delete-account`

The difference between using a tool and operating one. This module is where
confidence stops depending on things going well.

- `kosong doctor`: every line is a question you could ask yourself
- Exit codes and `--json`, and why a contract is a promise about the future
- Sync conflicts: why kosong will not merge, and how to resolve one by hand
- What `kosong site rollback` will not do, and why an honest limit beats a
  convincing wrong answer
- Deleting your account, and what deleting it does *not* touch
- Reading [SECURITY.md](../SECURITY.md): the boundary, stated plainly

**Finished when:** the learner can diagnose a broken setup from `kosong doctor`
alone, and resolve a sync conflict without help.

**Artefact:** a resolved conflict, and a setup they repaired themselves.

---

## Open decisions

Two things are unresolved, and both belong to whoever runs the course, not to
this file:

1. **Four modules or five in the first release.** PRD §13 Phase D says "the
   first four course modules"; FR-6 lists five. Module 5 is the one that maps to
   `doctor`, `status`, and recovery — the material that separates a learner who
   can publish from one who can operate. Shipping four means shipping the
   modules that get someone to a URL and stopping before the part that makes
   them independent.

2. **Delivery and pricing.** PRD open decision #4 defers the platform until free
   activation is proven. The exit gate for this phase is five CLI novices
   completing the journey; pricing is decided after that, not before.

---

## The rule this outline is measured against

> `kosong` succeeds when a person who avoided the terminal can make a page,
> publish it, find every artefact afterwards, and understand enough of the path
> to do it again.

A module that leaves the learner able to repeat the steps but not to explain
them has failed, however smooth it felt.
