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

Step 3 is what makes this recoverable — **from the second release onward.**
`latest` is what `npm install -g kosong` resolves, so until the tag moves, a
partial publish of a new version is invisible to users and the fix is to publish
the missing package and move the tag once.

### `--tag next` does not protect a package's first publish

npm sets `latest` on a package's **first** published version whatever `--tag`
says. Observed on the 0.2.0 publish: every one of the five came out of
`npm publish --tag next` carrying `next` *and* `latest`, and
`npm install -g kosong` resolved the moment the launcher landed — not when the
tag was moved afterwards.

So during a bootstrap the publish order is not a nicety with a safety net
behind it; it is the only thing standing between a user and a broken install.
Launcher last, always. Publish it first with a platform package missing and
`latest` points at something that installs cleanly and cannot run, with no tag
to hold it back.

This applies to any new package, so it will apply again the day a platform is
added — a new `@thefutureissolo/kosong-win32-x64` is a first publish even
though kosong is not.

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
   node npm/verify.mjs          # the packaging checks, and it builds npm/dist as a side effect
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

5. **Configure trusted publishing on all five.**

   **Check the npm version first, and do not skip the check.** `npm trust`
   arrived in npm 11.15.0. An older npm answers an unknown command by printing
   `Unknown command: "trust"` and **exiting 0** — so a loop over five packages
   reports success while configuring nothing at all, and `--yes` means there is
   not even a prompt missing to notice.

   That is exactly what happened here: npm 10.9.8 silently configured nothing,
   and the failure surfaced only at the next tagged release as

   ```
   npm error 404 ... could not be found or you do not have permission to access it
   ```

   which reads like a permissions problem on a package that plainly exists,
   rather than a setup step that never ran.

   **Authenticate before the loop, interactively.** `npm trust` opens a browser
   to sign in. Inside a `for` loop with `--yes` that prompt arrives on the first
   package and the remaining four run past whatever happens to it. Configuring
   all five in one loop looked like it worked and produced nothing:
   `npm trust list` later reported `No trust configurations found`.

   So do one package by hand, complete the browser sign-in, and only then loop
   over the rest.

   **`--file` takes the bare filename**, `release.yml`, not a path. Confirmed
   from `npm trust`'s own confirmation screen, which resolves it to
   `github.com/khalilhimura/kosong/blob/HEAD/.github/workflows/release.yml`
   before asking you to proceed. Read that screen: it is the one place the
   repository, the workflow and the permission are shown together before
   anything is created.

   ```bash
   if [ "$(printf '11.15.0\n%s\n' "$(npm --version)" | sort -V | head -1)" != "11.15.0" ]; then
     echo "npm $(npm --version) is too old for \`npm trust\` (needs 11.15.0+)." >&2
     echo "Run: npm install -g npm@latest" >&2
     exit 1
   fi

   # One, interactively. Finish the browser sign-in it opens.
   npm trust github @thefutureissolo/kosong-darwin-arm64 \
     --file release.yml --repo khalilhimura/kosong --allow-publish

   # Confirm that one took before doing anything else.
   npm trust list @thefutureissolo/kosong-darwin-arm64

   # Then the remaining four, on the authenticated session.
   for p in @thefutureissolo/kosong-darwin-x64 \
            @thefutureissolo/kosong-linux-x64-gnu \
            @thefutureissolo/kosong-linux-arm64-gnu \
            kosong; do
     npm trust github "$p" --file release.yml --repo khalilhimura/kosong \
       --allow-publish --yes
   done

   # Prove it took, rather than trusting five commands that cannot fail loudly.
   for p in $(node -p "
     const m = require('./npm/dist/kosong/package.json');
     [...Object.values(m.kosong.platforms), m.name].join(' ')
   "); do
     echo "--- $p"; npm trust list "$p"
   done
   ```

   The read-back is the point. The configuring commands cannot be trusted to
   have done anything, for the reasons above; only reading the configuration
   back proves
   it exists.

6. **Verify a real install, then move `latest`** — steps 4 and 5 of the order
   above. They do not become optional just because this release was manual.

Every release after this one runs the workflow with no token anywhere.

### The workflow knows this, and does not need switching on

Tagging `v0.2.0` does not produce a red run. The `npm` job asks the registry
whether `kosong` exists before it tries to publish, and if it does not, it skips
publishing and writes these instructions into the run summary instead.

`kosong` is the right thing to probe because it is published **last** — if the
launcher is there, the platform packages it names are too. Only a `404` counts
as "not yet"; any other answer, including a network failure, fails the step,
because silently skipping a real publish is worse than a red run.

So there is nothing to turn on afterwards. Once the five packages exist and
`npm trust` has run, the next tag publishes on its own.

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
