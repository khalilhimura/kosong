#!/usr/bin/env node
// The launcher installed as `kosong` by `npm install -g kosong`.
//
// CommonJS on purpose. The package declares no `type`, so this file is CJS,
// and `require.resolve` is how a platform package is located without guessing
// at node_modules layout — npm hoists, pnpm does not, and Yarn PnP has no
// directories at all. Resolution is the only thing all three agree on.
//
// This costs one Node startup, roughly 30ms, on every `kosong` invocation.
// That is the price of npm distribution and it is why `install.sh` still
// exists: it installs the binary itself, with no wrapper.

'use strict';

const { spawn } = require('node:child_process');
const fs = require('node:fs');
const path = require('node:path');

const own = require('../package.json');

// ---------------------------------------------------------------------------
// Which platform package
// ---------------------------------------------------------------------------

/// Which C library this Linux uses.
///
/// The same probe `detect-libc` uses: a glibc runtime names its version in the
/// diagnostic report, and a musl one does not. No subprocess, so this cannot
/// fail in a container without a shell.
function libc() {
  if (process.platform !== 'linux') return null;
  try {
    const report = process.report.getReport();
    return report.header.glibcVersionRuntime ? 'gnu' : 'musl';
  } catch {
    // Unable to tell. Assume the common case; a wrong guess produces the
    // "not installed" message below rather than a crash.
    return 'gnu';
  }
}

/// Must agree with `keyFor` in `npm/build.mjs`.
function platformKey() {
  const suffix = libc();
  return `${process.platform}-${process.arch}${suffix ? `-${suffix}` : ''}`;
}

const KEY = platformKey();
const WANTED = `@kosong/cli-${KEY}`;
const BINARY = process.platform === 'win32' ? 'kosong.exe' : 'kosong';

// The published set, read from this package's own manifest rather than
// duplicated here. Adding a target to `build.mjs` updates this by itself.
const SUPPORTED = Object.keys(own.optionalDependencies || {});

// ---------------------------------------------------------------------------
// Failure, explained
// ---------------------------------------------------------------------------

function fail(lines) {
  process.stderr.write(`\n${lines.join('\n')}\n\n`);
  process.exit(127);
}

function unsupportedPlatform() {
  const names = SUPPORTED.map((n) => n.replace('@kosong/cli-', '  '));
  fail([
    `kosong has no build for ${KEY}.`,
    '',
    'It is published for:',
    ...names,
    '',
    KEY.endsWith('-musl')
      ? 'This looks like Alpine or another musl system. Only glibc Linux is built\ntoday. Building from source works: https://github.com/khalilhimura/kosong'
      : 'See https://github.com/khalilhimura/kosong/releases',
  ]);
}

function packageMissing() {
  fail([
    `kosong is installed, but the part that runs on ${KEY} is not.`,
    '',
    'This usually means the install skipped optional dependencies. Reinstall:',
    '',
    '  npm install -g kosong --include=optional',
    '',
    'If that does not help, please report it with the command you ran:',
    'https://github.com/khalilhimura/kosong/issues',
  ]);
}

// ---------------------------------------------------------------------------
// Locate and run
// ---------------------------------------------------------------------------

function locate() {
  if (!SUPPORTED.includes(WANTED)) unsupportedPlatform();

  let manifest;
  try {
    manifest = require.resolve(`${WANTED}/package.json`);
  } catch {
    packageMissing();
  }

  const binary = path.join(path.dirname(manifest), 'bin', BINARY);
  if (!fs.existsSync(binary)) packageMissing();

  // npm marks a `bin` entry executable on extraction, but a tarball unpacked
  // by other means may not be. Restoring the bit costs nothing and turns an
  // EACCES into a working command.
  try {
    fs.accessSync(binary, fs.constants.X_OK);
  } catch {
    try {
      fs.chmodSync(binary, 0o755);
    } catch {
      fail([
        `kosong found its binary but cannot run it:`,
        `  ${binary}`,
        '',
        'Check the file permissions, or install without npm:',
        '  curl -fsSL https://kosong.thefutureissolo.com/install.sh | sh',
      ]);
    }
  }

  return binary;
}

/// Signals a caller may reasonably send, and that must reach the real process.
const FORWARDED = ['SIGINT', 'SIGTERM', 'SIGHUP', 'SIGQUIT'];

function run() {
  const binary = locate();

  // Asynchronous `spawn`, not `spawnSync`, and the reason is signals.
  //
  // `spawnSync` blocks the event loop for the child's whole lifetime, so a JS
  // signal handler registered here can never run — but registering one still
  // suppresses Node's default "terminate on SIGTERM". The combination is the
  // worst of both: this process ignores the signal, the child never learns of
  // it, and `kosong preview` cannot be stopped by anything short of SIGKILL.
  // Observed during Phase A verification, not theorised: a directed
  // `kill -TERM` left the child running to completion.
  //
  // Anything that manages processes sends a directed signal — systemd, Docker,
  // `timeout`, a CI runner cancelling a job. Only a terminal's Ctrl-C reaches
  // the whole foreground group, and that is the one case the broken version
  // happened to survive.
  const child = spawn(binary, process.argv.slice(2), { stdio: 'inherit' });

  for (const signal of FORWARDED) {
    process.on(signal, () => {
      // A terminal delivers to the whole process group, so the child has
      // usually had this already; a second one is harmless. A directed signal
      // has reached only this process, and forwarding is the entire point.
      if (child.exitCode === null && child.signalCode === null) child.kill(signal);
    });
  }

  child.on('error', (error) => {
    fail([
      `kosong could not start: ${error.message}`,
      '',
      'Please report this with the command you ran:',
      'https://github.com/khalilhimura/kosong/issues',
    ]);
  });

  child.on('exit', (code, signal) => {
    if (signal) {
      // Re-raise, so the status a caller sees is the real one. Exiting 1 here
      // would tell a script that kosong failed when the user interrupted it —
      // a difference `kosong sync` in a CI job would act on.
      for (const forwarded of FORWARDED) process.removeAllListeners(forwarded);
      process.kill(process.pid, signal);
      // Reached only if the signal is ignored at this level.
      process.exit(1);
    }
    process.exit(code === null ? 1 : code);
  });
}

run();
