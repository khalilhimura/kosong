#!/usr/bin/env node
// Generates the npm packages that carry the `kosong` binary.
//
// # Why there are N+1 packages
//
// npm has no way to fetch a different file per platform within one package.
// The convention every serious native CLI on npm settles on — esbuild, Biome,
// SWC, Rollup — is one thin package that users install, depending on one
// platform package per target through `optionalDependencies`. npm reads the
// `os`, `cpu`, and `libc` fields, installs exactly the one that matches, and
// silently skips the rest.
//
// The alternative is a `postinstall` script that downloads the binary. It was
// rejected deliberately: it breaks under `--ignore-scripts`, breaks offline,
// breaks behind a proxy that inspects TLS, and would mean re-implementing the
// checksum verification `install.sh` already does — turning install time into
// a live network trust decision. `spec/threat-model-v1.md` §5 trusts npm
// dependencies *at their pinned versions*, and platform packages stay inside
// that sentence. A postinstall download does not.
//
// # Usage
//
//   node npm/build.mjs                      # from ./target, for local testing
//   node npm/build.mjs --artifacts <dir>    # from release archives, for CI
//   node npm/build.mjs --out <dir>          # default: npm/dist
//
// Nothing here publishes. See npm/PUBLISHING.md for the publish order, which
// matters more than it looks.

import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import { workspaceVersion } from './cargo.mjs';

const HERE = path.dirname(fileURLToPath(import.meta.url));
const ROOT = path.resolve(HERE, '..');

/// The npm scope, and the only place it is written down.
///
/// Platform packages are `@thefutureissolo/kosong-<platform>-<arch>[-<libc>]`.
/// The product is named inside the package because the scope is an umbrella
/// for more than one project: `cli-darwin-arm64` under a company scope is a
/// CLI of nothing in particular.
///
/// The launcher is deliberately **not** scoped. It stays `kosong`, so the
/// command a beginner types is `npm install -g kosong`, and it is the only
/// name of the six that a user ever has to know.
const SCOPE = '@thefutureissolo';

// ---------------------------------------------------------------------------
// Targets
// ---------------------------------------------------------------------------

// The one place a Rust triple is mapped to npm's platform vocabulary.
//
// `keyFor` produces the string `bin/kosong.js` computes at runtime from
// `process.platform`, `process.arch`, and the libc probe. The launcher does not
// rebuild package names from it — the key-to-package map is written into the
// launcher's own manifest by `emitMainPackage`, so the scope and the naming
// scheme appear exactly once, here. Adding a row is the only edit a new target
// needs.
//
// `released` marks a triple that `.github/workflows/release.yml` actually
// builds. Windows is listed but not released: the name is reserved so that
// shipping it later is a matrix line rather than a rename, per
// `docs/superpowers/specs/2026-07-29-windows-support-design.md`.
const TARGETS = [
  { triple: 'aarch64-apple-darwin', os: 'darwin', cpu: 'arm64', released: true },
  { triple: 'x86_64-apple-darwin', os: 'darwin', cpu: 'x64', released: true },
  { triple: 'x86_64-unknown-linux-gnu', os: 'linux', cpu: 'x64', libc: 'glibc', released: true },
  { triple: 'aarch64-unknown-linux-gnu', os: 'linux', cpu: 'arm64', libc: 'glibc', released: true },
  { triple: 'x86_64-unknown-linux-musl', os: 'linux', cpu: 'x64', libc: 'musl', released: true },
  { triple: 'aarch64-unknown-linux-musl', os: 'linux', cpu: 'arm64', libc: 'musl', released: true },
  { triple: 'x86_64-pc-windows-msvc', os: 'win32', cpu: 'x64', released: false },
];

/// The package name suffix for a target, and the runtime key the launcher matches.
///
/// glibc is spelled in the name because musl will one day sit beside it, and a
/// package called `cli-linux-x64` that silently means glibc is the kind of
/// thing that is only ambiguous once it is too late to rename.
function keyFor(target) {
  const libc = target.libc ? `-${target.libc === 'glibc' ? 'gnu' : target.libc}` : '';
  return `${target.os}-${target.cpu}${libc}`;
}

function packageNameFor(target) {
  return `${SCOPE}/kosong-${keyFor(target)}`;
}

function binaryNameFor(target) {
  return target.os === 'win32' ? 'kosong.exe' : 'kosong';
}

/// The C library this host uses, or null where the concept does not apply.
///
/// Same probe the launcher uses: a glibc runtime names its version in the
/// diagnostic report and a musl one does not.
function hostLibc() {
  if (process.platform !== 'linux') return null;
  try {
    return process.report.getReport().header.glibcVersionRuntime ? 'glibc' : 'musl';
  } catch {
    return 'glibc';
  }
}

/// Whether a target describes the machine running this script.
///
/// libc is part of the answer, and an earlier version of this said it was not:
/// "no local build produces both". That held while linux had one row per
/// architecture. With musl added there are two, and on a Linux x64 host both
/// claimed the untargeted `target/release` — emitting two packages from one
/// glibc binary, one of them labelled musl. `verify.mjs` caught it.
function isHost(target) {
  if (target.os !== process.platform || target.cpu !== process.arch) return false;
  return (target.libc ?? null) === hostLibc();
}

// ---------------------------------------------------------------------------
// Inputs
// ---------------------------------------------------------------------------

/// Locates the built binary for a target.
///
/// From an extracted release archive when `--artifacts` is given, and from the
/// local cargo target directory otherwise. The local path is what makes this
/// testable on one machine without cutting a release.
function findBinary(target, version, artifactsDir) {
  const name = binaryNameFor(target);

  if (!artifactsDir) {
    const candidates = [path.join(ROOT, 'target', target.triple, 'release', name)];
    // `cargo build --release` with no `--target` writes to `target/release`,
    // which is what a developer will have built. Only the host's row may claim
    // it, or a local build would package this machine's binary as every
    // platform's.
    if (isHost(target)) candidates.push(path.join(ROOT, 'target', 'release', name));
    return candidates.find((candidate) => fs.existsSync(candidate)) ?? null;
  }

  const stem = `kosong-${version}-${target.triple}`;

  // Already extracted?
  const extracted = path.join(artifactsDir, stem, name);
  if (fs.existsSync(extracted)) return extracted;

  const archive = path.join(artifactsDir, `${stem}.tar.gz`);
  if (!fs.existsSync(archive)) return null;

  // argv array, never a command string — the same rule the product follows,
  // applied to its build tooling for the same reason.
  const result = spawnSync('tar', ['-xzf', archive, '-C', artifactsDir], { stdio: 'inherit' });
  if (result.status !== 0) throw new Error(`could not extract ${archive}`);

  return fs.existsSync(extracted) ? extracted : null;
}

// ---------------------------------------------------------------------------
// Emit
// ---------------------------------------------------------------------------

function writeJson(file, value) {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, `${JSON.stringify(value, null, 2)}\n`);
}

/// Fields every package shares, so the registry pages agree with each other.
function common(version) {
  return {
    version,
    license: 'Apache-2.0',
    repository: { type: 'git', url: 'git+https://github.com/khalilhimura/kosong.git' },
    homepage: 'https://kosong.thefutureissolo.com',
    bugs: { url: 'https://github.com/khalilhimura/kosong/issues' },
    engines: { node: '>=18' },
  };
}

function emitPlatformPackage(target, version, outDir, binarySource) {
  const name = packageNameFor(target);
  const dir = path.join(outDir, ...name.split('/'));
  const binName = binaryNameFor(target);

  const manifest = {
    name,
    description: `The kosong binary for ${target.os} ${target.cpu}.`,
    ...common(version),
    os: [target.os],
    cpu: [target.cpu],
    // Only meaningful on linux. npm, pnpm and yarn all read it; on a manager
    // that does not, the package installs and the launcher's own check is what
    // catches a musl host.
    ...(target.libc ? { libc: [target.libc] } : {}),
    files: ['bin', 'LICENSE'],
    // Deliberately no `bin` entry, matching what esbuild and Biome publish.
    //
    // Declaring one looks attractive — it makes npm mark the file executable
    // on extraction. But it also means that anyone who installs this package
    // directly, rather than receiving it as the launcher's optional
    // dependency, gets `kosong` linked to the raw binary and bypasses the
    // launcher entirely. That was observed during Phase A verification: a
    // global install of both packages linked this one's bin and the launcher
    // was never exercised, so the test passed without testing anything.
    //
    // The executable bit survives without it. Verified directly: `npm pack` of
    // a 0755 file emits 0755 with no `bin` declared. `bin/kosong.js` restores
    // the bit anyway if some other unpacking path drops it.
    preferUnplugged: true,
  };

  writeJson(path.join(dir, 'package.json'), manifest);

  fs.mkdirSync(path.join(dir, 'bin'), { recursive: true });
  const dest = path.join(dir, 'bin', binName);
  fs.copyFileSync(binarySource, dest);
  fs.chmodSync(dest, 0o755);
  fs.copyFileSync(path.join(ROOT, 'LICENSE'), path.join(dir, 'LICENSE'));

  const digest = createHash('sha256').update(fs.readFileSync(dest)).digest('hex');
  return { name, dir, digest, bytes: fs.statSync(dest).size };
}

function emitMainPackage(version, outDir, built) {
  const dir = path.join(outDir, 'kosong');

  // Only what was actually built is depended on. A dependency on a platform
  // package that was never published is an install failure for everyone, not
  // just that platform, because npm resolves optional dependencies before it
  // decides whether they apply.
  const optionalDependencies = Object.fromEntries(
    built.map(({ name }) => [name, version]),
  );

  const platforms = built.map(({ target }) => target);

  const manifest = {
    name: 'kosong',
    description:
      'Open a terminal. Write one page. Publish it. Learn what happened.',
    ...common(version),
    keywords: ['cli', 'markdown', 'static-site', 'okf', 'publishing', 'beginner'],
    // npm refuses the install with EBADPLATFORM on anything not listed, which
    // is a clear failure at install time rather than a confusing one later
    // when the launcher cannot find a binary.
    os: [...new Set(platforms.map((t) => t.os))],
    cpu: [...new Set(platforms.map((t) => t.cpu))],
    bin: { kosong: 'bin/kosong.js' },
    files: ['bin', 'LICENSE'],
    optionalDependencies,
    // The launcher's lookup table, so it never rebuilds a package name from
    // parts. Before this, the scope was written in `build.mjs`, twice in
    // `bin/kosong.js` and five times in `verify.mjs` — five places to miss when
    // the org changes, and a miss produces a launcher that resolves nothing on
    // every platform. Now the map is generated and the launcher only reads it.
    kosong: {
      platforms: Object.fromEntries(
        built.map(({ target, name }) => [keyFor(target), name]),
      ),
    },
  };

  writeJson(path.join(dir, 'package.json'), manifest);

  fs.mkdirSync(path.join(dir, 'bin'), { recursive: true });
  fs.copyFileSync(path.join(HERE, 'bin', 'kosong.js'), path.join(dir, 'bin', 'kosong.js'));
  fs.copyFileSync(path.join(ROOT, 'LICENSE'), path.join(dir, 'LICENSE'));
  fs.copyFileSync(path.join(HERE, 'README.md'), path.join(dir, 'README.md'));

  return { name: 'kosong', dir };
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

function parseArgs(argv) {
  const options = { artifacts: null, out: path.join(HERE, 'dist'), version: null };
  for (let i = 0; i < argv.length; i += 1) {
    if (argv[i] === '--artifacts') options.artifacts = path.resolve(argv[++i]);
    else if (argv[i] === '--out') options.out = path.resolve(argv[++i]);
    else if (argv[i] === '--version') options.version = argv[++i];
    else throw new Error(`unknown argument: ${argv[i]}`);
  }
  return options;
}

/// The version to publish, with `Cargo.toml` still the authority.
///
/// `--version` exists for one reason: a prerelease tag. `release.yml` names
/// archives from the tag, so `v0.1.1-rc.1` produces
/// `kosong-0.1.1-rc.1-<triple>.tar.gz` while the manifest says `0.1.1`. Without
/// the override the generator looks for files that are not there; with a naive
/// override it would happily publish `0.1.1` twice, and npm versions are
/// immutable.
///
/// So the override may add a prerelease suffix and may not do anything else.
/// The qualified version must still be the manifest's — the same rule
/// `release.yml` applies to the tag, enforced here so neither can be skipped
/// on its own.
function resolveVersion(override) {
  const manifest = workspaceVersion(ROOT);
  if (!override) return manifest;

  const base = override.split('-')[0];
  if (base !== manifest) {
    throw new Error(
      `--version ${override} disagrees with Cargo.toml (${manifest}).\n` +
        'Bump the workspace version, or pass the version that matches it.',
    );
  }
  return override;
}

function main() {
  const { artifacts, out, version: override } = parseArgs(process.argv.slice(2));
  const version = resolveVersion(override);

  // Discovery first, and nothing written yet. Everything below that can throw
  // does so before the output directory is touched, so a run that fails on a
  // mistyped `--version` or an incomplete artifacts directory leaves the last
  // good build intact instead of clearing it and then refusing to replace it.
  const found = [];
  const missing = [];

  for (const target of TARGETS) {
    if (!target.released) continue;

    const binary = findBinary(target, version, artifacts);
    if (binary) found.push({ target, binary });
    else missing.push(target.triple);
  }

  if (found.length === 0) {
    throw new Error(
      artifacts
        ? `no release archives found in ${artifacts}`
        : 'no binaries in ./target. Build one first:\n  cargo build --release',
    );
  }

  // A partial build is legitimate locally and never legitimate in CI, where a
  // launcher whose optionalDependencies omit a platform is a silent
  // regression: that platform installs cleanly and then cannot run.
  if (missing.length > 0 && artifacts) {
    throw new Error(`refusing to build an incomplete release. missing: ${missing.join(', ')}`);
  }

  fs.rmSync(out, { recursive: true, force: true });

  const built = found.map(({ target, binary }) => ({
    target,
    ...emitPlatformPackage(target, version, out, binary),
  }));
  const main = emitMainPackage(version, out, built);

  console.log(`kosong ${version} → ${path.relative(process.cwd(), out)}\n`);
  for (const { name, digest, bytes } of built) {
    console.log(`  ${name.padEnd(30)} ${(bytes / 1024 / 1024).toFixed(1)} MiB  ${digest.slice(0, 16)}`);
  }
  console.log(`  ${main.name.padEnd(30)} launcher\n`);

  if (missing.length > 0) {
    console.log(`  Local build, missing: ${missing.join(', ')}`);
    console.log('  Fine for testing. CI builds every target or fails.\n');
  }
}

try {
  main();
} catch (error) {
  // A stack trace is noise here. Every throw above is a condition with a
  // written explanation, and the person reading this in a CI log wants the
  // sentence, not the frames.
  process.stderr.write(`\nCould not build the npm packages: ${error.message}\n\n`);
  process.exit(1);
}
