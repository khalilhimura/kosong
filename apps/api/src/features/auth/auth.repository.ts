/**
 * D1 access for authentication.
 *
 * All SQL lives here. Every statement uses bound parameters — there is no
 * string interpolation into SQL anywhere in this file, which is what keeps
 * injection off the table structurally rather than by review.
 *
 * The two operations that must be atomic are {@link consumeCode} and
 * {@link incrementAttempt}. Both express their guard inside the `WHERE`
 * clause and check `meta.changes`, so two concurrent verifications cannot both
 * succeed against one code.
 */

import type { Env } from "../../shared/config";
import { generateId } from "../../shared/crypto";

export interface VerificationCodeRow {
  id: string;
  email_normalized: string;
  code_hash: string;
  expires_at: string;
  consumed_at: string | null;
  attempt_count: number;
}

export interface RefreshSessionRow {
  id: string;
  user_id: string;
  family_id: string;
  expires_at: string;
  revoked_at: string | null;
}

export class AuthRepository {
  constructor(private readonly env: Env) {}

  // -- Verification codes -------------------------------------------------

  async insertVerificationCode(input: {
    emailNormalized: string;
    codeHash: string;
    expiresAt: string;
    ipHash?: string;
  }): Promise<string> {
    const id = generateId();
    await this.env.DB.prepare(
      `INSERT INTO verification_codes
         (id, email_normalized, code_hash, expires_at, attempt_count, request_ip_hash, created_at)
       VALUES (?, ?, ?, ?, 0, ?, ?)`,
    )
      .bind(
        id,
        input.emailNormalized,
        input.codeHash,
        input.expiresAt,
        input.ipHash ?? null,
        new Date().toISOString(),
      )
      .run();
    return id;
  }

  /**
   * The newest unconsumed, unexpired code for an address.
   *
   * Newest wins: requesting a second code should let the second code work.
   */
  async findActiveCode(
    emailNormalized: string,
    now: string,
  ): Promise<VerificationCodeRow | null> {
    return this.env.DB.prepare(
      `SELECT id, email_normalized, code_hash, expires_at, consumed_at, attempt_count
         FROM verification_codes
        WHERE email_normalized = ?
          AND consumed_at IS NULL
          AND expires_at > ?
        ORDER BY created_at DESC
        LIMIT 1`,
    )
      .bind(emailNormalized, now)
      .first<VerificationCodeRow>();
  }

  /**
   * Increments the attempt counter and returns the new value.
   *
   * Incrementing *before* comparing the code means a failed guess costs an
   * attempt even if the request is abandoned mid-flight.
   */
  async incrementAttempt(id: string): Promise<number | null> {
    const row = await this.env.DB.prepare(
      `UPDATE verification_codes
          SET attempt_count = attempt_count + 1
        WHERE id = ?
        RETURNING attempt_count`,
    )
      .bind(id)
      .first<{ attempt_count: number }>();
    return row?.attempt_count ?? null;
  }

  /**
   * Marks a code used. Returns false if it was already used.
   *
   * The `consumed_at IS NULL` guard is what makes a code single-use under
   * concurrency: only one of two racing requests changes a row.
   */
  async consumeCode(id: string, now: string): Promise<boolean> {
    const result = await this.env.DB.prepare(
      `UPDATE verification_codes
          SET consumed_at = ?
        WHERE id = ? AND consumed_at IS NULL`,
    )
      .bind(now, id)
      .run();
    return (result.meta.changes ?? 0) === 1;
  }

  /** Burns all remaining attempts, killing a code that has been guessed at too often. */
  async exhaustCode(id: string, now: string): Promise<void> {
    await this.env.DB.prepare(
      `UPDATE verification_codes SET consumed_at = ? WHERE id = ?`,
    )
      .bind(now, id)
      .run();
  }

  // -- Rate limiting ------------------------------------------------------

  async countRecentCodesByEmail(emailNormalized: string, since: string): Promise<number> {
    const row = await this.env.DB.prepare(
      `SELECT COUNT(*) AS total FROM verification_codes
        WHERE email_normalized = ? AND created_at > ?`,
    )
      .bind(emailNormalized, since)
      .first<{ total: number }>();
    return row?.total ?? 0;
  }

  async countRecentCodesByIp(ipHash: string, since: string): Promise<number> {
    const row = await this.env.DB.prepare(
      `SELECT COUNT(*) AS total FROM verification_codes
        WHERE request_ip_hash = ? AND created_at > ?`,
    )
      .bind(ipHash, since)
      .first<{ total: number }>();
    return row?.total ?? 0;
  }

  async lastCodeRequestAt(emailNormalized: string): Promise<string | null> {
    const row = await this.env.DB.prepare(
      `SELECT created_at FROM verification_codes
        WHERE email_normalized = ?
        ORDER BY created_at DESC LIMIT 1`,
    )
      .bind(emailNormalized)
      .first<{ created_at: string }>();
    return row?.created_at ?? null;
  }

  // -- Users --------------------------------------------------------------

  /**
   * Returns the user id for an address, creating the account if needed.
   *
   * `ON CONFLICT DO NOTHING` followed by a select is safe under concurrency:
   * whichever request inserts first wins, and both then read the same row.
   */
  async findOrCreateUser(emailNormalized: string): Promise<{ id: string; created: boolean }> {
    const id = generateId();
    const insert = await this.env.DB.prepare(
      `INSERT INTO users (id, email_normalized, created_at)
       VALUES (?, ?, ?)
       ON CONFLICT(email_normalized) DO NOTHING`,
    )
      .bind(id, emailNormalized, new Date().toISOString())
      .run();

    if ((insert.meta.changes ?? 0) === 1) {
      return { id, created: true };
    }

    const existing = await this.env.DB.prepare(
      `SELECT id FROM users WHERE email_normalized = ?`,
    )
      .bind(emailNormalized)
      .first<{ id: string }>();

    if (!existing) {
      throw new Error("user row vanished between insert and select");
    }
    return { id: existing.id, created: false };
  }

  async userExists(userId: string): Promise<boolean> {
    const row = await this.env.DB.prepare(`SELECT 1 AS present FROM users WHERE id = ?`)
      .bind(userId)
      .first<{ present: number }>();
    return row !== null;
  }

  // -- Refresh sessions ---------------------------------------------------

  async createRefreshSession(input: {
    userId: string;
    tokenHash: string;
    familyId: string;
    deviceName?: string;
    expiresAt: string;
  }): Promise<string> {
    const id = generateId();
    await this.env.DB.prepare(
      `INSERT INTO refresh_sessions
         (id, user_id, token_hash, family_id, device_name, expires_at, created_at)
       VALUES (?, ?, ?, ?, ?, ?, ?)`,
    )
      .bind(
        id,
        input.userId,
        input.tokenHash,
        input.familyId,
        input.deviceName ?? null,
        input.expiresAt,
        new Date().toISOString(),
      )
      .run();
    return id;
  }

  /**
   * Looks up a session by token hash, including revoked ones.
   *
   * Revoked sessions are returned deliberately: presenting one is how token
   * reuse is detected, and that signal is lost if the query filters them out.
   */
  async findSessionByTokenHash(tokenHash: string): Promise<RefreshSessionRow | null> {
    return this.env.DB.prepare(
      `SELECT id, user_id, family_id, expires_at, revoked_at
         FROM refresh_sessions WHERE token_hash = ?`,
    )
      .bind(tokenHash)
      .first<RefreshSessionRow>();
  }

  async revokeSession(id: string, now: string): Promise<void> {
    await this.env.DB.prepare(
      `UPDATE refresh_sessions SET revoked_at = ? WHERE id = ? AND revoked_at IS NULL`,
    )
      .bind(now, id)
      .run();
  }

  /** Revokes every session descended from one login. */
  async revokeFamily(familyId: string, now: string): Promise<number> {
    const result = await this.env.DB.prepare(
      `UPDATE refresh_sessions SET revoked_at = ? WHERE family_id = ? AND revoked_at IS NULL`,
    )
      .bind(now, familyId)
      .run();
    return result.meta.changes ?? 0;
  }

  async revokeAllForUser(userId: string, now: string): Promise<void> {
    await this.env.DB.prepare(
      `UPDATE refresh_sessions SET revoked_at = ? WHERE user_id = ? AND revoked_at IS NULL`,
    )
      .bind(now, userId)
      .run();
  }

  async touchSession(id: string, now: string): Promise<void> {
    await this.env.DB.prepare(`UPDATE refresh_sessions SET last_used_at = ? WHERE id = ?`)
      .bind(now, id)
      .run();
  }

  // -- Account deletion ---------------------------------------------------

  /**
   * Removes every authentication record for a user.
   *
   * Idempotent: deleting an already-deleted account is a no-op, so a retried
   * request cannot fail. Verification codes are cleared by address so a code
   * in flight cannot resurrect the account.
   */
  async deleteUserRecords(userId: string, emailNormalized: string | null): Promise<void> {
    const statements = [
      this.env.DB.prepare(`DELETE FROM refresh_sessions WHERE user_id = ?`).bind(userId),
      this.env.DB.prepare(`DELETE FROM documents WHERE user_id = ?`).bind(userId),
      this.env.DB.prepare(`DELETE FROM users WHERE id = ?`).bind(userId),
    ];
    if (emailNormalized) {
      statements.push(
        this.env.DB.prepare(`DELETE FROM verification_codes WHERE email_normalized = ?`).bind(
          emailNormalized,
        ),
      );
    }
    await this.env.DB.batch(statements);
  }

  async emailForUser(userId: string): Promise<string | null> {
    const row = await this.env.DB.prepare(
      `SELECT email_normalized FROM users WHERE id = ?`,
    )
      .bind(userId)
      .first<{ email_normalized: string }>();
    return row?.email_normalized ?? null;
  }
}
