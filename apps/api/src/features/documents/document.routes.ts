/**
 * Document HTTP handlers.
 *
 * Both routes derive the user from the access token via `requireCaller`. There
 * is no path or body parameter naming a user or an object key, so there is
 * nothing for a caller to tamper with.
 */

import { z } from "zod";
import { requireCaller } from "../../shared/auth";
import { MAX_DOCUMENT_BYTES, type Env } from "../../shared/config";
import { ApiError, jsonResponse } from "../../shared/errors";
import type { Logger } from "../../shared/logging";
import { DocumentService } from "./document.service";

const putSchema = z.object({
  /** Raw OKF bytes, base64 encoded. */
  document: z.string().max(Math.ceil((MAX_DOCUMENT_BYTES * 4) / 3) + 8),
});

export async function handleGetDocument(
  request: Request,
  env: Env,
  logger: Logger,
): Promise<Response> {
  const caller = await requireCaller(request, env);
  const service = new DocumentService(env, logger);
  const payload = await service.read(caller.userId);

  return jsonResponse(payload, 200, { etag: payload.etag });
}

export async function handlePutDocument(
  request: Request,
  env: Env,
  logger: Logger,
): Promise<Response> {
  const caller = await requireCaller(request, env);

  let raw: unknown;
  try {
    raw = await request.json();
  } catch {
    throw ApiError.invalidInput("That request body was not valid JSON.");
  }

  const parsed = putSchema.safeParse(raw);
  if (!parsed.success) {
    // A `document` that fails the max-length check is over the size limit,
    // which deserves its own status rather than a generic 400.
    const tooLong = parsed.error.issues.some(
      (issue) => issue.path[0] === "document" && issue.code === "too_big",
    );
    throw tooLong
      ? ApiError.tooLarge(MAX_DOCUMENT_BYTES)
      : ApiError.invalidInput("The request must contain a `document` field.");
  }

  const service = new DocumentService(env, logger);
  const result = await service.write({
    userId: caller.userId,
    documentBase64: parsed.data.document,
    ifMatch: request.headers.get("if-match"),
  });

  return jsonResponse(result, 200, { etag: result.etag });
}
