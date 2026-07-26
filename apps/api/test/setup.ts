import { applyD1Migrations, env } from "cloudflare:test";

/**
 * Applies the real migrations before any test runs.
 *
 * The schema under test is therefore the same SQL that ships, so a migration
 * that would fail in production fails here first.
 */
await applyD1Migrations(env.DB, env.TEST_MIGRATIONS);
