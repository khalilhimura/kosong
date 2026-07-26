/**
 * Authentication logic.
 *
 * The controls implemented here map directly onto the threat table in
 * technical specification §14:
 *
 * | Threat | Control |
 * |---|---|
 * | Code guessing | CSPRNG code, keyed hash at rest, 10 min expiry, 5 attempts, rate limits |
 * | Account enumeration | Identical response and work whether or not the account exists |
 * | Token theft and reuse | Short access lifetime, hashed rotating refresh tokens, family revocation |
 */

import {
  ACCESS_TOKEN_TTL_SECONDS,
  CODE_REQUEST_COOLDOWN_SECONDS,
  CODE_TTL_SECONDS,
  MAX_CODES_PER_EMAIL,
  MAX_CODES_PER_IP,
  MAX_VERIFY_ATTEMPTS,
  RATE_LIMIT_WINDOW_SECONDS,
  REFRESH_TOKEN_TTL_SECONDS,
  documentKey,
  turnstileEnabled,
  type Env,
} from "../../shared/config";
import {
  constantTimeEqual,
  generateId,
  generateOpaqueToken,
  generateVerificationCode,
  hmacHex,
  mintAccessToken,
  sha256Hex,
} from "../../shared/crypto";
import { ApiError } from "../../shared/errors";
import {
  emailDomain,
  recordSecurityEvent,
  type Logger,
} from "../../shared/logging";
import { AuthRepository } from "./auth.repository";
import {
  EmailProviderUnavailable,
  EmailRecipientRejected,
  emailSenderFor,
  type EmailSender,
} from "./email";

/**
 * Normalizes an email address for storage and comparison.
 *
 * Trim and lowercase only. Provider-specific tricks — stripping dots, cutting
 * at `+` — are deliberately not applied: they encode assumptions about one
 * mail provider, and getting them wrong silently merges two people's accounts.
 */
export function normalizeEmail(email: string): string {
  return email.trim().toLowerCase();
}

/** The tokens and safe user summary returned on a successful sign-in. */
export interface SessionResult {
  access_token: string;
  refresh_token: string;
  user: { id: string; email_hint: string };
}

/**
 * Masks an address for display: `k***@example.com`.
 *
 * Enough for a person to recognise their own address, not enough to expose
 * someone else's if it ever appears in the wrong place.
 */
export function emailHint(email: string): string {
  const at = email.lastIndexOf("@");
  if (at <= 0) return "***";
  return `${email[0]}***${email.slice(at)}`;
}

export class AuthService {
  private readonly repository: AuthRepository;
  private readonly email: EmailSender;

  constructor(
    private readonly env: Env,
    private readonly logger: Logger,
    emailSender?: EmailSender,
  ) {
    this.repository = new AuthRepository(env);
    this.email = emailSender ?? emailSenderFor(env);
  }

  // -- Request a code -----------------------------------------------------

  /**
   * Issues a verification code.
   *
   * Performs the same work whether or not an account exists — the users table
   * is not consulted at all, because accounts are created on *verification*.
   * That makes enumeration by timing structurally impossible here rather than
   * something to be tuned.
   */
  async requestCode(input: {
    email: string;
    ipHash?: string;
    turnstileToken?: string;
  }): Promise<void> {
    const emailNormalized = normalizeEmail(input.email);

    if (turnstileEnabled(this.env)) {
      await this.assertHuman(input.turnstileToken);
    }

    await this.assertWithinLimits(emailNormalized, input.ipHash);

    const code = generateVerificationCode();
    const codeHash = await hmacHex(this.env.CODE_HMAC_SECRET, code);
    const expiresAt = new Date(Date.now() + CODE_TTL_SECONDS * 1000).toISOString();

    await this.repository.insertVerificationCode({
      emailNormalized,
      codeHash,
      expiresAt,
      ipHash: input.ipHash,
    });

    try {
      await this.email.sendVerificationCode({ to: emailNormalized, code });
    } catch (error) {
      if (error instanceof EmailProviderUnavailable) {
        // Ours to own. Answering "a code is on its way" would leave the user
        // waiting for something that is never going to arrive.
        this.logger.error({
          event: "email_provider_unavailable",
          outcome: "failure",
          email_domain: emailDomain(emailNormalized),
        });
        throw new ApiError(
          503,
          "EMAIL_UNAVAILABLE",
          "Codes cannot be sent right now. Please try again in a few minutes.",
        );
      }

      if (!(error instanceof EmailRecipientRejected)) {
        throw error;
      }

      // The provider will not deliver to this address. The caller is told
      // nothing, deliberately: §8.2 requires one answer for every request, and
      // a distinct one here would say which addresses a provider accepts.
      // The code stays stored and simply expires unused.
      this.logger.warn({
        event: "code_undeliverable",
        outcome: "failure",
        email_domain: emailDomain(emailNormalized),
        ip_hash: input.ipHash,
      });
      return;
    }

    this.logger.info({
      event: "code_requested",
      outcome: "success",
      email_domain: emailDomain(emailNormalized),
      ip_hash: input.ipHash,
    });
    await recordSecurityEvent(this.env, this.logger, {
      type: "code_requested",
      ipHash: input.ipHash,
      metadata: { email_domain: emailDomain(emailNormalized) },
    });
  }

  /** Enforces the cooldown and both rate limits. */
  private async assertWithinLimits(
    emailNormalized: string,
    ipHash?: string,
  ): Promise<void> {
    const now = Date.now();
    const windowStart = new Date(now - RATE_LIMIT_WINDOW_SECONDS * 1000).toISOString();

    const lastRequest = await this.repository.lastCodeRequestAt(emailNormalized);
    if (lastRequest) {
      const elapsed = (now - Date.parse(lastRequest)) / 1000;
      if (elapsed < CODE_REQUEST_COOLDOWN_SECONDS) {
        await this.rateLimited(ipHash, "cooldown");
        throw ApiError.rateLimited(Math.ceil(CODE_REQUEST_COOLDOWN_SECONDS - elapsed));
      }
    }

    const perEmail = await this.repository.countRecentCodesByEmail(
      emailNormalized,
      windowStart,
    );
    if (perEmail >= MAX_CODES_PER_EMAIL) {
      await this.rateLimited(ipHash, "per_email");
      throw ApiError.rateLimited(RATE_LIMIT_WINDOW_SECONDS);
    }

    if (ipHash) {
      const perIp = await this.repository.countRecentCodesByIp(ipHash, windowStart);
      if (perIp >= MAX_CODES_PER_IP) {
        await this.rateLimited(ipHash, "per_ip");
        throw ApiError.rateLimited(RATE_LIMIT_WINDOW_SECONDS);
      }
    }
  }

  private async rateLimited(ipHash: string | undefined, reason: string): Promise<void> {
    this.logger.warn({ event: "code_request_rate_limited", outcome: "failure", detail: reason });
    await recordSecurityEvent(this.env, this.logger, {
      type: "code_request_rate_limited",
      ipHash,
      metadata: { reason },
    });
  }

  /** Checks a Turnstile token against Cloudflare's siteverify endpoint. */
  private async assertHuman(token: string | undefined): Promise<void> {
    if (!token || !this.env.TURNSTILE_SECRET) {
      throw ApiError.invalidInput("That request could not be verified.");
    }

    const body = new FormData();
    body.append("secret", this.env.TURNSTILE_SECRET);
    body.append("response", token);

    const response = await fetch(
      "https://challenges.cloudflare.com/turnstile/v0/siteverify",
      { method: "POST", body },
    );
    const result = (await response.json()) as { success?: boolean };

    if (!result.success) {
      throw ApiError.invalidInput("That request could not be verified.");
    }
  }

  // -- Verify a code ------------------------------------------------------

  /**
   * Verifies a code and returns a session, creating the account if new.
   *
   * Every failure produces the identical message. Distinguishing "no such
   * code" from "wrong code" from "expired" would hand an attacker a way to
   * probe which addresses have codes outstanding.
   */
  async verifyCode(input: {
    email: string;
    code: string;
    deviceName?: string;
    ipHash?: string;
  }): Promise<SessionResult> {
    const emailNormalized = normalizeEmail(input.email);
    const now = new Date().toISOString();
    const rejection = ApiError.invalidInput("That code is not right, or it has expired.");

    const record = await this.repository.findActiveCode(emailNormalized, now);
    if (!record) {
      await this.verifyFailed(input.ipHash, "no_active_code");
      throw rejection;
    }

    // Counted before comparison, so abandoning the request mid-flight still
    // costs an attempt.
    const attempts = await this.repository.incrementAttempt(record.id);
    if (attempts === null || attempts > MAX_VERIFY_ATTEMPTS) {
      await this.repository.exhaustCode(record.id, now);
      this.logger.warn({ event: "code_verify_exhausted", outcome: "failure" });
      await recordSecurityEvent(this.env, this.logger, {
        type: "code_verify_exhausted",
        ipHash: input.ipHash,
      });
      throw rejection;
    }

    const providedHash = await hmacHex(this.env.CODE_HMAC_SECRET, input.code);
    if (!constantTimeEqual(providedHash, record.code_hash)) {
      await this.verifyFailed(input.ipHash, "wrong_code");
      throw rejection;
    }

    // Single-use, enforced by the database rather than by this check.
    if (!(await this.repository.consumeCode(record.id, now))) {
      await this.verifyFailed(input.ipHash, "already_consumed");
      throw rejection;
    }

    const { id: userId, created } = await this.repository.findOrCreateUser(emailNormalized);
    const session = await this.issueSession(userId, generateId(), input.deviceName);

    this.logger.info({
      event: "code_verified",
      outcome: "success",
      user_id: userId,
      detail: created ? "account_created" : "existing_account",
    });
    await recordSecurityEvent(this.env, this.logger, {
      type: "code_verified",
      userId,
      ipHash: input.ipHash,
      metadata: { account_created: created },
    });

    return {
      ...session,
      user: { id: userId, email_hint: emailHint(emailNormalized) },
    };
  }

  private async verifyFailed(ipHash: string | undefined, reason: string): Promise<void> {
    this.logger.warn({ event: "code_verify_failed", outcome: "failure", detail: reason });
    await recordSecurityEvent(this.env, this.logger, {
      type: "code_verify_failed",
      ipHash,
      metadata: { reason },
    });
  }

  // -- Refresh ------------------------------------------------------------

  /**
   * Rotates a refresh token.
   *
   * Presenting an already-rotated token means either an attacker stole it or
   * the legitimate client replayed one. Neither can be distinguished from the
   * server, so the whole family is revoked and everyone signs in again. Losing
   * a session beats leaving a thief with a working credential.
   */
  async refresh(refreshToken: string): Promise<Omit<SessionResult, "user">> {
    const tokenHash = await sha256Hex(refreshToken);
    const now = new Date().toISOString();

    const session = await this.repository.findSessionByTokenHash(tokenHash);
    if (!session) {
      throw ApiError.sessionExpired("Please sign in again.");
    }

    if (session.revoked_at !== null) {
      const revoked = await this.repository.revokeFamily(session.family_id, now);
      this.logger.warn({
        event: "refresh_reuse_detected",
        outcome: "failure",
        user_id: session.user_id,
        detail: `revoked ${revoked} sessions`,
      });
      await recordSecurityEvent(this.env, this.logger, {
        type: "refresh_reuse_detected",
        userId: session.user_id,
        metadata: { revoked_count: revoked },
      });
      throw ApiError.sessionExpired("Please sign in again.");
    }

    if (Date.parse(session.expires_at) <= Date.now()) {
      throw ApiError.sessionExpired("Please sign in again.");
    }

    // The old token stops working the moment the new one exists.
    await this.repository.revokeSession(session.id, now);
    await this.repository.touchSession(session.id, now);

    const issued = await this.issueSession(session.user_id, session.family_id);

    this.logger.info({
      event: "session_refreshed",
      outcome: "success",
      user_id: session.user_id,
    });
    return issued;
  }

  /** Mints an access and refresh token pair within one family. */
  private async issueSession(
    userId: string,
    familyId: string,
    deviceName?: string,
  ): Promise<Omit<SessionResult, "user">> {
    const refreshToken = generateOpaqueToken();

    await this.repository.createRefreshSession({
      userId,
      // Hashed at rest: a database leak yields no usable token.
      tokenHash: await sha256Hex(refreshToken),
      familyId,
      deviceName,
      expiresAt: new Date(Date.now() + REFRESH_TOKEN_TTL_SECONDS * 1000).toISOString(),
    });

    const accessToken = await mintAccessToken(
      this.env.TOKEN_HMAC_SECRET,
      userId,
      ACCESS_TOKEN_TTL_SECONDS,
    );

    return { access_token: accessToken, refresh_token: refreshToken };
  }

  // -- Logout -------------------------------------------------------------

  /**
   * Revokes a refresh token.
   *
   * Succeeds even for an unknown token: signing out is not an operation that
   * should report whether a credential was real.
   */
  async logout(refreshToken: string): Promise<void> {
    const session = await this.repository.findSessionByTokenHash(
      await sha256Hex(refreshToken),
    );
    if (!session) return;

    await this.repository.revokeSession(session.id, new Date().toISOString());
    this.logger.info({
      event: "session_revoked",
      outcome: "success",
      user_id: session.user_id,
    });
    await recordSecurityEvent(this.env, this.logger, {
      type: "session_revoked",
      userId: session.user_id,
    });
  }

  // -- Account deletion ---------------------------------------------------

  /**
   * Deletes an account and everything belonging to it.
   *
   * Idempotent, so a retry after a network failure cannot leave a
   * half-deleted account. The document object is removed first: an orphaned
   * database row is a tidiness problem, whereas orphaned bytes with no row to
   * point at them are a data-retention one.
   */
  async deleteAccount(userId: string): Promise<void> {
    const emailNormalized = await this.repository.emailForUser(userId);

    await this.env.DOCUMENTS.delete(documentKey(userId));
    await this.repository.deleteUserRecords(userId, emailNormalized);

    this.logger.info({ event: "account_deleted", outcome: "success", user_id: userId });
    await recordSecurityEvent(this.env, this.logger, {
      type: "account_deleted",
      userId,
    });
  }
}
