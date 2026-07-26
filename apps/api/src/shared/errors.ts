/**
 * The error contract, per technical specification §11.
 *
 * Every failure response is `{ code, message, request_id }`. The code is
 * stable and machine-readable; the message is for a person; the request id is
 * what a user can quote when asking for help.
 *
 * Messages must never leak whether an account exists, what another user's
 * state is, or any internal detail.
 */

/** Stable machine-readable failure codes. Add, never rename. */
export type ErrorCode =
  | "INVALID_INPUT"
  | "INVALID_OKF"
  | "UNAUTHENTICATED"
  | "SESSION_EXPIRED"
  | "FORBIDDEN"
  | "NOT_FOUND"
  | "DOCUMENT_CONFLICT"
  | "DOCUMENT_TOO_LARGE"
  | "RATE_LIMITED"
  | "INTERNAL_ERROR"
  | "NOT_READY";

export class ApiError extends Error {
  constructor(
    readonly status: number,
    readonly code: ErrorCode,
    message: string,
    /** Extra response fields, e.g. remote metadata on a conflict. */
    readonly details?: Record<string, unknown>,
    /** Seconds to wait, for 429 responses. */
    readonly retryAfterSeconds?: number,
  ) {
    super(message);
    this.name = "ApiError";
  }

  static invalidInput(message = "That request was not valid."): ApiError {
    return new ApiError(400, "INVALID_INPUT", message);
  }

  static invalidOkf(message: string): ApiError {
    return new ApiError(400, "INVALID_OKF", message);
  }

  static unauthenticated(message = "You are not signed in."): ApiError {
    return new ApiError(401, "UNAUTHENTICATED", message);
  }

  static sessionExpired(message = "Your session has expired."): ApiError {
    return new ApiError(401, "SESSION_EXPIRED", message);
  }

  static notFound(message = "There is nothing here."): ApiError {
    return new ApiError(404, "NOT_FOUND", message);
  }

  static rateLimited(retryAfterSeconds: number): ApiError {
    return new ApiError(
      429,
      "RATE_LIMITED",
      "Too many requests. Please wait a moment and try again.",
      undefined,
      retryAfterSeconds,
    );
  }

  static conflict(details: Record<string, unknown>): ApiError {
    return new ApiError(
      409,
      "DOCUMENT_CONFLICT",
      "The remote document changed after your last sync.",
      details,
    );
  }

  static tooLarge(limit: number): ApiError {
    return new ApiError(
      413,
      "DOCUMENT_TOO_LARGE",
      `That document is larger than the ${limit} byte limit.`,
    );
  }

  static internal(): ApiError {
    return new ApiError(
      500,
      "INTERNAL_ERROR",
      "Something went wrong on our side.",
    );
  }
}

/** Builds a JSON response carrying the standard error body. */
export function errorResponse(error: ApiError, requestId: string): Response {
  const headers: Record<string, string> = {
    "content-type": "application/json; charset=utf-8",
    // A failure is never cacheable; a stale 401 would be very confusing.
    "cache-control": "no-store",
  };
  if (error.retryAfterSeconds !== undefined) {
    headers["retry-after"] = String(error.retryAfterSeconds);
  }

  return new Response(
    JSON.stringify({
      code: error.code,
      message: error.message,
      request_id: requestId,
      ...(error.details ?? {}),
    }),
    { status: error.status, headers },
  );
}

/** Builds a JSON success response. */
export function jsonResponse(
  body: unknown,
  status = 200,
  extraHeaders: Record<string, string> = {},
): Response {
  return new Response(status === 204 ? null : JSON.stringify(body), {
    status,
    headers: {
      "content-type": "application/json; charset=utf-8",
      "cache-control": "no-store",
      ...extraHeaders,
    },
  });
}
