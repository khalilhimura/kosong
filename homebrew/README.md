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

## The formula cannot live here

A Homebrew tap must be its own repository, named `homebrew-<something>`. So
`khalilhimura/homebrew-kosong` would give:

```bash
brew tap khalilhimura/kosong
brew install kosong
```

That repository does not exist yet, and creating it is not something this
repository can do.

## Getting the formula into the tap, and what each way costs

**Manual, and the recommended start.** After a release, run the generator, and
commit the result to the tap:

```bash
node homebrew/formula.mjs <version> --out Formula/kosong.rb   # in the tap repo
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
