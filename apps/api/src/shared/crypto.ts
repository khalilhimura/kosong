/**
 * Cryptographic primitives.
 *
 * Everything here is a security control. The properties that matter:
 *
 * - Verification codes are generated with a CSPRNG and **without modulo
 *   bias**, so every code is equally likely.
 * - Codes and refresh tokens are stored only as keyed hashes, so a database
 *   leak yields nothing usable.
 * - Comparisons of secret material run in constant time, so response timing
 *   does not reveal how much of a guess was correct.
 */

const encoder = new TextEncoder();

// ---------------------------------------------------------------------------
// Hashing
// ---------------------------------------------------------------------------

async function hmacKey(secret: string): Promise<CryptoKey> {
  return crypto.subtle.importKey(
    "raw",
    encoder.encode(secret),
    { name: "HMAC", hash: "SHA-256" },
    false,
    ["sign"],
  );
}

/** HMAC-SHA256 of `message` under `secret`, hex encoded. */
export async function hmacHex(secret: string, message: string): Promise<string> {
  const key = await hmacKey(secret);
  const signature = await crypto.subtle.sign("HMAC", key, encoder.encode(message));
  return toHex(new Uint8Array(signature));
}

/** SHA-256 of `message`, hex encoded. */
export async function sha256Hex(message: string): Promise<string> {
  const digest = await crypto.subtle.digest("SHA-256", encoder.encode(message));
  return toHex(new Uint8Array(digest));
}

function toHex(bytes: Uint8Array): string {
  return Array.from(bytes)
    .map((b) => b.toString(16).padStart(2, "0"))
    .join("");
}

/**
 * Compares two strings without leaking their contents through timing.
 *
 * Length is compared first and returns early, which is safe here because all
 * callers compare fixed-width hex digests.
 */
export function constantTimeEqual(a: string, b: string): boolean {
  if (a.length !== b.length) return false;

  let difference = 0;
  for (let i = 0; i < a.length; i++) {
    difference |= a.charCodeAt(i) ^ b.charCodeAt(i);
  }
  return difference === 0;
}

// ---------------------------------------------------------------------------
// Random values
// ---------------------------------------------------------------------------

/**
 * Generates a six-digit verification code, uniformly distributed.
 *
 * Rejection sampling rather than `random % 1000000`. A modulo of a 32-bit
 * value by 10^6 is very slightly biased, because 2^32 is not a multiple of
 * 10^6; the bias is tiny, but this is the one number standing between an
 * attacker and an account, so it is generated correctly rather than
 * nearly-correctly.
 */
export function generateVerificationCode(): string {
  const limit = 1_000_000;
  // Largest multiple of `limit` that fits in a uint32. Values at or above it
  // are discarded so the remaining range divides evenly.
  const ceiling = Math.floor(0x1_0000_0000 / limit) * limit;

  const buffer = new Uint32Array(1);
  let value: number;
  do {
    crypto.getRandomValues(buffer);
    value = buffer[0]!;
  } while (value >= ceiling);

  return String(value % limit).padStart(6, "0");
}

/** Generates an opaque token: 32 random bytes, base64url encoded. */
export function generateOpaqueToken(): string {
  const bytes = new Uint8Array(32);
  crypto.getRandomValues(bytes);
  return base64UrlEncode(bytes);
}

export function generateId(): string {
  return crypto.randomUUID();
}

// ---------------------------------------------------------------------------
// Access tokens
// ---------------------------------------------------------------------------

interface AccessTokenPayload {
  /** User id. */
  sub: string;
  /** Issued at, seconds since epoch. */
  iat: number;
  /** Expires at, seconds since epoch. */
  exp: number;
}

/**
 * Mints a signed access token (JWT, HS256).
 *
 * Self-contained so document requests need no database round trip to
 * authenticate. The trade-off is that it cannot be revoked before it expires,
 * which is why the lifetime is short and revocation lives at the refresh layer.
 */
export async function mintAccessToken(
  secret: string,
  userId: string,
  ttlSeconds: number,
  now: number = Math.floor(Date.now() / 1000),
): Promise<string> {
  const header = { alg: "HS256", typ: "JWT" };
  const payload: AccessTokenPayload = {
    sub: userId,
    iat: now,
    exp: now + ttlSeconds,
  };

  const encodedHeader = base64UrlEncode(encoder.encode(JSON.stringify(header)));
  const encodedPayload = base64UrlEncode(encoder.encode(JSON.stringify(payload)));
  const signingInput = `${encodedHeader}.${encodedPayload}`;

  const key = await hmacKey(secret);
  const signature = await crypto.subtle.sign("HMAC", key, encoder.encode(signingInput));

  return `${signingInput}.${base64UrlEncode(new Uint8Array(signature))}`;
}

/**
 * Verifies an access token and returns the user id, or null.
 *
 * Returns null for every failure rather than throwing per reason, so a caller
 * cannot accidentally turn "bad signature" and "expired" into distinguishable
 * responses.
 */
export async function verifyAccessToken(
  secret: string,
  token: string,
  now: number = Math.floor(Date.now() / 1000),
): Promise<string | null> {
  const parts = token.split(".");
  if (parts.length !== 3) return null;

  const [encodedHeader, encodedPayload, providedSignature] = parts as [
    string,
    string,
    string,
  ];

  const key = await hmacKey(secret);
  const expected = await crypto.subtle.sign(
    "HMAC",
    key,
    encoder.encode(`${encodedHeader}.${encodedPayload}`),
  );

  if (!constantTimeEqual(base64UrlEncode(new Uint8Array(expected)), providedSignature)) {
    return null;
  }

  let payload: AccessTokenPayload;
  try {
    payload = JSON.parse(new TextDecoder().decode(base64UrlDecode(encodedPayload)));
  } catch {
    return null;
  }

  // The algorithm is fixed at HS256 by construction; the header is not
  // consulted when choosing how to verify, so `alg: none` has nothing to act
  // on.
  if (typeof payload.sub !== "string" || payload.sub.length === 0) return null;
  if (typeof payload.exp !== "number" || payload.exp <= now) return null;

  return payload.sub;
}

// ---------------------------------------------------------------------------
// base64url
// ---------------------------------------------------------------------------

function base64UrlEncode(bytes: Uint8Array): string {
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

function base64UrlDecode(text: string): Uint8Array {
  const padded = text.replace(/-/g, "+").replace(/_/g, "/");
  const binary = atob(padded.padEnd(Math.ceil(padded.length / 4) * 4, "="));
  return Uint8Array.from(binary, (c) => c.charCodeAt(0));
}

export { base64UrlEncode, base64UrlDecode };
