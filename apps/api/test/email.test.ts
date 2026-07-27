/**
 * What the Resend adapter makes of each answer the provider can give.
 *
 * These exist because the rest of the suite injects fake senders, which meant
 * the one piece of code that decides *whose fault a failure is* was never
 * exercised. That is where the bug lived: every 4xx read as "bad recipient",
 * so a revoked API key — a 401 — would have been absorbed as though a user had
 * mistyped their address, answering every sign-in with "a code is on its way"
 * while sending nothing at all.
 *
 * The distinction these lock down:
 *
 * | Provider says | Whose fault | Caller does |
 * |---|---|---|
 * | 400, 422 | the recipient's | answer 202, same as always |
 * | anything else | the service's | answer 503, say try again |
 */

import { afterEach, describe, expect, it, vi } from "vitest";
import {
  ConsoleEmailSender,
  EmailProviderUnavailable,
  EmailRecipientRejected,
  ResendEmailSender,
  emailSenderFor,
} from "../src/features/auth/email";
import type { Env } from "../src/shared/config";

const MESSAGE = { to: "person@example.com", code: "123456" };

/** Replies with one status and records what it was asked to send. */
function stubProvider(status: number, body = "{}") {
  const calls: Array<{ url: string; init: RequestInit }> = [];
  vi.stubGlobal("fetch", async (url: string, init: RequestInit) => {
    calls.push({ url, init });
    return new Response(body, { status });
  });
  return calls;
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("a provider answer about the recipient", () => {
  it.each([400, 422])("treats %i as the address's problem", async (status) => {
    stubProvider(status);
    const sender = new ResendEmailSender("key", "kosong <login@example.com>");

    await expect(sender.sendVerificationCode(MESSAGE)).rejects.toBeInstanceOf(
      EmailRecipientRejected,
    );
  });
});

describe("a provider answer about the service", () => {
  // 401 is the regression: a revoked or mistyped API key. 402 is an unpaid
  // account, 429 a rate limit, 5xx a fault at their end. None of them says
  // anything about the address, and each needs an operator, not a retype.
  it.each([401, 402, 403, 404, 429, 500, 502, 503])(
    "treats %i as ours to own",
    async (status) => {
      stubProvider(status);
      const sender = new ResendEmailSender("key", "kosong <login@example.com>");

      await expect(sender.sendVerificationCode(MESSAGE)).rejects.toBeInstanceOf(
        EmailProviderUnavailable,
      );
    },
  );

  it("treats a provider it cannot reach at all as unavailable", async () => {
    vi.stubGlobal("fetch", async () => {
      throw new TypeError("network error");
    });
    const sender = new ResendEmailSender("key", "kosong <login@example.com>");

    await expect(sender.sendVerificationCode(MESSAGE)).rejects.toBeInstanceOf(
      EmailProviderUnavailable,
    );
  });
});

describe("what a failure is allowed to carry", () => {
  it("never puts the address or the code in the error", async () => {
    // The provider's body can echo both, and these errors reach the log.
    stubProvider(422, JSON.stringify({ message: `bad recipient ${MESSAGE.to}` }));
    const sender = new ResendEmailSender("key", "kosong <login@example.com>");

    const error = await sender.sendVerificationCode(MESSAGE).catch((e) => e);

    expect(String(error)).not.toContain(MESSAGE.to);
    expect(String(error)).not.toContain(MESSAGE.code);
  });
});

describe("a successful send", () => {
  it("resolves, and asks Resend for exactly what it should", async () => {
    const calls = stubProvider(200, JSON.stringify({ id: "sent" }));
    const sender = new ResendEmailSender("secret-key", "kosong <login@example.com>");

    await expect(sender.sendVerificationCode(MESSAGE)).resolves.toBeUndefined();

    expect(calls).toHaveLength(1);
    const call = calls[0]!;
    expect(call.url).toBe("https://api.resend.com/emails");
    const headers = call.init.headers as Record<string, string>;
    expect(headers.authorization).toBe("Bearer secret-key");

    const body = JSON.parse(call.init.body as string);
    expect(body.from).toBe("kosong <login@example.com>");
    expect(body.to).toEqual([MESSAGE.to]);
    // The code belongs in the message, and nowhere else.
    expect(body.subject).toContain(MESSAGE.code);
    expect(body.text).toContain(MESSAGE.code);
  });
});

describe("choosing a sender", () => {
  it("uses Resend when a key is configured", () => {
    const env = { RESEND_API_KEY: "key", EMAIL_FROM: "a@b.c" } as unknown as Env;
    expect(emailSenderFor(env)).toBeInstanceOf(ResendEmailSender);
  });

  it("falls back to the console in development", () => {
    // The fallback writes the code to the log, which is only acceptable on a
    // machine where the log and the mailbox belong to the same person.
    const env = { EMAIL_FROM: "a@b.c", ENVIRONMENT: "development" } as unknown as Env;
    expect(emailSenderFor(env)).toBeInstanceOf(ConsoleEmailSender);
  });

  it("refuses to send rather than log codes in production", async () => {
    // This happened: a deployed service ran for about an hour with no key,
    // writing live sign-in codes into its own log because nothing stopped it.
    const env = { EMAIL_FROM: "a@b.c", ENVIRONMENT: "production" } as unknown as Env;
    const sender = emailSenderFor(env);

    expect(sender).not.toBeInstanceOf(ConsoleEmailSender);
    await expect(sender.sendVerificationCode(MESSAGE)).rejects.toBeInstanceOf(
      EmailProviderUnavailable,
    );
  });
});
