/**
 * Retention: deleting rows that have outlived their purpose.
 *
 * Runs from a cron trigger, not the request path. A sweep on the request path
 * would put delete latency in front of someone's sign-in and, on a quiet
 * service, might never run at all.
 *
 * Nothing here is reachable from a request, and nothing here reads one.
 */

import {
  SECURITY_EVENT_RETENTION_SECONDS,
  VERIFICATION_CODE_RETENTION_SECONDS,
  type Env,
} from "../../shared/config";
import type { Logger } from "../../shared/logging";

/**
 * How much one run may delete.
 *
 * The cap is what keeps a first run against a long-neglected table from
 * running into the wall clock. Ten thousand rows per table per day is far
 * beyond this service's rate of producing them, so in ordinary operation the
 * cap is never reached — and if it is, that is worth seeing.
 */
export interface SweepLimits {
  batchSize: number;
  maxBatches: number;
}

export const DEFAULT_LIMITS: SweepLimits = { batchSize: 500, maxBatches: 20 };

/**
 * What is swept, and how long each is kept.
 *
 * Table names are literals and are interpolated into SQL below, which is safe
 * only because they are literals. Nothing in this list may ever come from a
 * request.
 */
const POLICIES = [
  { table: "security_events", retentionSeconds: SECURITY_EVENT_RETENTION_SECONDS },
  { table: "verification_codes", retentionSeconds: VERIFICATION_CODE_RETENTION_SECONDS },
] as const;

export interface SweepResult {
  table: string;
  deleted: number;
  /** True when the cap stopped the run with rows still eligible. */
  capped: boolean;
  failed?: boolean;
}

function cutoff(retentionSeconds: number): string {
  return new Date(Date.now() - retentionSeconds * 1000).toISOString();
}

/** Deletes rows past their retention window, in bounded batches. */
export async function sweep(
  env: Env,
  logger: Logger,
  limits: SweepLimits = DEFAULT_LIMITS,
): Promise<SweepResult[]> {
  const results: SweepResult[] = [];

  for (const policy of POLICIES) {
    // One table failing must not stop the other. A sweep that cannot clean
    // the audit log should still clear the plaintext addresses.
    try {
      results.push(await sweepTable(env, policy.table, policy.retentionSeconds, limits));
    } catch (error) {
      logger.error({
        event: "retention_sweep_failed",
        outcome: "failure",
        detail: `${policy.table}:${error instanceof Error ? error.name : "unknown"}`,
      });
      results.push({ table: policy.table, deleted: 0, capped: false, failed: true });
    }
  }

  for (const result of results) {
    if (result.failed) continue;
    logger.log(result.capped ? "warn" : "info", {
      event: "retention_swept",
      outcome: "success",
      detail: `${result.table} deleted=${result.deleted}${result.capped ? " capped" : ""}`,
    });
  }

  return results;
}

async function sweepTable(
  env: Env,
  table: string,
  retentionSeconds: number,
  limits: SweepLimits,
): Promise<SweepResult> {
  const before = cutoff(retentionSeconds);
  let deleted = 0;

  for (let batch = 0; batch < limits.maxBatches; batch += 1) {
    const result = await env.DB.prepare(
      `DELETE FROM ${table}
        WHERE id IN (SELECT id FROM ${table} WHERE created_at < ? LIMIT ?)`,
    )
      .bind(before, limits.batchSize)
      .run();

    const changes = result.meta.changes ?? 0;
    deleted += changes;

    // A short batch means the eligible rows ran out.
    if (changes < limits.batchSize) {
      return { table, deleted, capped: false };
    }
  }

  return { table, deleted, capped: true };
}
