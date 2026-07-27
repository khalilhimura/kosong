/**
 * Retention: security events and verification codes do not accumulate forever.
 *
 * The load-bearing test here is not "old rows go away" — it is that sweeping
 * verification codes cannot switch off rate limiting. The limiter counts rows
 * within the last hour, so a retention window shorter than that would disable
 * a real control while every test about deletion still passed.
 */

import { env } from "cloudflare:test";
import { describe, expect, it } from "vitest";
import worker from "../src/index";
import { sweep } from "../src/features/retention/retention";
import {
  MAX_CODES_PER_EMAIL,
  RATE_LIMIT_WINDOW_SECONDS,
  SECURITY_EVENT_RETENTION_SECONDS,
  VERIFICATION_CODE_RETENTION_SECONDS,
} from "../src/shared/config";
import { Logger } from "../src/shared/logging";
import { call, clearCooldown, requestCode, uniqueEmail } from "./helpers";

const DAY = 24 * 60 * 60 * 1000;

function agedIso(millisAgo: number): string {
  return new Date(Date.now() - millisAgo).toISOString();
}

/** Inserts a security event with a chosen age. Returns its id. */
async function insertEvent(millisAgo: number): Promise<string> {
  const id = crypto.randomUUID();
  await env.DB.prepare(
    `INSERT INTO security_events (id, user_id, event_type, request_id, ip_hash, created_at)
     VALUES (?, ?, ?, ?, ?, ?)`,
  )
    .bind(id, null, "code_requested", crypto.randomUUID(), "hash", agedIso(millisAgo))
    .run();
  return id;
}

/** Inserts a verification code row with a chosen age. Returns its id. */
async function insertCode(email: string, millisAgo: number): Promise<string> {
  const id = crypto.randomUUID();
  const created = agedIso(millisAgo);
  await env.DB.prepare(
    `INSERT INTO verification_codes
       (id, email_normalized, code_hash, expires_at, attempt_count, created_at)
     VALUES (?, ?, ?, ?, 0, ?)`,
  )
    .bind(id, email, "hash", created, created)
    .run();
  return id;
}

async function eventExists(id: string): Promise<boolean> {
  const row = await env.DB.prepare(`SELECT id FROM security_events WHERE id = ?`)
    .bind(id)
    .first();
  return row !== null;
}

async function codeExists(id: string): Promise<boolean> {
  const row = await env.DB.prepare(`SELECT id FROM verification_codes WHERE id = ?`)
    .bind(id)
    .first();
  return row !== null;
}

describe("sweeping security events", () => {
  it("deletes rows older than the retention window and keeps newer ones", async () => {
    const stale = await insertEvent(SECURITY_EVENT_RETENTION_SECONDS * 1000 + DAY);
    const fresh = await insertEvent(DAY);

    await sweep(env, Logger.forRequest());

    expect(await eventExists(stale)).toBe(false);
    expect(await eventExists(fresh)).toBe(true);
  });
});

describe("sweeping verification codes", () => {
  it("deletes rows older than the retention window and keeps newer ones", async () => {
    const stale = await insertCode(
      "stale@example.com",
      VERIFICATION_CODE_RETENTION_SECONDS * 1000 + 60_000,
    );
    const fresh = await insertCode("fresh@example.com", 60_000);

    await sweep(env, Logger.forRequest());

    expect(await codeExists(stale)).toBe(false);
    expect(await codeExists(fresh)).toBe(true);
  });

  it("keeps codes old enough to be forgotten but young enough to still be counted", async () => {
    // One minute inside the rate-limit window. The limiter counts this row;
    // a sweep that removes it raises the per-email and per-IP caps silently.
    const counted = await insertCode(
      "counted@example.com",
      (RATE_LIMIT_WINDOW_SECONDS - 60) * 1000,
    );

    await sweep(env, Logger.forRequest());

    expect(await codeExists(counted)).toBe(true);
  });

  it("still refuses a rate-limited address after a sweep", async () => {
    const email = uniqueEmail();
    for (let i = 0; i < MAX_CODES_PER_EMAIL; i += 1) {
      await requestCode(email);
      await clearCooldown(email);
    }

    await sweep(env, Logger.forRequest());

    const response = await call("/v1/auth/code/request", {
      method: "POST",
      body: { email },
    });
    expect(response.status).toBe(429);
  });
});

describe("bounding the work", () => {
  it("stops at the batch cap and reports that it was capped", async () => {
    const old = SECURITY_EVENT_RETENTION_SECONDS * 1000 + DAY;
    for (let i = 0; i < 5; i += 1) await insertEvent(old);

    // Two batches of two: four rows go, the fifth waits for tomorrow.
    const summary = await sweep(env, Logger.forRequest(), { batchSize: 2, maxBatches: 2 });

    const events = summary.find((entry) => entry.table === "security_events");
    expect(events).toMatchObject({ deleted: 4, capped: true });
  });

  it("reports not capped when the work fits", async () => {
    await insertEvent(SECURITY_EVENT_RETENTION_SECONDS * 1000 + DAY);

    const summary = await sweep(env, Logger.forRequest(), { batchSize: 50, maxBatches: 5 });

    expect(summary.find((entry) => entry.table === "security_events")?.capped).toBe(false);
  });
});

describe("the scheduled handler", () => {
  it("sweeps when the cron fires", async () => {
    const stale = await insertEvent(SECURITY_EVENT_RETENTION_SECONDS * 1000 + DAY);

    // The handler awaits its own work rather than deferring it to waitUntil,
    // so there is no execution context to wait on.
    await worker.scheduled?.(
      { cron: "0 3 * * *", scheduledTime: Date.now(), noRetry: () => {} },
      env,
    );

    expect(await eventExists(stale)).toBe(false);
  });
});

describe("the retention windows themselves", () => {
  it("keeps verification codes for longer than rate limiting counts them", () => {
    // Not a style preference. Below this line the limiter stops seeing rows it
    // depends on, and nothing about that failure is visible: codes still send,
    // sign-in still works, and the cap is simply gone.
    expect(VERIFICATION_CODE_RETENTION_SECONDS).toBeGreaterThan(RATE_LIMIT_WINDOW_SECONDS);
  });
});
