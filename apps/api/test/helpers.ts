import { env } from "cloudflare:test";
import type { EmailSender } from "../src/features/auth/email";
import { RecordingEmailSender } from "../src/features/auth/email";
import { route } from "../src/index";

export const BASE = "https://api.kosong.test";

/** A minimal conformant OKF document. */
export function okfDocument(title = "My First Site"): string {
  return `---\ntype: Page\ntitle: ${title}\n---\n# ${title}\n\nStart here.\n`;
}

export function toBase64(text: string): string {
  const bytes = new TextEncoder().encode(text);
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary);
}

export function fromBase64(encoded: string): string {
  return new TextDecoder().decode(
    Uint8Array.from(atob(encoded), (c) => c.charCodeAt(0)),
  );
}

/**
 * A fresh address per call.
 *
 * Tests must not share addresses: the per-email cooldown is a real control,
 * and reusing one would make unrelated tests interfere.
 */
let counter = 0;
export function uniqueEmail(): string {
  counter += 1;
  return `person${counter}.${Date.now()}@example.com`;
}

export interface RequestOptions {
  method?: string;
  body?: unknown;
  headers?: Record<string, string>;
  accessToken?: string;
  // Not just the recording one: a test needs to inject senders that fail.
  emailSender?: EmailSender;
  ip?: string;
}

/** Sends a request through the Worker's router. */
export async function call(path: string, options: RequestOptions = {}): Promise<Response> {
  const headers: Record<string, string> = {
    "content-type": "application/json",
    ...options.headers,
  };
  if (options.accessToken) {
    headers["authorization"] = `Bearer ${options.accessToken}`;
  }
  if (options.ip) {
    headers["cf-connecting-ip"] = options.ip;
  }

  const request = new Request(`${BASE}${path}`, {
    method: options.method ?? "GET",
    headers,
    body: options.body === undefined ? undefined : JSON.stringify(options.body),
  });

  return route(request, env, { emailSender: options.emailSender });
}

export interface Session {
  access_token: string;
  refresh_token: string;
  user: { id: string; email_hint: string };
  email: string;
}

/** Requests a code, reads it from the recorded email, and verifies it. */
export async function signIn(email = uniqueEmail()): Promise<Session> {
  const mail = new RecordingEmailSender();

  const requested = await call("/v1/auth/code/request", {
    method: "POST",
    body: { email },
    emailSender: mail,
  });
  if (requested.status !== 202) {
    throw new Error(`code request failed: ${requested.status}`);
  }

  const sent = mail.sent.at(-1);
  if (!sent) throw new Error("no verification email was sent");

  const verified = await call("/v1/auth/code/verify", {
    method: "POST",
    body: { email, code: sent.code },
    emailSender: mail,
  });
  if (verified.status !== 200) {
    throw new Error(`verify failed: ${verified.status} ${await verified.text()}`);
  }

  return { ...(await verified.json<Omit<Session, "email">>()), email };
}

/** Requests a code and returns it, without verifying. */
export async function requestCode(email: string, ip?: string): Promise<string> {
  const mail = new RecordingEmailSender();
  const response = await call("/v1/auth/code/request", {
    method: "POST",
    body: { email },
    emailSender: mail,
    ip,
  });
  if (response.status !== 202) {
    throw new Error(`code request failed: ${response.status}`);
  }
  const sent = mail.sent.at(-1);
  if (!sent) throw new Error("no verification email was sent");
  return sent.code;
}

/**
 * Ages existing codes past the cooldown, without leaving the rate-limit window.
 *
 * Lets a test exercise a control other than the cooldown without waiting a
 * real minute. Five minutes is chosen deliberately: past the 60-second
 * cooldown, but well inside the one-hour window the per-email cap counts over.
 * Backdating further would silently disable the very limit under test.
 */
export async function clearCooldown(email: string): Promise<void> {
  await env.DB.prepare(
    `UPDATE verification_codes SET created_at = ? WHERE email_normalized = ?`,
  )
    .bind(new Date(Date.now() - 5 * 60 * 1000).toISOString(), email.toLowerCase())
    .run();
}

/** An error body with the per-request id removed, so two can be compared. */
export async function bodyWithoutRequestId(
  response: Response,
): Promise<Record<string, unknown>> {
  const { request_id: _ignored, ...rest } = await response.json<Record<string, unknown>>();
  return rest;
}

/** Forces the active code for an address to have expired. */
export async function expireCode(email: string): Promise<void> {
  await env.DB.prepare(
    `UPDATE verification_codes SET expires_at = ? WHERE email_normalized = ?`,
  )
    .bind(new Date(Date.now() - 1000).toISOString(), email.toLowerCase())
    .run();
}
