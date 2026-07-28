// Reading the workspace version out of Cargo.toml.
//
// Shared by `build.mjs`, which stamps it into five package manifests, and
// `verify.mjs`, which checks the packaged binary reports it. One copy, because
// two would drift and this value gates an immutable npm publish.
//
// Sharing does not weaken the check it feeds. `verify.mjs` compares this
// against the version the *binary* prints, which Cargo compiled in — so a bug
// here makes that assertion fail rather than agree with itself.

import fs from 'node:fs';
import path from 'node:path';

/// The version declared by `[workspace.package]`.
///
/// Bounded to that section. Reading everything after the heading and taking
/// the first `version` line would survive today only because `version` happens
/// to be its first key: move it, or add a table above it that has a version of
/// its own, and the wrong value would be read silently and then published
/// under a number that cannot be taken back.
export function workspaceVersion(root) {
  const manifest = fs.readFileSync(path.join(root, 'Cargo.toml'), 'utf8');

  const heading = '[workspace.package]';
  const start = manifest.indexOf(heading);
  if (start === -1) throw new Error('Cargo.toml has no [workspace.package] section');

  const rest = manifest.slice(start + heading.length);
  const nextHeading = rest.search(/\n[ \t]*\[/);
  const section = nextHeading === -1 ? rest : rest.slice(0, nextHeading);

  const match = section.match(/^\s*version\s*=\s*"([^"]+)"/m);
  if (!match) throw new Error('[workspace.package] declares no version');
  return match[1];
}
