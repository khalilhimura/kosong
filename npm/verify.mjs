#!/usr/bin/env node
// Checks that the generated npm packages install and run.
//
// # Why this exists
//
// The npm layer has no type checker, no linter, and no `cargo test`. Both bugs
// found while building it were invisible to every check in `ci.yml`:
//
// - the platform package declared `bin`, so a global install linked `kosong`
//   to the raw binary and the launcher never ran — a test that passed while
//   testing nothing;
// - the launcher used `spawnSync`, which blocks the event loop so signal
//   handlers never run while still suppressing Node's default terminate, so a
//   directed `kill -TERM` was ignored and `kosong preview` needed SIGKILL.
//
// Each has an assertion below named after it. Neither would be caught by
// reading the code, and neither shows up in a passing `npm pack`.
//
// # Usage
//
//   cargo build --release
//   node npm/verify.mjs
//
// Exits non-zero on the first failure, naming what was expected.

import { spawn, spawnSync } from 'node:child_process';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import { workspaceVersion } from './cargo.mjs';

const HERE = path.dirname(fileURLToPath(import.meta.url));
const ROOT = path.resolve(HERE, '..');
const POSIX = process.platform !== 'win32';

let failures = 0;

function check(name, body) {
  try {
    body();
    console.log(`  ok    ${name}`);
  } catch (error) {
    failures += 1;
    console.log(`  FAIL  ${name}`);
    console.log(`        ${error.message.split('\n').join('\n        ')}`);
  }
}

/// Runs a command to completion, capturing output. Never a command string.
function run(command, args, options = {}) {
  return spawnSync(command, args, { encoding: 'utf8', cwd: ROOT, ...options });
}

function node(args, options = {}) {
  return run(process.execPath, args, options);
}

// ---------------------------------------------------------------------------
// The generator's guards
// ---------------------------------------------------------------------------

const VERSION = workspaceVersion(ROOT);

console.log(`\nnpm packaging, kosong ${VERSION}\n`);

check('generator refuses a version that disagrees with Cargo.toml', () => {
  const result = node(['npm/build.mjs', '--version', '99.0.0']);
  assert.notEqual(result.status, 0, 'expected a non-zero exit');
  assert.match(result.stderr, /disagrees with Cargo\.toml/);
});

check('generator accepts a prerelease of the manifest version', () => {
  const result = node(['npm/build.mjs', '--version', `${VERSION}-rc.1`]);
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, new RegExp(`kosong ${VERSION.replace(/\./g, '\\.')}-rc\\.1`));
});

check('generator refuses an incomplete release', () => {
  // An artifacts directory holding fewer targets than the matrix releases. A
  // launcher built from this would install cleanly on the missing platform and
  // then be unable to run, and npm versions are immutable.
  const artifacts = fs.mkdtempSync(path.join(os.tmpdir(), 'kosong-partial-'));
  const stem = `kosong-${VERSION}-x86_64-unknown-linux-gnu`;
  fs.mkdirSync(path.join(artifacts, stem));
  fs.writeFileSync(path.join(artifacts, stem, 'kosong'), 'stand-in');

  // An output directory of its own, because a failed build now preserves
  // whatever was there before. Pointed at the shared `npm/dist`, "no launcher
  // exists" would be satisfied by a launcher left behind from an earlier
  // passing check, and the assertion would hold without testing anything.
  const out = fs.mkdtempSync(path.join(os.tmpdir(), 'kosong-out-'));
  fs.rmSync(out, { recursive: true, force: true });

  const result = node([
    'npm/build.mjs', '--artifacts', artifacts, '--version', VERSION, '--out', out,
  ]);
  assert.notEqual(result.status, 0, 'expected a non-zero exit');
  assert.match(result.stderr, /incomplete release/);
  assert.ok(
    !fs.existsSync(path.join(out, 'kosong')),
    'the launcher was written anyway; the guard must precede emission',
  );
});

check('a failed build leaves the last good one alone', () => {
  // Everything that can throw runs before the output directory is touched.
  // Clearing it first would mean a mistyped `--version` destroys a working
  // build and then declines to replace it, which is the worst of both.
  assert.equal(node(['npm/build.mjs']).status, 0, 'the baseline build failed');
  const before = fs.readdirSync(path.join(HERE, 'dist'));

  assert.notEqual(node(['npm/build.mjs', '--version', '99.0.0']).status, 0);
  assert.deepEqual(fs.readdirSync(path.join(HERE, 'dist')), before);
});

// ---------------------------------------------------------------------------
// A real install
// ---------------------------------------------------------------------------

const built = node(['npm/build.mjs']);
if (built.status !== 0) {
  console.error(`\nCould not generate the packages:\n${built.stderr}`);
  console.error('Build a binary first:  cargo build --release\n');
  process.exit(1);
}

/// The platform package for the host, which is the only one with a real binary.
function hostPackage() {
  const scope = path.join(HERE, 'dist', '@kosong');
  const entries = fs.readdirSync(scope);
  const wanted = entries.filter((entry) => entry.includes(process.arch));
  assert.ok(wanted.length === 1, `expected one package for ${process.arch}, saw ${entries}`);
  return path.join(scope, wanted[0]);
}

check('platform package declares no bin', () => {
  // Regression guard. Declaring one makes npm link `kosong` to the raw binary
  // whenever this package is installed directly, bypassing the launcher — and
  // a smoke test that runs the binary instead of the launcher passes while
  // verifying nothing at all.
  const manifest = JSON.parse(fs.readFileSync(path.join(hostPackage(), 'package.json'), 'utf8'));
  assert.equal(manifest.bin, undefined, 'platform packages must not declare a bin entry');
});

/// Builds the layout npm actually produces, from packed tarballs.
///
/// Packing and unpacking is the point: it is what proves the executable bit
/// survives, which is the reason the `bin` entry could be dropped.
function install() {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'kosong-npm-'));
  const modules = path.join(root, 'node_modules');
  fs.mkdirSync(path.join(modules, '@kosong'), { recursive: true });

  for (const [source, destination] of [
    [hostPackage(), path.join(modules, '@kosong', path.basename(hostPackage()))],
    [path.join(HERE, 'dist', 'kosong'), path.join(modules, 'kosong')],
  ]) {
    const packed = run('npm', ['pack', '--pack-destination', root], { cwd: source });
    assert.equal(packed.status, 0, packed.stderr);
    const tarball = path.join(root, packed.stdout.trim().split('\n').pop());

    const extractTo = fs.mkdtempSync(path.join(root, 'x-'));
    const extracted = run('tar', ['-xzf', tarball, '-C', extractTo]);
    assert.equal(extracted.status, 0, extracted.stderr);
    fs.renameSync(path.join(extractTo, 'package'), destination);
  }

  return path.join(modules, 'kosong', 'bin', 'kosong.js');
}

const LAUNCHER = install();

check('the binary is executable after a pack and unpack round trip', () => {
  const binary = path.join(
    path.dirname(LAUNCHER),
    '..',
    '..',
    '@kosong',
    path.basename(hostPackage()),
    'bin',
    process.platform === 'win32' ? 'kosong.exe' : 'kosong',
  );
  assert.ok(fs.existsSync(binary), `no binary at ${binary}`);
  if (POSIX) {
    const mode = fs.statSync(binary).mode & 0o111;
    assert.notEqual(mode, 0, 'the executable bit did not survive npm pack');
  }
});

check('the launcher runs the real binary', () => {
  const result = node([LAUNCHER, '--version']);
  assert.equal(result.status, 0, result.stderr);
  assert.equal(result.stdout.trim(), `kosong ${VERSION}`);
});

check('machine-readable output passes through unaltered', () => {
  const result = node([LAUNCHER, 'doctor', '--json']);
  const report = JSON.parse(result.stdout);
  assert.equal(report.schema, 1);
});

check('a usage error keeps its exit code', () => {
  const result = node([LAUNCHER, 'no-such-command']);
  assert.equal(result.status, 2, `expected clap's exit code 2, saw ${result.status}`);
});

// ---------------------------------------------------------------------------
// Signals
// ---------------------------------------------------------------------------

/// Replaces the real binary with a stand-in, so the child's behaviour is known.
///
/// `exec` matters: without it the child is a shell that waits for its own
/// child before acting on a signal, and the test measures the shell rather
/// than the launcher. That produced a false failure during Phase A.
/// `async`, and the `await` below is load-bearing. A synchronous version
/// restores the real binary the instant `body` returns its promise — before
/// the spawned child has started — so every signal case silently measured
/// real `kosong` instead of the stand-in. The tell was a usage exit of 2 where
/// 42 was expected; had the assertions been looser it would have passed while
/// testing nothing, which is the same failure this file exists to catch.
async function withStandIn(script, body) {
  const binary = path.join(
    path.dirname(LAUNCHER),
    '..',
    '..',
    '@kosong',
    path.basename(hostPackage()),
    'bin',
    process.platform === 'win32' ? 'kosong.exe' : 'kosong',
  );
  const original = fs.readFileSync(binary);
  fs.writeFileSync(binary, script);
  fs.chmodSync(binary, 0o755);
  try {
    return await body();
  } finally {
    fs.writeFileSync(binary, original);
    fs.chmodSync(binary, 0o755);
  }
}

function exitStatusOf(script, signal) {
  return withStandIn(
    script,
    () =>
      new Promise((resolve) => {
        const child = spawn(process.execPath, [LAUNCHER], { stdio: 'ignore' });
        child.on('exit', (code, killedBy) => resolve({ code, killedBy }));
        if (signal) setTimeout(() => child.kill(signal), 500);
      }),
  );
}

if (POSIX) {
  for (const signal of ['SIGINT', 'SIGTERM', 'SIGHUP', 'SIGQUIT']) {
    const { killedBy } = await exitStatusOf('#!/bin/sh\nexec sleep 30\n', signal);
    check(`${signal} reaches the real process`, () => {
      // A launcher that ignores the signal leaves the child running and this
      // resolves 30 seconds later with a normal exit, which is the bug.
      assert.equal(killedBy, signal, `expected the launcher to die by ${signal}, saw ${killedBy}`);
    });
  }

  const { code } = await exitStatusOf('#!/bin/sh\nexit 42\n', null);
  check('an exit code passes through unchanged', () => {
    assert.equal(code, 42, `expected 42, saw ${code}`);
  });
}

// ---------------------------------------------------------------------------

console.log('');
if (failures > 0) {
  console.error(`${failures} check${failures === 1 ? '' : 's'} failed.\n`);
  process.exit(1);
}
console.log('All checks passed.\n');
