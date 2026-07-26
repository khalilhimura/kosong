/**
 * kosong remote API.
 *
 * A hand-written router rather than a framework. There are nine routes, and
 * the product principle is that a user can inspect what they depend on — a
 * file you can read end to end beats a dependency you cannot.
 *
 * @see spec/okf-profile-v1.md and the technical specification, §8 to §11.
 */

import {
  handleCodeRequest,
  handleCodeVerify,
  handleDeleteAccount,
  handleLogout,
  handleRefresh,
} from "./features/auth/auth.routes";
import type { EmailSender } from "./features/auth/email";
import {
  handleGetDocument,
  handlePutDocument,
} from "./features/documents/document.routes";
import { requireSecrets, type Env } from "./shared/config";
import { ApiError, errorResponse, jsonResponse } from "./shared/errors";
import { Logger } from "./shared/logging";

/**
 * Test seam for injecting a recording email sender.
 *
 * Not reachable in production: nothing sets it, and the Worker export does not
 * accept it. It exists so tests can assert a code was issued without a real
 * one leaving the process.
 */
export interface Overrides {
  emailSender?: EmailSender;
}

export async function route(
  request: Request,
  env: Env,
  overrides: Overrides = {},
): Promise<Response> {
  const logger = Logger.forRequest();
  const started = Date.now();
  const url = new URL(request.url);
  const path = url.pathname.replace(/\/+$/, "") || "/";
  const method = request.method.toUpperCase();

  try {
    const response = await dispatch(request, env, logger, path, method, overrides);

    logger.info({
      event: "request",
      outcome: response.status < 400 ? "success" : "failure",
      // The route pattern, never the raw URL.
      route: `${method} ${path}`,
      method,
      status: response.status,
      duration_ms: Date.now() - started,
    });
    return withSecurityHeaders(response, logger.requestId);
  } catch (error) {
    const apiError =
      error instanceof ApiError ? error : ApiError.internal();

    if (!(error instanceof ApiError)) {
      // Only the error's type is logged. A message can carry a bound SQL
      // parameter, which for these routes means a token or an address.
      logger.error({
        event: "unhandled_error",
        outcome: "failure",
        route: `${method} ${path}`,
        detail: error instanceof Error ? error.name : "unknown",
      });
    } else {
      logger.warn({
        event: "request_failed",
        outcome: "failure",
        route: `${method} ${path}`,
        status: apiError.status,
        error_code: apiError.code,
        duration_ms: Date.now() - started,
      });
    }

    return withSecurityHeaders(errorResponse(apiError, logger.requestId), logger.requestId);
  }
}

async function dispatch(
  request: Request,
  env: Env,
  logger: Logger,
  path: string,
  method: string,
  overrides: Overrides,
): Promise<Response> {
  // Liveness needs no configuration, so it is answered before the secret
  // check. A misconfigured Worker should still report that it is running.
  if (path === "/health" && method === "GET") {
    return jsonResponse({ status: "ok" });
  }

  if (path === "/ready" && method === "GET") {
    return readiness(env);
  }

  // Every route below touches credentials.
  try {
    requireSecrets(env);
  } catch {
    logger.error({ event: "missing_secrets", outcome: "failure" });
    throw new ApiError(503, "NOT_READY", "This service is not ready yet.");
  }

  switch (`${method} ${path}`) {
    case "POST /v1/auth/code/request":
      return handleCodeRequest(request, env, logger, overrides.emailSender);
    case "POST /v1/auth/code/verify":
      return handleCodeVerify(request, env, logger, overrides.emailSender);
    case "POST /v1/auth/refresh":
      return handleRefresh(request, env, logger);
    case "POST /v1/auth/logout":
      return handleLogout(request, env, logger);
    case "DELETE /v1/account":
      return handleDeleteAccount(request, env, logger);
    case "GET /v1/document":
      return handleGetDocument(request, env, logger);
    case "PUT /v1/document":
      return handlePutDocument(request, env, logger);
    default:
      throw ApiError.notFound("There is no such endpoint.");
  }
}

/** Readiness: can the database actually be reached? */
async function readiness(env: Env): Promise<Response> {
  try {
    await env.DB.prepare("SELECT 1").first();
    return jsonResponse({ status: "ready" });
  } catch {
    return jsonResponse({ status: "not_ready" }, 503);
  }
}

/**
 * Applies response headers and surfaces the request id.
 *
 * The API returns only JSON, so the content policy denies everything: there is
 * no legitimate case for a response of ours to load or execute anything.
 */
function withSecurityHeaders(response: Response, requestId: string): Response {
  const headers = new Headers(response.headers);
  headers.set("x-request-id", requestId);
  headers.set("x-content-type-options", "nosniff");
  headers.set("content-security-policy", "default-src 'none'; frame-ancestors 'none'");
  headers.set("referrer-policy", "no-referrer");
  // A CLI is not a browser: no origin should be able to call this from a page.
  headers.set("vary", "origin");

  return new Response(response.body, {
    status: response.status,
    statusText: response.statusText,
    headers,
  });
}

export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    return route(request, env);
  },
} satisfies ExportedHandler<Env>;
