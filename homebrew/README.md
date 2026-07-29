# Homebrew

`formula.mjs` generates the formula for a released version. It reads the
release's `SHA256SUMS` and embeds the checksums, so `brew install` verifies the
same bytes `install.sh` downloads and the npm packages carry. Nothing is
rebuilt, and nothing here needs a compiler.

```bash
node homebrew/formula.mjs 0.2.1                        # to stdout
node homebrew/formula.mjs 0.2.1 --out Formula/kosong.rb
```

It refuses to write a formula that is missing a platform. A formula short one
target does not fail to build — it installs on three platforms and tells the
fourth that kosong is unavailable, which is a worse thing to publish than an
error.

## The tap

A Homebrew tap must be its own repository, named `homebrew-<something>`. The
tap is <https://github.com/khalilhimura/homebrew-tap>, and installing is:

```bash
brew tap khalilhimura/tap
brew trust khalilhimura/tap
brew install kosong
```

**The `brew trust` line is required and easy to miss.** Homebrew refuses to
load formulae from a tap outside its own repositories until it is trusted, and
`brew install` stops with an error rather than installing anything.

This was found late and only by luck. Every check before it used a tap created
locally with `brew tap-new`, which Homebrew trusts implicitly — `style`,
`fetch`, `install` and `test` all passed. The first install from a tap *cloned
from GitHub* failed, which is the only sequence a user ever performs. Verify a
tap by untapping and retapping from the remote; a local tap proves the formula,
not the install.

The repository holds `Formula/kosong.rb` and a trimmed `tests.yml`.
`brew tap-new` also scaffolds `publish.yml`, which exists to pull bottles;
kosong ships a prebuilt binary and compiles nothing, so it was deleted rather
than left looking load-bearing.

## Getting the formula into the tap, and what each way costs

**Manual, and what is in use.** After a release, regenerate and commit:

```bash
node homebrew/formula.mjs <version> \
  --out "$(brew --repository)/Library/Taps/khalilhimura/homebrew-tap/Formula/kosong.rb"
```

One command and one commit per release. No credential exists anywhere. For a
project that releases occasionally this is not a compromise, it is the cheapest
correct answer.

**The tap pulls.** A scheduled workflow *in the tap repository* checks for a
newer GitHub Release and updates its own formula, using that repository's own
`GITHUB_TOKEN`. Automatic, and still no cross-repository credential. Costs a
workflow to maintain in a second repository, and the formula lags a release by
up to the polling interval.

**This repository pushes.** `release.yml` writes the formula into the tap
directly. It is the most immediate and the only one that needs a **personal
access token with write access to another repository, stored in this
repository's secrets**.

That is precisely the kind of long-lived credential the npm publishing setup
was built to avoid: `release.yml` publishes five packages to npm over OIDC with
no token anywhere, and adding a PAT for Homebrew would put back what that
removed. Recommended against unless the per-release step becomes a real burden,
and then prefer the tap pulling.

## What has been verified

Against the real 0.2.1 release, on Homebrew 6.0.12:

- `brew style` — no offences
- `brew fetch` — every URL resolves and every embedded checksum matches
- `brew install` — installs from the release archive
- `brew test` — `--version` reports the formula's version, and `kosong new`
  writes a page that is then read back

Done through a throwaway local tap, which was removed afterwards. What has
**not** been tested is the Linux half: `brew fetch` verified those checksums,
but no Linux machine has installed through Homebrew. The binaries are the same
ones `install-smoke.yml` runs on Linux from npm, so the risk sits in the
formula's platform selection rather than in the binary.
