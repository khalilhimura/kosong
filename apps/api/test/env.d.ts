import type { D1Migration } from "@cloudflare/vitest-pool-workers";

/**
 * Extra bindings visible to tests.
 *
 * `cloudflare:test` resolves `env` as `Cloudflare.Env`, the interface
 * `wrangler types` generates, so the migration list injected by
 * `vitest.config.ts` is merged into that namespace rather than declared on
 * `ProvidedEnv`.
 */
declare global {
  namespace Cloudflare {
    interface Env {
      TEST_MIGRATIONS: D1Migration[];
    }
  }
}

export {};
