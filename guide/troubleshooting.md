# When something goes wrong

Find what you saw on your screen. Every entry says what happened, what to do,
and — because knowing why is the point — what the failure was actually telling
you.

If you are not sure where to start, run this. It checks everything kosong needs
and prints a next step for anything that is not ready:

```bash
kosong doctor
```

Nothing on this page can lose your work. `kosong.md` is an ordinary file on your
computer, and no command here deletes it.

---

## Exit codes, if you are scripting

A command's number tells you what kind of problem it was, without reading the
message. These are fixed; see the [CLI contract](../spec/cli-v1.md).

| Code | Meaning | Typical cause |
|---:|---|---|
| `0` | Success | — |
| `2` | Your input, or something not ready locally | No page yet, bad email, invalid front matter |
| `3` | Sign-in | Not signed in, or the session expired |
| `4` | An outside tool | `gh` or `wrangler` missing or not signed in |
| `5` | Network or the kosong service | Offline, rate limited, service down |
| `10` | A bug in kosong | Please report it |

---

## Starting out

### `kosong: command not found`

The install put `kosong` in `~/.local/bin`, and your shell does not look there.

```bash
export PATH="$HOME/.local/bin:$PATH"
```

Add that line to `~/.zshrc` or `~/.bashrc` to make it stick. **Why:** your shell
finds programs by searching a list of folders. Installing a program and telling
your shell where to look are two separate things.

### "kosong has not made a page in this folder"

You are in a folder with no `kosong.md`. Either move to the one that has it, or
make a page here:

```bash
kosong start
```

kosong looks in the current folder and walks upwards, so being in a subfolder of
your page is fine. **Why:** a page is a file in a place, not an entry in a
database somewhere else.

### "kosong does not know which editor to open"

```bash
export EDITOR=nano
```

Any editor works: `nano`, `vim`, `code --wait`, `zed --wait`. For editors that
open a window, the `--wait` part matters — without it the editor returns
immediately and kosong thinks you are done.

**Why:** the terminal is not an editor. It opens one, which means you keep using
whichever editor you already like.

### "the editor setting contains `;`, which kosong cannot use"

Something in `EDITOR` has a `;`, `|`, `$`, or similar. kosong never runs a
shell, so it cannot interpret those, and it refuses rather than guessing.

Set `EDITOR` to a program and its arguments only — `code --wait`, not
`code --wait; echo done`.

**Why:** if kosong ran shell strings, anything that could set a variable could
run any command. Refusing is what makes a `;` in a filename harmless.

---

## Your page

### "this file needs fixing" / front-matter errors

The block between the `---` lines at the top is YAML, and something in it does
not parse. The message names the problem; these are the common ones:

| Message | Fix |
|---|---|
| `type` is missing | Add `type: Page` |
| Give `type` a value | `type:` with nothing after it — write `type: Page` |
| The `kosong:` block needs indented lines | Indent the lines under `kosong:` by two spaces |
| Add `id` inside the `kosong:` block | Add it back, or delete the whole `kosong:` block and run `kosong status` — kosong rebuilds it |
| Check the front matter for a stray quote, tab, or missing colon | A value containing `:` needs quotes: `title: "Notes: part two"` |

**Why:** the file is yours to edit by hand, which means it is yours to break by
hand. kosong tells you which line and rebuilds what it safely can.

### "this file uses kosong profile N, but this version understands profile 1"

The page was written by a newer kosong than the one you are running.

```bash
curl -fsSL https://kosong.thefutureissolo.com/install.sh | sh
```

**Why:** the format is versioned so an older program refuses a newer file rather
than silently dropping the parts it does not understand.

### "index.md is reserved"

Open Knowledge Format uses `index.md` for a bundle's own description, so kosong
will not put your page there. Choose another name:

```bash
kosong new --path notes.md
```

---

## Preview

### "something else is already using port 4173"

```bash
kosong preview --port 4174
```

### "ports below 1024 need administrator rights"

Pick a higher number. Ports under 1024 are reserved for system services.

### The preview page is not reachable from my phone

That is deliberate and cannot be changed. The preview binds to `127.0.0.1`, which
means your computer only.

**Why:** a preview is for looking at your own work in progress. Publishing is a
separate, deliberate act — `kosong site publish` — not a side effect of looking.

---

## Signing in

### "too many requests; try again in N seconds"

Wait the number of seconds it names. A code can be requested once a minute per
address.

**Why:** without a cooldown, an address could be used to send someone else
unlimited mail.

### "that code is not right, or it has expired"

Codes last ten minutes, work once, and allow a handful of attempts. Request a
fresh one:

```bash
kosong login --email you@example.com
```

Check you are using the newest email — requesting a code invalidates the
previous one.

### "you are already signed in as somebody else"

```bash
kosong logout
kosong login
```

**Why:** reporting success while quietly leaving the old session in place would
have you operating one account believing you were on another.

### "your system keychain was not available"

Not an error. kosong stored your sign-in in a file only you can read, and said
so rather than pretending otherwise. Common on servers and in containers, where
no keychain daemon runs.

---

## Syncing

### "your page and the server's have both changed"

Nothing was overwritten. Both versions were written to `.kosong/conflicts/`, and
the message gives you the two paths. Open them, decide what the page should say,
save it as `kosong.md`, then:

```bash
kosong sync --push
```

**Why:** kosong will not merge two versions of your writing, because it cannot
know which parts you meant to keep.

### "the server refused the write, and now reports no page at all"

The remote copy was deleted between kosong reading it and writing to it — most
often because the account was deleted somewhere else. Your page here is
untouched. Send it again:

```bash
kosong sync --push
```

### "you have not synced a page yet"

There is nothing on the server to pull. Push first:

```bash
kosong sync --push
```

### "could not reach the kosong service"

You are offline, or the service is down. **Everything local still works** —
create, edit, preview, and publish need no account and no network.

---

## GitHub and Cloudflare

### "`gh` is not installed" / "`wrangler` is not installed"

`gh` is an ordinary global install:

```bash
brew install gh                 # or see https://cli.github.com
```

`wrangler` is not. Cloudflare installs it **into each project**, so that you and
anyone you work with use the same pinned version — and kosong looks in your site
folder before it looks at your `PATH`:

```bash
cd my-site
npm i -D wrangler@latest
npx wrangler login
```

Neither is needed for local work. `kosong doctor` reports a missing one as a
warning, never a failure — and when it does find one, it prints the path it
found, so you can see which copy kosong will use.

### `npm install -g` said it worked, but the command is not found

```
$ npm install -g wrangler
changed 35 packages
$ wrangler login
zsh: command not found: wrangler
```

Both of those are true. npm did install it — into a folder your shell does not
search. Ask npm where that was:

```bash
npm config get prefix
```

The binary is in `<prefix>/bin`. If that folder is not in your `PATH`, nothing
can run it by name:

```bash
echo "$PATH"
```

`kosong site publish` diagnoses this for you rather than repeating "not
installed": it names the exact path it found and the folder missing from your
`PATH`.

**Prefer the project install above to fixing your `PATH`.** A global npm prefix
sometimes belongs to an application — a bundled copy of Node, say — and an
application update can quietly remove everything installed into it. A wrangler
in your site folder is yours, is pinned, and travels with the project.

### "GitHub CLI is installed, but it is not signed in yet"

```bash
gh auth login
kosong doctor
```

**Why kosong cannot do this for you:** kosong never receives your GitHub or
Cloudflare credentials. They stay in `gh` and `wrangler`, and kosong only ever
asks those tools whether you are signed in. See [providers](providers.md).

### "this folder has changes kosong did not make"

kosong refuses to commit files it did not create. Commit or stash your own work
first:

```bash
git -C my-site status
```

**Why:** a publish command that swept up whatever happened to be in the folder
would eventually publish something you did not mean to.

### `kosong site rollback` will not roll anything back

Correct, and it says so. Wrangler has no command for rolling back a Cloudflare
Pages deployment, and doing it through Cloudflare's API would mean kosong
holding your Cloudflare credentials. It lists your deployment history and tells
you exactly where to click. See [providers](providers.md).

---

## Still stuck

Two commands describe your situation exactly, and both are safe to run and safe
to paste anywhere — neither prints tokens, codes, email addresses, or the
contents of your page:

```bash
kosong status --json
kosong doctor --json
```

Report anything that looks like a bug in kosong itself — exit code `10`, a
panic, or advice that does not work — at
<https://github.com/khalilhimura/kosong/issues>. For anything with a security
impact, see [SECURITY.md](../SECURITY.md) instead.
