/**
 * Transactional email, behind an interface.
 *
 * The technical specification names Resend but requires a replaceable
 * provider boundary, so the rest of the code depends on {@link EmailSender}
 * and never on Resend directly. Swapping providers means adding a class here.
 *
 * The tests use {@link RecordingEmailSender}, which is also how a test can
 * assert that a code was sent without a real code ever leaving the process.
 */

import type { Env } from "../../shared/config";

export interface VerificationEmail {
  to: string;
  code: string;
}

export interface EmailSender {
  sendVerificationCode(message: VerificationEmail): Promise<void>;
}

/**
 * The provider refused this address.
 *
 * A fact about the recipient, not about the service: retrying changes
 * nothing. The caller must still answer as it answers every other code
 * request, because a different answer here would tell an attacker which
 * addresses a provider will accept.
 */
export class EmailRecipientRejected extends Error {
  override readonly name = "EmailRecipientRejected";
}

/**
 * The provider could not be reached, or failed on its own account.
 *
 * A fact about the service. Worth telling the user about, because the code
 * they are waiting for is never going to arrive.
 */
export class EmailProviderUnavailable extends Error {
  override readonly name = "EmailProviderUnavailable";
}

/** Resend. */
export class ResendEmailSender implements EmailSender {
  constructor(
    private readonly apiKey: string,
    private readonly from: string,
  ) {}

  async sendVerificationCode({ to, code }: VerificationEmail): Promise<void> {
    let response: Response;
    try {
      response = await fetch("https://api.resend.com/emails", {
        method: "POST",
        headers: {
          authorization: `Bearer ${this.apiKey}`,
          "content-type": "application/json",
        },
        body: JSON.stringify({
          from: this.from,
          to: [to],
          subject: `${code} is your kosong sign-in code`,
          text: bodyText(code),
        }),
      });
    } catch {
      // Never reached the provider at all. The error is not inspected: it can
      // carry the request, and the request carries the address.
      throw new EmailProviderUnavailable("could not reach the email provider");
    }

    if (response.ok) {
      return;
    }

    // The provider's response body may echo the address, so only the status
    // is surfaced.
    //
    // Only these two are statements about the recipient: a malformed address,
    // or a domain the provider will not deliver to. Everything else — an
    // expired key, an unpaid account, a rate limit, a fault at their end — is
    // a statement about *us*, and must not be quietly absorbed as though a
    // user had mistyped their address. A revoked API key that read as "bad
    // recipient" would answer every sign-in with "a code is on its way" while
    // sending nothing, and nothing would say otherwise.
    if (response.status === 400 || response.status === 422) {
      throw new EmailRecipientRejected(
        `email provider rejected the recipient (${response.status})`,
      );
    }

    throw new EmailProviderUnavailable(`email provider returned ${response.status}`);
  }
}

/**
 * Writes the code to the log instead of sending it.
 *
 * For local development only. Selected when no API key is configured, and it
 * says so loudly, because a deployment that silently stopped sending email
 * would look identical to one that worked.
 */
export class ConsoleEmailSender implements EmailSender {
  async sendVerificationCode({ code }: VerificationEmail): Promise<void> {
    console.log(
      JSON.stringify({
        level: "warn",
        event: "email_not_configured",
        detail: "RESEND_API_KEY is unset, so no email was sent",
        // Safe only because this path requires a missing key, which cannot
        // happen in a correctly deployed environment.
        development_only_code: code,
      }),
    );
  }
}

/**
 * Refuses to send, loudly.
 *
 * Selected when a deployed service has no email provider configured — the one
 * situation {@link ConsoleEmailSender} must never be reached in, since it
 * writes verification codes into the log. That happened once, for about an
 * hour, because nothing enforced the difference between "not configured yet"
 * and "not configured, in production".
 *
 * Failing every sign-in is the right outcome: the alternative is a service
 * that appears to work while quietly publishing the credentials it issues.
 */
export class MisconfiguredEmailSender implements EmailSender {
  async sendVerificationCode(): Promise<void> {
    console.log(
      JSON.stringify({
        level: "error",
        event: "email_not_configured",
        detail:
          "RESEND_API_KEY is unset in a production deployment; no code was sent, and none will be",
      }),
    );
    throw new EmailProviderUnavailable("no email provider is configured");
  }
}

/** Captures messages in memory. Tests only. */
export class RecordingEmailSender implements EmailSender {
  readonly sent: VerificationEmail[] = [];

  async sendVerificationCode(message: VerificationEmail): Promise<void> {
    this.sent.push(message);
  }
}

function bodyText(code: string): string {
  return [
    `Your kosong sign-in code is ${code}`,
    "",
    "It works once, and expires in 10 minutes.",
    "",
    "If you did not ask to sign in, you can ignore this message.",
    "Nobody can use this code without also having access to your terminal.",
  ].join("\n");
}

/**
 * Picks a sender for the environment.
 *
 * The production branch is the load-bearing one. Logging codes is a
 * development convenience, and a convenience that follows an unconfigured key
 * into production is a way of handing out sign-in codes to anyone who can read
 * a log. A deployed service without a provider fails instead, and says why.
 */
export function emailSenderFor(env: Env): EmailSender {
  if (env.RESEND_API_KEY) {
    return new ResendEmailSender(env.RESEND_API_KEY, env.EMAIL_FROM);
  }
  if (env.ENVIRONMENT === "production") {
    return new MisconfiguredEmailSender();
  }
  return new ConsoleEmailSender();
}
