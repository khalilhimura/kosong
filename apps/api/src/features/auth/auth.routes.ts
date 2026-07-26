/**
 * Authentication HTTP handlers.
 *
 * Every request body is validated with Zod before it reaches a service, so
 * services never see unvalidated shapes.
 */

import { z } from "zod";
import { requireCaller } from "../../shared/auth";
import type { Env } from "../../shared/config";
import { ApiError, jsonResponse } from "../../shared/errors";
import { clientIp, hashIp, type Logger } from "../../shared/logging";
import { AuthService } from "./auth.service";
import type { EmailSender } from "./email";

/** RFC 5321 caps an address at 254 characters. */
const MAX_EMAIL_LENGTH = 254;

const codeRequestSchema = z.object({
  email: z.email().max(MAX_EMAIL_LENGTH),
  turnstile_token: z.string().max(4096).optional(),
});

const codeVerifySchema = z.object({
  email: z.email().max(MAX_EMAIL_LENGTH),
  // Exactly six digits. Anything else is rejected before a lookup happens.
  code: z.string().regex(/^\d{6}$/, "a code is six digits"),
  device_name: z.string().max(100).optional(),
});

const refreshSchema = z.object({
  refresh_token: z.string().min(1).max(512),
});

/** Parses a JSON body against a schema, or fails with a safe message. */
async function parseBody<T>(request: Request, schema: z.ZodType<T>): Promise<T> {
  let raw: unknown;
  try {
    raw = await request.json();
  } catch {
    throw ApiError.invalidInput("That request body was not valid JSON.");
  }

  const result = schema.safeParse(raw);
  if (!result.success) {
    // Only field names are surfaced. Zod messages can echo the submitted
    // value, which for these routes means an email address or a code.
    const fields = result.error.issues
      .map((issue) => issue.path.join("."))
      .filter(Boolean);
    throw ApiError.invalidInput(
      fields.length > 0
        ? `Check these fields: ${[...new Set(fields)].join(", ")}.`
        : "That request was not valid.",
    );
  }
  return result.data;
}

export async function handleCodeRequest(
  request: Request,
  env: Env,
  logger: Logger,
  emailSender?: EmailSender,
): Promise<Response> {
  const body = await parseBody(request, codeRequestSchema);
  const service = new AuthService(env, logger, emailSender);

  await service.requestCode({
    email: body.email,
    ipHash: await hashIp(env, clientIp(request)),
    turnstileToken: body.turnstile_token,
  });

  // 202 with an identical body whether or not the address has an account.
  // §8.2: the response must not enable enumeration.
  return jsonResponse(
    {
      status: "accepted",
      message: "If that address can receive mail, a code is on its way.",
    },
    202,
  );
}

export async function handleCodeVerify(
  request: Request,
  env: Env,
  logger: Logger,
  emailSender?: EmailSender,
): Promise<Response> {
  const body = await parseBody(request, codeVerifySchema);
  const service = new AuthService(env, logger, emailSender);

  const session = await service.verifyCode({
    email: body.email,
    code: body.code,
    deviceName: body.device_name,
    ipHash: await hashIp(env, clientIp(request)),
  });

  return jsonResponse(session, 200);
}

export async function handleRefresh(
  request: Request,
  env: Env,
  logger: Logger,
): Promise<Response> {
  const body = await parseBody(request, refreshSchema);
  const service = new AuthService(env, logger);
  return jsonResponse(await service.refresh(body.refresh_token), 200);
}

export async function handleLogout(
  request: Request,
  env: Env,
  logger: Logger,
): Promise<Response> {
  const body = await parseBody(request, refreshSchema);
  const service = new AuthService(env, logger);
  await service.logout(body.refresh_token);

  // Always 204: signing out must not report whether the credential was real.
  return jsonResponse(null, 204);
}

export async function handleDeleteAccount(
  request: Request,
  env: Env,
  logger: Logger,
): Promise<Response> {
  const caller = await requireCaller(request, env);
  const service = new AuthService(env, logger);

  await service.deleteAccount(caller.userId);

  // 200, not 202: deletion completes before the response, so there is no
  // pending state for the user to wonder about. §8.2 permits 202 only when
  // deletion is asynchronous.
  return jsonResponse({ status: "deleted" }, 200);
}
