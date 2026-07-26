/**
 * The response contract, redaction, and crypto primitives.
 *
 * §11 fixes the error body shape and the list of things that must never be
 * logged. These are the tests that keep both honest.
 */

import { env } from "cloudflare:test";
import { describe, expect, it } from "vitest";
import {
  constantTimeEqual,
  generateVerificationCode,
  mintAccessToken,
  verifyAccessToken,
} from "../src/shared/crypto";
import { redact } from "../src/shared/logging";
import { call, okfDocument, signIn, toBase64, uniqueEmail } from "./helpers";

describe("health and readiness", () => {
  it("reports liveness without needing configuration", async () => {
    const response = await call("/health");

    expect(response.status).toBe(200);
    expect(await response.json()).toEqual({ status: "ok" });
  });

  it("reports readiness after checking the database", async () => {
    const response = await call("/ready");

    expect(response.status).toBe(200);
    expect(await response.json()).toEqual({ status: "ready" });
  });
});

describe("error contract", () => {
  it("returns code, message, and request_id on every failure", async () => {
    const response = await call("/v1/document");

    expect(response.status).toBe(401);
    const body = await response.json<Record<string, unknown>>();
    expect(body).toHaveProperty("code");
    expect(body).toHaveProperty("message");
    expect(body).toHaveProperty("request_id");
    expect(typeof body.request_id).toBe("string");
  });

  it("surfaces the request id as a header too", async () => {
    const response = await call("/health");
    expect(response.headers.get("x-request-id")).toBeTruthy();
  });

  it("returns 404 for an unknown endpoint", async () => {
    const response = await call("/v1/does-not-exist");

    expect(response.status).toBe(404);
    expect((await response.json<{ code: string }>()).code).toBe("NOT_FOUND");
  });

  it("returns 404 for a known path with the wrong method", async () => {
    const response = await call("/v1/auth/code/request", { method: "GET" });
    expect(response.status).toBe(404);
  });

  it("rejects a body that is not JSON", async () => {
    const request = new Request("https://api.kosong.test/v1/auth/code/request", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: "not json at all",
    });
    const { route } = await import("../src/index");
    const response = await route(request, env);

    expect(response.status).toBe(400);
  });

  it("sets security headers on every response", async () => {
    const response = await call("/health");

    expect(response.headers.get("x-content-type-options")).toBe("nosniff");
    expect(response.headers.get("referrer-policy")).toBe("no-referrer");
    expect(response.headers.get("content-security-policy")).toContain("default-src 'none'");
  });

  it("marks responses uncacheable", async () => {
    const session = await signIn();
    await call("/v1/document", {
      method: "PUT",
      accessToken: session.access_token,
      headers: { "if-match": "*" },
      body: { document: toBase64(okfDocument()) },
    });

    const response = await call("/v1/document", { accessToken: session.access_token });
    expect(response.headers.get("cache-control")).toBe("no-store");
  });
});

describe("redaction", () => {
  it("strips anything shaped like a credential", async () => {
    const output = redact({
      access_token: "secret-value",
      refresh_token: "secret-value",
      code: "123456",
      password: "hunter2",
      authorization: "Bearer abc",
      document: "# my private page",
      email: "person@example.com",
      body: "content",
    });

    for (const value of Object.values(output)) {
      expect(value).toBe("[redacted]");
    }
  });

  it("keeps the fields that carry no secret", () => {
    const output = redact({
      email_domain: "example.com",
      error_code: "RATE_LIMITED",
      user_id: "abc-123",
      status: 429,
    });

    expect(output.email_domain).toBe("example.com");
    expect(output.error_code).toBe("RATE_LIMITED");
    expect(output.user_id).toBe("abc-123");
    expect(output.status).toBe(429);
  });

  it("drops absent values rather than logging null", () => {
    const output = redact({ present: 1, missing: undefined, empty: null });

    expect(Object.keys(output)).toEqual(["present"]);
  });

  it("never writes a raw address into the security event log", async () => {
    const email = uniqueEmail();
    await call("/v1/auth/code/request", { method: "POST", body: { email } });

    const rows = await env.DB.prepare(
      `SELECT metadata_json FROM security_events WHERE metadata_json IS NOT NULL`,
    ).all<{ metadata_json: string }>();

    for (const row of rows.results) {
      expect(row.metadata_json).not.toContain(email);
      // The domain alone is fine; the mailbox is not.
      expect(row.metadata_json).not.toContain(email.split("@")[0]);
    }
  });
});

describe("verification codes", () => {
  it("are always six digits", () => {
    for (let i = 0; i < 500; i++) {
      expect(generateVerificationCode()).toMatch(/^\d{6}$/);
    }
  });

  it("cover the whole range, including leading zeros", () => {
    // A generator that dropped leading zeros would produce short codes; one
    // with a narrow range would fail the spread check.
    const seen = new Set<string>();
    for (let i = 0; i < 3000; i++) seen.add(generateVerificationCode());

    expect(seen.size).toBeGreaterThan(2500);
    const values = [...seen].map(Number);
    expect(Math.min(...values)).toBeLessThan(200_000);
    expect(Math.max(...values)).toBeGreaterThan(800_000);
  });
});

describe("constant-time comparison", () => {
  it("matches identical strings", () => {
    expect(constantTimeEqual("abc123", "abc123")).toBe(true);
  });

  it("rejects differences at any position", () => {
    expect(constantTimeEqual("abc123", "xbc123")).toBe(false);
    expect(constantTimeEqual("abc123", "abc12x")).toBe(false);
  });

  it("rejects different lengths", () => {
    expect(constantTimeEqual("abc", "abcd")).toBe(false);
  });
});

describe("access tokens", () => {
  const secret = "test-token-secret";

  it("round-trip to the user they were minted for", async () => {
    const token = await mintAccessToken(secret, "user-123", 900);
    expect(await verifyAccessToken(secret, token)).toBe("user-123");
  });

  it("fail verification under a different secret", async () => {
    const token = await mintAccessToken(secret, "user-123", 900);
    expect(await verifyAccessToken("a-different-secret", token)).toBeNull();
  });

  it("fail once expired", async () => {
    const now = Math.floor(Date.now() / 1000);
    const token = await mintAccessToken(secret, "user-123", 60, now - 3600);

    expect(await verifyAccessToken(secret, token, now)).toBeNull();
  });

  it("cannot be forged by rewriting the payload", async () => {
    const token = await mintAccessToken(secret, "user-123", 900);
    const [header, , signature] = token.split(".");

    const forgedPayload = btoa(
      JSON.stringify({ sub: "someone-else", exp: Math.floor(Date.now() / 1000) + 900 }),
    )
      .replace(/\+/g, "-")
      .replace(/\//g, "_")
      .replace(/=+$/, "");

    expect(await verifyAccessToken(secret, `${header}.${forgedPayload}.${signature}`)).toBeNull();
  });

  it("cannot be bypassed with an unsigned token", async () => {
    // The `alg: none` attack. Verification is fixed at HS256 by construction
    // and never consults the header, so there is nothing to downgrade.
    const header = btoa(JSON.stringify({ alg: "none", typ: "JWT" }))
      .replace(/=+$/, "")
      .replace(/\+/g, "-")
      .replace(/\//g, "_");
    const payload = btoa(
      JSON.stringify({ sub: "user-123", exp: Math.floor(Date.now() / 1000) + 900 }),
    )
      .replace(/=+$/, "")
      .replace(/\+/g, "-")
      .replace(/\//g, "_");

    expect(await verifyAccessToken(secret, `${header}.${payload}.`)).toBeNull();
    expect(await verifyAccessToken(secret, `${header}.${payload}`)).toBeNull();
  });

  it("reject malformed input without throwing", async () => {
    for (const bad of ["", "a", "a.b", "a.b.c.d", "...", "not-a-token"]) {
      expect(await verifyAccessToken(secret, bad)).toBeNull();
    }
  });
});
