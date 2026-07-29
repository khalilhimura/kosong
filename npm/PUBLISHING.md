# Publishing to npm

The `npm` job in `.github/workflows/release.yml` does steps 1–3 below. Steps 4
and 5 are yours, deliberately.

## The order is not a detail

Publishing is not atomic. Each `npm publish` is a separate request, and a
half-published release is worse than a failed one: `kosong@0.2.0` existing
while `@thefutureissolo/kosong-linux-x64-gnu@0.2.0` does not means every Linux install
resolves, succeeds, and then cannot run. npm versions are immutable, so the
repair is a new version rather than a fix.

1. **Platform packages first**, in any order. They depend on nothing.
2. **The launcher last.** Its `optionalDependencies` pin exact versions, so it
   must not exist before the things it names.
3. **Everything under `--tag next`**, never straight to `latest`.
4. **Verify a real install**, on macOS and on Linux.
5. **Then move the tag:** `npm dist-tag add kosong@<version> latest`.

Step 3 is what makes this recoverable. `latest` is what `npm install -g kosong`
resolves, so until the tag moves, a partial publish is invisible to users and
the fix is to publish the missing package and move the tag once.

**Steps 4 and 5 are not automated on purpose.** A green workflow says the
tarballs uploaded, not that the binary inside them runs. Nothing about the
publish proves that `kosong --version` works on a machine that is not a GitHub
runner, and once `latest` moves, it is users who find out.

## What the workflow already guarantees

- **A release is all-or-nothing.** `build.mjs` refuses to write the launcher at
  all if any released target is missing from the artifacts, so a launcher whose
  `optionalDependencies` omit a platform cannot reach the publish step.
- **Re-running is safe.** Each publish checks `npm view <name>@<version>` first
  and skips what is already there, so a run that failed halfway can be retried
  rather than needing a new version.
- **A rehearsal produces something to inspect.** `workflow_dispatch` builds and
  packs but does not publish, and uploads the tarballs as `npm-packages`.
- **The version cannot drift.** `build.mjs` reads `[workspace.package]` from
  `Cargo.toml`. `--version` may add a prerelease suffix and nothing else — the
  qualified version must still be the manifest's, which is the same rule
  `release.yml` applies to the tag, enforced in a second place so neither can
  be skipped alone.

## Authentication

The job uses npm **trusted publishing** (OIDC) rather than an `NPM_TOKEN`
secret, with `--provenance`. `release.yml` already had `id-token: write` for
build provenance, so this added no new permission and there is no long-lived
credential in the repository to leak.

The runner's bundled npm is too old for trusted publishing, so the job installs
`npm@latest` first. Without that, publishing falls back to looking for a token
that is deliberately not there.

This applies from the **second** release onward. See below for why the first
one is different.

## The first release cannot use the workflow, and that is not a bug

Trusted publishing is configured per package, and npm's own prerequisites for
`npm trust` say it plainly: **"The package you're configuring must already
exist on the npm registry."** Five packages that have never been published
cannot be configured, so the first publish has to happen some other way.

Do it by hand, from a machine, once. The alternative — a publish token in
repository secrets for one release — puts a credential in CI that this project
has otherwise avoided entirely, to save one manual afternoon.

Checked against the registry on 2026-07-29: `kosong` is unclaimed, and nothing
is published under the `@thefutureissolo` scope. Re-check both before starting; if the
unscoped name has gone, the launcher, the docs and this file all change.

1. **Create or confirm the `@thefutureissolo` org** on npmjs, owned by the publishing account. Free
   for public packages.
2. **Enable two-factor authentication** on that account. `npm trust` requires
   it, and so does publishing to a new scope.
3. **Generate the packages** for the release version:

   ```bash
   cargo build --release
   node npm/verify.mjs          # 15 checks, and it builds npm/dist as a side effect
   ```

   For a real release use the four published binaries rather than one local
   build — download the release archives and pass `--artifacts`, exactly as the
   workflow does. A local build produces only the host's platform, and
   `build.mjs` will refuse to write a launcher from an incomplete set.

4. **Publish, platform packages first, launcher last, all to `next`:**

   ```bash
   for p in npm/dist/@*/* npm/dist/kosong; do
     ( cd "$p" && npm publish --access public --tag next )
   done
   ```

   `--access public` is not optional. Scoped packages are private by default
   and the first publish fails without it.

5. **Configure trusted publishing on all five** (needs npm 11.15.0 or later):

   ```bash
   # Read the names out of what was built, rather than retyping five of them.
   for p in $(node -p "
     const m = require('./npm/dist/kosong/package.json');
     [...Object.values(m.kosong.platforms), m.name].join(' ')
   "); do
     npm trust github "$p" --file release.yml --repo khalilhimura/kosong \
       --allow-publish --yes
   done
   ```

6. **Verify a real install, then move `latest`** — steps 4 and 5 of the order
   above. They do not become optional just because this release was manual.

Every release after this one runs the workflow with no token anywhere.

### What this costs, stated plainly

The npm packages for that first version carry **no provenance attestation**.
`--provenance` needs a CI runner with an OIDC token, and a laptop is not one.
The GitHub Release tarballs for the same version are still attested by the
`build` job, so only the npm copies of a single version are affected, and every
version after is attested on both channels.

Publishing by hand first does not break the tagged release. The workflow skips
any package whose exact version is already on the registry, so tagging `v0.2.0`
after hand-publishing `0.2.0` runs green and does nothing.

## Verifying locally

```bash
cargo build --release
node npm/build.mjs
```

Then, from a scratch directory:

```bash
npm install -g ./npm/dist/@thefutureissolo/kosong-<your-platform> ./npm/dist/kosong
kosong --version
```

**Read what that actually links.** Installing both packages as top-level
globals lets the platform package's own bin win, so `kosong` runs the binary
directly and the launcher is never exercised — a test that passes without
testing anything. It happened during Phase A. To exercise the launcher, build
the layout npm really produces: the launcher in `node_modules/kosong`, the
platform package beside it under `node_modules/@thefutureissolo/`, and the `bin` symlink
pointing at `kosong/bin/kosong.js`.

If `kosong --version` prints a version that is not in `Cargo.toml`, the shell
found a different `kosong` on `PATH` — likely one from `install.sh`.
