/**
 * Secrets, declared as part of the environment.
 *
 * `wrangler types` generates `Cloudflare.Env` from `wrangler.jsonc`, which
 * covers bindings and plain vars. It cannot know about secrets, because those
 * are set with `wrangler secret put` and deliberately appear in no config
 * file.
 *
 * Augmenting the generated interface here keeps one source of truth for the
 * environment's shape: bindings come from the config, secrets from this file.
 * Maintaining a parallel hand-written `Env` instead would drift the moment a
 * binding was added.
 */
declare global {
  namespace Cloudflare {
    interface Env {
      /** Hashes verification codes at rest. */
      CODE_HMAC_SECRET: string;
      /** Signs access tokens and hashes refresh tokens. */
      TOKEN_HMAC_SECRET: string;
      /** Hashes client IPs before they are stored. */
      IP_HASH_SECRET: string;
      /** Transactional email. Absent locally, which selects the console sender. */
      RESEND_API_KEY?: string;
      /** Only required when TURNSTILE_ENABLED is "true". */
      TURNSTILE_SECRET?: string;
    }
  }
}

export {};
