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

/** Resend. */
export class ResendEmailSender implements EmailSender {
  constructor(
    private readonly apiKey: string,
    private readonly from: string,
  ) {}

  async sendVerificationCode({ to, code }: VerificationEmail): Promise<void> {
    const response = await fetch("https://api.resend.com/emails", {
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

    if (!response.ok) {
      // The provider's response body may echo the address, so only the status
      // is surfaced. The caller turns this into a generic failure.
      throw new Error(`email provider returned ${response.status}`);
    }
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

/** Picks a sender for the environment. */
export function emailSenderFor(env: Env): EmailSender {
  if (env.RESEND_API_KEY) {
    return new ResendEmailSender(env.RESEND_API_KEY, env.EMAIL_FROM);
  }
  return new ConsoleEmailSender();
}
