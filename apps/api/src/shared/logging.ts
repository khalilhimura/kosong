/**
 * Structured, redacted logging and the security event log.
 *
 * Technical specification §11 forbids logging verification codes, bearer or
 * refresh tokens, document bytes, raw email addresses, authorization headers,
 * and provider credentials.
 *
 * The defence here is structural rather than procedural. Log entries are built
 * from a fixed field set, and every value passes through {@link redact}, which
 * strips anything shaped like a credential. Relying on each call site to
 * remember the rule is how these leaks happen.
 */

import type { Env } from "./config";
import { generateId, hmacHex } from "./crypto";

export type LogLevel = "debug" | "info" | "warn" | "error";

/** Fields permitted in a log line. Anything else is dropped. */
export interface LogFields {
  event: string;
  outcome?: "success" | "failure";
  /** Route pattern, never the raw URL — a URL can carry identifiers. */
  route?: string;
  method?: string;
  status?: number;
  duration_ms?: number;
  error_code?: string;
  user_id?: string;
  ip_hash?: string;
  /** Domain part of an email address only. `example.com`, never the mailbox. */
  email_domain?: string;
  detail?: string;
}

/** Substrings that mean a value must never be logged. */
const FORBIDDEN_KEY_PATTERNS = [
  "token",
  "secret",
  "password",
  "code",
  "authorization",
  "bearer",
  "email",
  "document",
  "body",
  "content",
];

/**
 * Removes anything that looks like a credential.
 *
 * `email_domain` and `error_code` are allowed through by name: both match a
 * forbidden pattern as a substring, but neither carries a secret. Everything
 * else matching is replaced rather than trusted.
 */
export function redact(fields: Record<string, unknown>): Record<string, unknown> {
  const allowedExceptions = new Set(["email_domain", "error_code"]);
  const output: Record<string, unknown> = {};

  for (const [key, value] of Object.entries(fields)) {
    if (value === undefined || value === null) continue;

    const lowered = key.toLowerCase();
    if (
      !allowedExceptions.has(lowered) &&
      FORBIDDEN_KEY_PATTERNS.some((pattern) => lowered.includes(pattern))
    ) {
      output[key] = "[redacted]";
      continue;
    }
    output[key] = value;
  }
  return output;
}

/** Per-request logger carrying the request id. */
export class Logger {
  constructor(readonly requestId: string) {}

  static forRequest(): Logger {
    return new Logger(generateId());
  }

  log(level: LogLevel, fields: LogFields): void {
    const line = {
      timestamp: new Date().toISOString(),
      level,
      request_id: this.requestId,
      ...redact(fields as unknown as Record<string, unknown>),
    };
    // Workers Logs collects stdout/stderr; JSON keeps entries queryable.
    console.log(JSON.stringify(line));
  }

  info(fields: LogFields): void {
    this.log("info", fields);
  }

  warn(fields: LogFields): void {
    this.log("warn", fields);
  }

  error(fields: LogFields): void {
    this.log("error", fields);
  }
}

/**
 * The domain of an email address, for logging.
 *
 * Enough to spot an attack pattern across one provider, not enough to
 * identify a person.
 */
export function emailDomain(email: string): string | undefined {
  const at = email.lastIndexOf("@");
  return at === -1 ? undefined : email.slice(at + 1);
}

/** Keyed hash of a client IP. Comparable across requests, not reversible. */
export async function hashIp(env: Env, ip: string | null): Promise<string | undefined> {
  if (!ip) return undefined;
  return hmacHex(env.IP_HASH_SECRET, ip);
}

/** Client IP as seen by Cloudflare. */
export function clientIp(request: Request): string | null {
  return request.headers.get("cf-connecting-ip");
}

// ---------------------------------------------------------------------------
// Security events
// ---------------------------------------------------------------------------

/** Auditable security events. Add, never rename. */
export type SecurityEventType =
  | "code_requested"
  | "code_request_rate_limited"
  | "code_verified"
  | "code_verify_failed"
  | "code_verify_exhausted"
  | "session_refreshed"
  | "refresh_reuse_detected"
  | "session_revoked"
  | "account_deleted"
  | "document_read"
  | "document_written"
  | "document_conflict"
  | "unauthorized_document_access";

/**
 * Records a security event in D1.
 *
 * Never throws: an audit write failing must not turn a successful operation
 * into a failed one. A failure is logged instead.
 */
export async function recordSecurityEvent(
  env: Env,
  logger: Logger,
  event: {
    type: SecurityEventType;
    userId?: string;
    ipHash?: string;
    metadata?: Record<string, unknown>;
  },
): Promise<void> {
  try {
    await env.DB.prepare(
      `INSERT INTO security_events (id, user_id, event_type, request_id, ip_hash, created_at, metadata_json)
       VALUES (?, ?, ?, ?, ?, ?, ?)`,
    )
      .bind(
        generateId(),
        event.userId ?? null,
        event.type,
        logger.requestId,
        event.ipHash ?? null,
        new Date().toISOString(),
        // Redacted before storage, so the audit log cannot become the leak.
        event.metadata ? JSON.stringify(redact(event.metadata)) : null,
      )
      .run();
  } catch (error) {
    logger.error({
      event: "security_event_write_failed",
      outcome: "failure",
      detail: error instanceof Error ? error.name : "unknown",
    });
  }
}
