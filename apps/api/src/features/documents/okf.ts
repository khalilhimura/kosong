/**
 * Open Knowledge Format validation at the API boundary.
 *
 * Technical specification §9.4 requires validating OKF structure "on the CLI
 * before sending and on the API before storing". The CLI check is a courtesy
 * to the user; this one is the rule. A client is not trusted to have run it.
 *
 * The rules mirror `kosong_core::okf` and `spec/okf-profile-v1.md`:
 * parseable YAML front matter carrying a non-empty `type`. Everything else —
 * unknown keys, unknown `type` values, a missing `kosong` block — is
 * explicitly permitted, because OKF forbids consumers from rejecting for any
 * of those.
 *
 * @see https://github.com/GoogleCloudPlatform/knowledge-catalog/blob/main/okf/SPEC.md
 */

import { parse as parseYaml } from "yaml";

/**
 * Cap on YAML alias expansion.
 *
 * Without one, a small document can expand exponentially during parsing — the
 * "billion laughs" attack. The size limit alone does not prevent it, because
 * the blow-up happens after the bytes are counted.
 */
const MAX_YAML_ALIAS_COUNT = 100;

export interface OkfValidationResult {
  valid: boolean;
  /** Present when invalid. Safe to return: describes structure, not content. */
  reason?: string;
  /** The `type` value, when the document is valid. */
  type?: string;
}

/**
 * Validates OKF text.
 *
 * Never throws. A parser failure on hostile input is a validation result, not
 * an exception that becomes a 500.
 */
export function validateOkf(text: string): OkfValidationResult {
  const withoutBom = text.startsWith("﻿") ? text.slice(1) : text;

  const split = splitFrontMatter(withoutBom);
  if (!split) {
    return {
      valid: false,
      reason: "the document has no front matter, or it is never closed",
    };
  }

  let parsed: unknown;
  try {
    parsed = parseYaml(split.yaml, { maxAliasCount: MAX_YAML_ALIAS_COUNT });
  } catch {
    // The parser message can quote document content, so it is not passed on.
    return { valid: false, reason: "the front matter is not valid YAML" };
  }

  if (parsed === null || typeof parsed !== "object" || Array.isArray(parsed)) {
    return {
      valid: false,
      reason: "the front matter is not a set of key/value pairs",
    };
  }

  const type = (parsed as Record<string, unknown>)["type"];
  if (type === undefined || type === null) {
    return { valid: false, reason: "the required `type` field is missing" };
  }
  if (typeof type === "string" && type.trim() === "") {
    return { valid: false, reason: "the `type` field is empty" };
  }

  return { valid: true, type: typeof type === "string" ? type : String(type) };
}

/** Splits `---` delimited front matter from the body. */
function splitFrontMatter(text: string): { yaml: string; body: string } | null {
  let rest: string;
  if (text.startsWith("---\r\n")) {
    rest = text.slice(5);
  } else if (text.startsWith("---\n")) {
    rest = text.slice(4);
  } else {
    return null;
  }

  let offset = 0;
  while (offset < rest.length) {
    const newline = rest.indexOf("\n", offset);
    const lineEnd = newline === -1 ? rest.length : newline + 1;
    const line = rest.slice(offset, lineEnd).replace(/[\r\n]+$/, "");

    if (line === "---" || line === "...") {
      return { yaml: rest.slice(0, offset), body: rest.slice(lineEnd) };
    }
    offset = lineEnd;
  }
  return null;
}
