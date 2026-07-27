/**
 * Environment bindings and the security constants that govern them.
 *
 * Every number here is a security control from technical specification §8
 * and §14. They are named constants rather than inline literals so a change
 * is a visible, reviewable edit.
 */

/**
 * The Worker's environment.
 *
 * Bindings and plain vars are generated from `wrangler.jsonc` by
 * `wrangler types`; secrets are declared in `src/env.d.ts`. Adding a binding
 * to the config makes it appear here with no edit to this file.
 */
export type Env = Cloudflare.Env;

/** How long a verification code stays usable. §8.2: ten minutes. */
export const CODE_TTL_SECONDS = 10 * 60;

/**
 * Verification attempts allowed against one code before it is dead.
 *
 * With a six-digit code there are 10^6 possibilities, so five attempts leaves
 * a 1-in-200,000 chance per issued code. Combined with the ten-minute expiry
 * and per-email rate limiting, guessing is not a viable path.
 */
export const MAX_VERIFY_ATTEMPTS = 5;

/** Seconds a caller must wait between requesting codes for one address. */
export const CODE_REQUEST_COOLDOWN_SECONDS = 60;

/** Codes requestable per email address within {@link RATE_LIMIT_WINDOW_SECONDS}. */
export const MAX_CODES_PER_EMAIL = 5;

/** Codes requestable per source IP within {@link RATE_LIMIT_WINDOW_SECONDS}. */
export const MAX_CODES_PER_IP = 20;

/** Window over which the two limits above are counted. */
export const RATE_LIMIT_WINDOW_SECONDS = 60 * 60;

/**
 * Access token lifetime. Short, because it is a bearer credential that the
 * server cannot revoke individually; revocation happens at the refresh layer.
 */
export const ACCESS_TOKEN_TTL_SECONDS = 15 * 60;

/** Refresh token lifetime. Rotated on every use. */
export const REFRESH_TOKEN_TTL_SECONDS = 30 * 24 * 60 * 60;

/**
 * How long an auditable security event is kept.
 *
 * Ninety days is long enough to recognise a slow pattern — credential
 * stuffing spread thin to stay under the rate limits — and short enough that
 * the log is not an indefinite record of who signed in from where.
 */
export const SECURITY_EVENT_RETENTION_SECONDS = 90 * 24 * 60 * 60;

/**
 * How long a verification code row is kept after it is created.
 *
 * **Must stay well above {@link RATE_LIMIT_WINDOW_SECONDS}.** The per-email
 * and per-IP limits are enforced by counting rows created inside that window,
 * so deleting them sooner would not fail loudly — it would quietly stop the
 * limiter from counting, leaving the endpoint open while every test about
 * expiry still passed.
 *
 * A day is twenty-four times the window, and clears the plaintext address of
 * anyone who asked for a code and never came back.
 */
export const VERIFICATION_CODE_RETENTION_SECONDS = 24 * 60 * 60;

/**
 * Largest document accepted, in bytes.
 *
 * Must match `kosong_core::workspace::MAX_DOCUMENT_BYTES`. §9.4 requires the
 * limit in both layers: the CLI check is a courtesy, this one is the rule.
 */
export const MAX_DOCUMENT_BYTES = 1024 * 1024;

/** R2 key for a user's document. Derived server-side, never client-supplied. */
export function documentKey(userId: string): string {
  return `users/${userId}/document-v1`;
}

export function turnstileEnabled(env: Env): boolean {
  // `wrangler types` narrows a var to the literal value in wrangler.jsonc, so
  // the generated type here is `"false"`. That is accurate for the checked-in
  // config but wrong in general: this is a per-environment override, and a
  // deployment can set it to "true" without the type changing. Read as a
  // plain string.
  return (env.TURNSTILE_ENABLED as string) === "true";
}

/**
 * Fails fast when a required secret is missing.
 *
 * Running without `CODE_HMAC_SECRET` would silently store codes hashed under
 * an empty key, so this is checked at request time rather than assumed.
 */
export function requireSecrets(env: Env): void {
  const missing = (
    ["CODE_HMAC_SECRET", "TOKEN_HMAC_SECRET", "IP_HASH_SECRET"] as const
  ).filter((name) => !env[name]);

  if (missing.length > 0) {
    throw new Error(`missing required secrets: ${missing.join(", ")}`);
  }
}
