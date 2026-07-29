# Changelog

What changed, in the order it changed, in language a user of `kosong` can act
on. Anything that alters the [CLI contract](spec/cli-v1.md) says so explicitly.

This project follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).
Before `1.0.0`, a minor version may change behaviour; the CLI contract's stable
surface — exit codes, `--json` shapes at their schema version, documented flags
and environment variables — will not change without being named here.

## Unreleased

## [0.2.1] — 2026-07-29

A release you can rebuild and check for yourself. Nothing about using `kosong`
changes.

### Changed

- Release archives are now built reproducibly. Building the same tag twice used
  to produce two different checksums, because `tar` and `gzip` record the moment
  they ran alongside the files. Nothing was wrong with the downloads — the
  published `SHA256SUMS` always matched what was published — but it meant nobody
  could rebuild a release and confirm they had got the same thing. Timestamps
  now come from the commit being built rather than the clock, so a rebuild of
  the same tag on the same platform produces identical bytes.

  This changes nothing about how you verify a download; `shasum -a 256 -c
  SHA256SUMS --ignore-missing` works exactly as before.

## [0.2.0] — 2026-07-29

kosong can be installed with npm. Everything else in this release is a fix to
publishing, which stopped working correctly the second time you used it.

### Added

- kosong can be installed with npm. `npm install -g kosong` now works, and
  `npx kosong start` runs it without installing anything permanently. This is
  in addition to the installer at
  <https://kosong.thefutureissolo.com/install.sh>, which is unchanged and still
  the way to get kosong without involving Node at all.

  The npm package carries the same binary the installer downloads. There is no
  install script and nothing is fetched while installing: npm selects the build
  for your machine from ones already published, so this works offline, under
  `--ignore-scripts`, and behind a proxy that inspects TLS. macOS and glibc
  Linux, on Intel and ARM — the same four builds as every other channel.
  Windows and Alpine are not among them, and on those the install stops with a
  message saying so rather than leaving you something that cannot run.

  Installed this way, `kosong` is a small script that starts the real binary,
  which costs a fraction of a second on each command. If that matters to you,
  use the installer.

- The service now deletes what it no longer needs. Security events are kept 90
  days and verification-code records 24 hours; a daily job removes the rest.
  Both tables previously grew without limit, which meant the record of a
  sign-in attempt outlived any use for it — and, for codes, that the plaintext
  address of anyone who asked for a code and never came back was kept
  indefinitely. [`spec/telemetry-v1.md`](spec/telemetry-v1.md) states the
  windows and why each is the length it is. Nothing you can see changes.

### Fixed

- Publishing a page now works the first time. `kosong site publish` deployed
  into a Cloudflare Pages project without ever creating one, so a first publish
  stopped at a wrangler error telling you to run a command kosong does not
  offer. Publishing now makes the project when it does not exist, as a step it
  shows you and asks about before running. Publishing to a project that already
  exists is unchanged and asks nothing extra. A name Cloudflare will not accept
  is now refused with an explanation instead of a provider error, and a name
  already taken on your account stops the publish rather than deploying into
  someone else's project.

- A deployed service with no email provider configured now refuses to send,
  rather than falling back to writing verification codes into its own log. That
  fallback exists for local development, where the log and the mailbox belong
  to the same person; following an unset key into production made live sign-in
  codes readable by anyone with access to the logs. Sign-in fails with `503
  EMAIL_UNAVAILABLE` and the misconfiguration is logged once, loudly.

- Publishing a page a second time now works. Every publish runs `npm install`,
  which writes `package-lock.json`; the publish after that read the lockfile as
  a change kosong did not make and stopped, telling you to commit a file kosong
  itself had caused to appear. `init` → `publish` → `edit` → `publish` failed
  that way for every site, and the only escape was to commit the file by hand.
  The lockfile is now kosong's to keep, and is committed with the rest of your
  site. A site that is already stuck heals itself on the next publish — there
  is nothing to migrate and nothing to delete first.

- Deleting a file kosong made is now recorded. `rm astro.config.mjs` followed
  by `kosong site publish` reported success while git quietly kept the file for
  ever, so the history you own stopped matching the folder in front of you, and
  anyone who cloned it got the file back. A removed file is now staged as a
  removal, the same as any other change.

- A push that fails now says why. `could not send to GitHub; your page will
  still be published` was the whole report — no cause, and nothing to do about
  it. kosong now prints git's own reason, and where it recognises the cause the
  repair names the command that fixes it. The common cause is worth stating
  outright, because it is not obvious: being signed in to GitHub and being
  signed in to git are two different things. `gh auth login` asks about the
  second as a separate question, and `gh auth status` passing tells you nothing
  about it — so kosong could create your repository and then fail every push to
  it, with neither tool objecting. `gh auth setup-git`, run once, connects the
  second half, and the message says to run `gh auth login` first if that
  command reports you are not signed in at all. A failed push still does not
  stop the publish; your page goes live either way.

- A project name that begins like a token is no longer blanked out. kosong
  hides anything credential-shaped in the output of the commands it runs, and
  one of the shapes it hid was three characters — `xox` — which are also the
  first three of `xoxo`. If your Cloudflare Pages project was called
  `xoxo-blog`, kosong never saw the name Cloudflare sent back: it concluded the
  project did not exist, tried to create one that did, and told you to choose a
  different name for a name already yours. A credential is now recognised only
  at the start of a word, and Slack's token types are named in full rather than
  by a prefix they share with ordinary words.

## [0.1.2] — 2026-07-27

A failing sign-in now tells you something you can act on. Nothing about the
local half of kosong changes.

### Fixed

- Asking for a code no longer fails with a server error when the email provider
  refuses the address. The provider returns 422 for domains it will not deliver
  to, such as `example.com`, and any provider failure became an unhandled 500 —
  which told the user nothing and, because it differed from the ordinary
  response, told an attacker which addresses a provider accepts. A refused
  recipient now gets the same `202` and the same body as everyone else, logged
  for the operator. A provider that is genuinely down is reported as `503
  EMAIL_UNAVAILABLE` with guidance to try again, because a code that will never
  arrive should not look like one that is on its way.
- Only a malformed address or a refused domain counts as the recipient's fault.
  An expired API key, an unpaid account, or a rate limit at the provider is the
  service's, and is reported as such. Classified the other way, a revoked key
  would have answered every sign-in with "a code is on its way" while sending
  nothing at all.
- `kosong` shows what the service said when the service explained itself. A
  `503` carrying a message now surfaces that message; previously any 5xx became
  "the service had a problem" plus a request id, which is not a next action.

## [0.1.1] — 2026-07-27

The service this release talks to is deployed and reachable. 0.1.0 shipped
pointing at hostnames that did not exist, so its account features could not
work without setting `KOSONG_API_URL` by hand.

### Changed

- `kosong` now reaches the service by default. The built-in address is
  `https://api.kosong.thefutureissolo.com`, which exists; 0.1.0 named
  `api.kosong.dev`, which does not.
- The installer moved to `https://kosong.thefutureissolo.com/install.sh`,
  including in the advice printed by the binary itself. A repair action naming
  a dead URL is worse than none.

### Added

- A project website at <https://kosong.thefutureissolo.com>, which serves the
  installer and renders the guides from the same Markdown the repository holds.

### Fixed

- Verification emails send. 0.1.0's service sent from a domain nobody owned,
  and with no email provider configured it wrote codes into its own logs.

## [0.1.0] — 2026-07-26

First release. The local half of kosong is complete and works with no
account and no network; the account features need a deployed service, and
the install host in the README does not exist yet — install from a release
archive until it does.

### Added

- `kosong delete-account` deletes your account and the page stored on the
  server. Nothing on your computer is removed, and a published site stays
  online; the command says so before it asks. `--dry-run` shows the effect
  without performing it, and confirmation defaults to no, so an unattended
  script must pass `--yes`.
- `kosong doctor` now states its lesson: every line it prints is a question you
  can ask your computer yourself.

### Fixed

- Repair advice no longer points at `kosong update --check`, a command that
  does not exist. Anyone who followed it got `unrecognized subcommand`. Both
  places now point at the install command. A test walks real failure output and
  checks that every command kosong tells you to run is one it has.
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
  ahead of GitHub's removal of Node.js 20 from the runners. The release
  workflow's `upload-artifact` and `download-artifact` moved for the same
  reason, found by rehearsing a release rather than waiting for a tag.

### Documentation

- `LICENSE` — the Apache-2.0 text the manifests have always referred to.
- [`guide/troubleshooting.md`](guide/troubleshooting.md) — what to do when a
  command fails, keyed by what you actually saw.
- [`guide/providers.md`](guide/providers.md) — what kosong will and will not
  ask GitHub and Cloudflare to do, and why that boundary exists.
- [`guide/course-outline.md`](guide/course-outline.md) — the five course
  modules mapped to real commands and the artefacts they produce.

[0.2.1]: https://github.com/khalilhimura/kosong/releases/tag/v0.2.1
[0.2.0]: https://github.com/khalilhimura/kosong/releases/tag/v0.2.0
[0.1.2]: https://github.com/khalilhimura/kosong/releases/tag/v0.1.2
[0.1.1]: https://github.com/khalilhimura/kosong/releases/tag/v0.1.1
[0.1.0]: https://github.com/khalilhimura/kosong/releases/tag/v0.1.0
