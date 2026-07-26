/**
 * Deriving the caller's identity from their credentials.
 *
 * Architectural rule 4 in technical specification §2: "Remote API owns
 * identity and authorization." The user id comes from a verified access token
 * and from nowhere else — not a header, not a query parameter, not the request
 * body. This module is the only place that decides who is calling, so there is
 * exactly one thing to audit.
 */

import type { Env } from "./config";
import { verifyAccessToken } from "./crypto";
import { ApiError } from "./errors";

/** An authenticated caller. */
export interface Caller {
  userId: string;
}

/**
 * Extracts and verifies the bearer token, returning the caller.
 *
 * Throws for a missing, malformed, or invalid token. Every case produces the
 * same response, so a caller cannot learn which part they got wrong.
 */
export async function requireCaller(request: Request, env: Env): Promise<Caller> {
  const header = request.headers.get("authorization");
  if (!header) {
    throw ApiError.unauthenticated();
  }

  // Scheme comparison is case-insensitive per RFC 7235.
  const match = /^Bearer\s+(.+)$/i.exec(header.trim());
  if (!match) {
    throw ApiError.unauthenticated();
  }

  const userId = await verifyAccessToken(env.TOKEN_HMAC_SECRET, match[1]!.trim());
  if (!userId) {
    throw ApiError.sessionExpired("Your session has expired. Please sign in again.");
  }

  return { userId };
}
