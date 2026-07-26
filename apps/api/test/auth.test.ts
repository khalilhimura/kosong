/**
 * Authentication lifecycle and the §14 threat controls.
 *
 * Each `describe` below corresponds to a row of the threat table: code
 * guessing, account enumeration, token theft and reuse.
 */

import { env } from "cloudflare:test";
import { describe, expect, it } from "vitest";
import { MAX_VERIFY_ATTEMPTS } from "../src/shared/config";
import {
  bodyWithoutRequestId,
  call,
  clearCooldown,
  expireCode,
  requestCode,
  signIn,
  uniqueEmail,
} from "./helpers";

describe("requesting a code", () => {
  it("accepts a valid address and sends a six-digit code", async () => {
    const email = uniqueEmail();
    const code = await requestCode(email);

    expect(code).toMatch(/^\d{6}$/);
  });

  it("rejects an address that is not an address", async () => {
    const response = await call("/v1/auth/code/request", {
      method: "POST",
      body: { email: "not-an-email" },
    });

    expect(response.status).toBe(400);
    expect((await response.json<{ code: string }>()).code).toBe("INVALID_INPUT");
  });

  it("stores the code only as a hash", async () => {
    const email = uniqueEmail();
    const code = await requestCode(email);

    const row = await env.DB.prepare(
      `SELECT code_hash FROM verification_codes WHERE email_normalized = ?`,
    )
      .bind(email.toLowerCase())
      .first<{ code_hash: string }>();

    expect(row).not.toBeNull();
    expect(row!.code_hash).not.toContain(code);
    // HMAC-SHA256, hex encoded.
    expect(row!.code_hash).toMatch(/^[0-9a-f]{64}$/);
  });

  it("normalizes the address before storing it", async () => {
    const email = uniqueEmail().toUpperCase();
    await requestCode(email);

    const row = await env.DB.prepare(
      `SELECT email_normalized FROM verification_codes WHERE email_normalized = ?`,
    )
      .bind(email.toLowerCase())
      .first();

    expect(row).not.toBeNull();
  });
});

describe("account enumeration", () => {
  it("responds identically whether or not the account exists", async () => {
    // A brand new address.
    const fresh = uniqueEmail();
    const freshResponse = await call("/v1/auth/code/request", {
      method: "POST",
      body: { email: fresh },
    });

    // An address that already has an account.
    const existing = await signIn();
    await clearCooldown(existing.email);
    const existingResponse = await call("/v1/auth/code/request", {
      method: "POST",
      body: { email: existing.email },
    });

    expect(freshResponse.status).toBe(existingResponse.status);
    expect(await bodyWithoutRequestId(freshResponse)).toEqual(
      await bodyWithoutRequestId(existingResponse),
    );
  });

  it("never reveals in an error whether a code exists for an address", async () => {
    const unknown = await call("/v1/auth/code/verify", {
      method: "POST",
      body: { email: uniqueEmail(), code: "000000" },
    });

    const email = uniqueEmail();
    await requestCode(email);
    const wrongCode = await call("/v1/auth/code/verify", {
      method: "POST",
      body: { email, code: "000000" },
    });

    expect(unknown.status).toBe(wrongCode.status);
    expect(await bodyWithoutRequestId(unknown)).toEqual(
      await bodyWithoutRequestId(wrongCode),
    );
  });
});

describe("rate limiting", () => {
  it("enforces a cooldown between requests for one address", async () => {
    const email = uniqueEmail();
    await requestCode(email);

    const second = await call("/v1/auth/code/request", {
      method: "POST",
      body: { email },
    });

    expect(second.status).toBe(429);
    expect((await second.json<{ code: string }>()).code).toBe("RATE_LIMITED");
    expect(second.headers.get("retry-after")).toBeTruthy();
  });

  it("caps how many codes one address can receive in the window", async () => {
    const email = uniqueEmail();

    // Five allowed; the cooldown is cleared between them so the per-email
    // limit is what is being tested.
    for (let i = 0; i < 5; i++) {
      await requestCode(email);
      await clearCooldown(email);
    }

    const blocked = await call("/v1/auth/code/request", {
      method: "POST",
      body: { email },
    });
    expect(blocked.status).toBe(429);
  });
});

describe("verifying a code", () => {
  it("signs in and creates the account on first use", async () => {
    const session = await signIn();

    expect(session.access_token).toBeTruthy();
    expect(session.refresh_token).toBeTruthy();
    expect(session.user.id).toBeTruthy();

    const user = await env.DB.prepare(`SELECT id FROM users WHERE id = ?`)
      .bind(session.user.id)
      .first();
    expect(user).not.toBeNull();
  });

  it("returns a masked address, never the full one", async () => {
    const session = await signIn();

    expect(session.user.email_hint).toMatch(/^.\*\*\*@/);
    expect(session.user.email_hint).not.toBe(session.email);
  });

  it("reuses the existing account on a later sign-in", async () => {
    const first = await signIn();
    await clearCooldown(first.email);

    const code = await requestCode(first.email);
    const response = await call("/v1/auth/code/verify", {
      method: "POST",
      body: { email: first.email, code },
    });
    const second = await response.json<{ user: { id: string } }>();

    expect(second.user.id).toBe(first.user.id);
  });

  it("refuses a code that has already been used", async () => {
    const email = uniqueEmail();
    const code = await requestCode(email);

    const first = await call("/v1/auth/code/verify", {
      method: "POST",
      body: { email, code },
    });
    expect(first.status).toBe(200);

    const second = await call("/v1/auth/code/verify", {
      method: "POST",
      body: { email, code },
    });
    expect(second.status).toBe(400);
  });

  it("refuses an expired code", async () => {
    const email = uniqueEmail();
    const code = await requestCode(email);
    await expireCode(email);

    const response = await call("/v1/auth/code/verify", {
      method: "POST",
      body: { email, code },
    });

    expect(response.status).toBe(400);
  });

  it("refuses a code belonging to a different address", async () => {
    const mine = uniqueEmail();
    const code = await requestCode(mine);
    const theirs = uniqueEmail();
    await requestCode(theirs);

    const response = await call("/v1/auth/code/verify", {
      method: "POST",
      body: { email: theirs, code },
    });

    expect(response.status).toBe(400);
  });

  it("stops accepting guesses after the attempt limit", async () => {
    const email = uniqueEmail();
    const code = await requestCode(email);

    // Exhaust every attempt with wrong codes. The real code is avoided so the
    // limit, not a lucky guess, is what ends the loop.
    for (let i = 0; i < MAX_VERIFY_ATTEMPTS; i++) {
      const wrong = String((Number(code) + i + 1) % 1_000_000).padStart(6, "0");
      await call("/v1/auth/code/verify", {
        method: "POST",
        body: { email, code: wrong },
      });
    }

    // The correct code no longer works.
    const response = await call("/v1/auth/code/verify", {
      method: "POST",
      body: { email, code },
    });
    expect(response.status).toBe(400);
  });

  it("rejects a code that is not six digits before touching the database", async () => {
    for (const code of ["12345", "1234567", "abcdef", "", "12 345"]) {
      const response = await call("/v1/auth/code/verify", {
        method: "POST",
        body: { email: uniqueEmail(), code },
      });
      expect(response.status).toBe(400);
      expect((await response.json<{ code: string }>()).code).toBe("INVALID_INPUT");
    }
  });
});

describe("refresh tokens", () => {
  it("rotates the token on every use", async () => {
    const session = await signIn();

    const response = await call("/v1/auth/refresh", {
      method: "POST",
      body: { refresh_token: session.refresh_token },
    });
    expect(response.status).toBe(200);

    const rotated = await response.json<{ access_token: string; refresh_token: string }>();
    expect(rotated.refresh_token).not.toBe(session.refresh_token);
    expect(rotated.access_token).toBeTruthy();
  });

  it("stores refresh tokens only as hashes", async () => {
    const session = await signIn();

    const rows = await env.DB.prepare(`SELECT token_hash FROM refresh_sessions`).all<{
      token_hash: string;
    }>();

    for (const row of rows.results) {
      expect(row.token_hash).not.toBe(session.refresh_token);
      expect(row.token_hash).toMatch(/^[0-9a-f]{64}$/);
    }
  });

  it("revokes the whole family when a rotated token is presented again", async () => {
    // The reuse-detection control: presenting an already-rotated token means
    // either theft or replay, and neither is distinguishable, so every
    // descendant of that login is revoked.
    const session = await signIn();

    const first = await call("/v1/auth/refresh", {
      method: "POST",
      body: { refresh_token: session.refresh_token },
    });
    const rotated = await first.json<{ refresh_token: string }>();

    // Present the old one again.
    const reuse = await call("/v1/auth/refresh", {
      method: "POST",
      body: { refresh_token: session.refresh_token },
    });
    expect(reuse.status).toBe(401);

    // The token issued by the legitimate rotation is now dead too.
    const afterRevocation = await call("/v1/auth/refresh", {
      method: "POST",
      body: { refresh_token: rotated.refresh_token },
    });
    expect(afterRevocation.status).toBe(401);
  });

  it("records reuse as a security event", async () => {
    const session = await signIn();
    await call("/v1/auth/refresh", {
      method: "POST",
      body: { refresh_token: session.refresh_token },
    });
    await call("/v1/auth/refresh", {
      method: "POST",
      body: { refresh_token: session.refresh_token },
    });

    const event = await env.DB.prepare(
      `SELECT event_type FROM security_events WHERE event_type = 'refresh_reuse_detected'`,
    ).first();

    expect(event).not.toBeNull();
  });

  it("rejects an unknown token", async () => {
    const response = await call("/v1/auth/refresh", {
      method: "POST",
      body: { refresh_token: "not-a-real-token" },
    });
    expect(response.status).toBe(401);
  });
});

describe("logout", () => {
  it("revokes the session", async () => {
    const session = await signIn();

    const response = await call("/v1/auth/logout", {
      method: "POST",
      body: { refresh_token: session.refresh_token },
    });
    expect(response.status).toBe(204);

    const afterLogout = await call("/v1/auth/refresh", {
      method: "POST",
      body: { refresh_token: session.refresh_token },
    });
    expect(afterLogout.status).toBe(401);
  });

  it("succeeds for an unknown token, revealing nothing", async () => {
    const response = await call("/v1/auth/logout", {
      method: "POST",
      body: { refresh_token: "never-existed" },
    });
    expect(response.status).toBe(204);
  });
});

describe("account deletion", () => {
  it("removes the user, their sessions, and their document", async () => {
    const session = await signIn();

    await call("/v1/document", {
      method: "PUT",
      accessToken: session.access_token,
      headers: { "if-match": "*" },
      body: { document: btoa("---\ntype: Page\n---\n# Mine\n") },
    });

    const response = await call("/v1/account", {
      method: "DELETE",
      accessToken: session.access_token,
    });
    expect(response.status).toBe(200);

    const user = await env.DB.prepare(`SELECT id FROM users WHERE id = ?`)
      .bind(session.user.id)
      .first();
    expect(user).toBeNull();

    const document = await env.DB.prepare(`SELECT user_id FROM documents WHERE user_id = ?`)
      .bind(session.user.id)
      .first();
    expect(document).toBeNull();

    const object = await env.DOCUMENTS.get(`users/${session.user.id}/document-v1`);
    expect(object).toBeNull();

    const sessions = await env.DB.prepare(
      `SELECT id FROM refresh_sessions WHERE user_id = ?`,
    )
      .bind(session.user.id)
      .all();
    expect(sessions.results).toHaveLength(0);
  });

  it("is idempotent, so a retry cannot half-delete an account", async () => {
    const session = await signIn();

    const first = await call("/v1/account", {
      method: "DELETE",
      accessToken: session.access_token,
    });
    expect(first.status).toBe(200);

    // The access token is self-contained and still within its lifetime, so
    // the second call reaches the service exactly as a retry would.
    const second = await call("/v1/account", {
      method: "DELETE",
      accessToken: session.access_token,
    });
    expect(second.status).toBe(200);
  });

  it("requires authentication", async () => {
    const response = await call("/v1/account", { method: "DELETE" });
    expect(response.status).toBe(401);
  });
});
