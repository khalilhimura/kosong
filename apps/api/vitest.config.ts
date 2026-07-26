import path from "node:path";
import { cloudflareTest, readD1Migrations } from "@cloudflare/vitest-pool-workers";
import { defineConfig } from "vitest/config";

/**
 * Tests run inside workerd against real D1 and R2 bindings, not mocks.
 *
 * That matters for this API. The properties under test are things like "two
 * concurrent verifications cannot both consume one code" and "an If-Match
 * mismatch is rejected" — a mocked database would agree with whatever the code
 * already does, which is the opposite of a test.
 *
 * Migrations are read here and handed to tests as a binding, so the schema
 * under test is the same SQL that ships.
 */
const migrations = await readD1Migrations(path.join(import.meta.dirname, "migrations"));

export default defineConfig({
  plugins: [
    cloudflareTest({
      wrangler: { configPath: "./wrangler.jsonc" },
      miniflare: {
        bindings: {
          TEST_MIGRATIONS: migrations,
          // Test-only secrets. Real ones are set with `wrangler secret put`
          // and never appear in a config file.
          CODE_HMAC_SECRET: "test-code-secret",
          TOKEN_HMAC_SECRET: "test-token-secret",
          IP_HASH_SECRET: "test-ip-secret",
        },
      },
    }),
  ],
  test: {
    setupFiles: ["./test/setup.ts"],
  },
});
