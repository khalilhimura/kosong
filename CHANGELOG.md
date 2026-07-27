# Changelog

What changed, in the order it changed, in language a user of `kosong` can act
on. Anything that alters the [CLI contract](spec/cli-v1.md) says so explicitly.

This project follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).
Before `1.0.0`, a minor version may change behaviour; the CLI contract's stable
surface — exit codes, `--json` shapes at their schema version, documented flags
and environment variables — will not change without being named here.

## Unreleased

### Added

- The service now deletes what it no longer needs. Security events are kept 90
  days and verification-code records 24 hours; a daily job removes the rest.
  Both tables previously grew without limit, which meant the record of a
  sign-in attempt outlived any use for it — and, for codes, that the plaintext
  address of anyone who asked for a code and never came back was kept
  indefinitely. [`spec/telemetry-v1.md`](spec/telemetry-v1.md) states the
  windows and why each is the length it is. Nothing you can see changes.

### Fixed

- A deployed service with no email provider configured now refuses to send,
  rather than falling back to writing verification codes into its own log. That
  fallback exists for local development, where the log and the mailbox belong
  to the same person; following an unset key into production made live sign-in
  codes readable by anyone with access to the logs. Sign-in fails with `503
  EMAIL_UNAVAILABLE` and the misconfiguration is logged once, loudly.

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

[0.1.2]: https://github.com/khalilhimura/kosong/releases/tag/v0.1.2
[0.1.1]: https://github.com/khalilhimura/kosong/releases/tag/v0.1.1
[0.1.0]: https://github.com/khalilhimura/kosong/releases/tag/v0.1.0
