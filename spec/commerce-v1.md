# kosong Commerce and Release Integrity v1

**Status:** Normative from `kosong` v0.5. Release gate for the first paid feature.
**Last updated:** 2026-07-31

Nothing in `kosong` is sold yet. This document is written before anything is,
because every control it describes is cheap now and expensive once money is
moving and accounts exist that have paid.

It defines **how an entitlement is established, stored, read, and enforced**.
It deliberately does not decide *what is sold* — that is a product question,
and a mechanism that only works for one answer to it is a mechanism that will
be rewritten. Sections 2 to 4 hold whether the first paid thing is more
storage, more documents, a custom domain, or something not yet imagined.

Part II is unrelated to money and ships independently. It is here because it
is the other half of "before we take payment": a release nobody can verify is
not a release worth charging for.

---

## Part I — Commerce

## 1. The rule

**Authority over what a user may do lives on the server, is read from D1 at
the moment it is used, and is never asserted by the client.**

Everything below is a consequence of that sentence. It is not a general
principle borrowed from elsewhere; it is forced by two facts specific to this
project:

- The CLI is Apache-2.0 and its source is public. A check compiled into it is
  a check a fork removes.
- `KOSONG_API_URL` already lets any user point the CLI at any host, and must
  keep doing so — it is how the Worker is developed against locally.

So a client-side entitlement check is not weak. It is decorative. The only
question worth asking of any control here is: *does this still hold when the
caller wrote their own client?*

This restates architectural rule 4 in the technical specification — "Remote
API owns identity and authorization" — for a case §2 did not have to consider,
because in v1 every authenticated user could do exactly the same things.

## 2. Entitlements

### 2.1 Schema

New migration, `apps/api/migrations/0004_entitlements.sql`:

```sql
-- What a user is entitled to, and the billing relationship behind it.
--
-- Absence of a row means the free plan. That is deliberate: it needs no
-- backfill for existing accounts, adds no write to the sign-in path, and
-- fails closed — a row that was never written cannot grant anything.
--
-- Document content and email addresses never appear here, per §10 of the
-- technical specification. The provider ids below are opaque handles issued
-- by the payment provider; they identify a billing relationship, not a person.

CREATE TABLE entitlements (
  user_id TEXT PRIMARY KEY REFERENCES users(id),

  -- The plan name. Compared for equality only, never parsed. Adding a plan is
  -- adding a value here and a row to the limits table in shared/plans.ts.
  plan TEXT NOT NULL,

  -- active | past_due | canceled.
  --
  -- Separate from `plan` because "was on pro, payment failed" and "is on free"
  -- are different states that deserve different messages, and collapsing them
  -- loses the ability to tell someone their card was declined.
  status TEXT NOT NULL,

  -- Which payment provider issued this. Present so a second provider, or a
  -- migration between providers, does not require reading rows ambiguously.
  provider TEXT NOT NULL,
  provider_customer_id TEXT NOT NULL,
  provider_subscription_id TEXT,

  -- When the paid period ends. A canceled subscription usually stays usable
  -- until this passes; §2.2 is what enforces that.
  current_period_end TEXT,

  -- The `created` timestamp of the most recent provider event applied to this
  -- row. Webhook deliveries are not ordered, and without this an old
  -- `subscription.updated` arriving after `subscription.deleted` silently
  -- resurrects a cancelled subscription. §3.5 is the check that uses it.
  provider_event_at TEXT NOT NULL,

  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

-- The webhook's only lookup: provider customer id to user. A webhook never
-- receives a kosong user id and must never be trusted with one.
CREATE UNIQUE INDEX entitlements_provider_customer_idx
  ON entitlements(provider, provider_customer_id);
```

There is no `entitlements` row for a free user, and `deleteAccount` removes the
row like any other. The `REFERENCES users(id)` is real: an entitlement without
an account is not a state this service has.

### 2.2 Reading an entitlement

New file, `apps/api/src/features/entitlements/entitlement.repository.ts`, and
`entitlement.service.ts` beside it. The service exposes exactly one function to
the rest of the codebase:

```ts
/** What this user may do, right now. Never cached across requests. */
export async function limitsFor(env: Env, userId: string): Promise<PlanLimits>;
```

`PlanLimits` is a plain record of numbers and booleans, defined in
`apps/api/src/shared/plans.ts` alongside the plan table. Callers ask for a
limit; they never ask for a plan name and branch on it. A `if (plan === "pro")`
scattered through the codebase is how a plan rename becomes an outage.

The resolution rules, in order:

| Condition | Result |
|---|---|
| No row | `FREE_LIMITS` |
| `status = 'active'` | that plan's limits |
| `status = 'past_due'` | that plan's limits, until `current_period_end` passes |
| `status = 'canceled'` | that plan's limits, until `current_period_end` passes |
| `current_period_end` is past | `FREE_LIMITS`, whatever `status` says |

The last row is the one that matters. It means a lapsed entitlement degrades on
its own, with no cron, no sweep, and no webhook required. If every billing
webhook were dropped on the floor for a month, the worst outcome is that users
keep paid access until the period they already paid for runs out — which is what
they are owed anyway. The failure mode of the billing integration is therefore
*correct behaviour*, not *free service forever*.

`past_due` grants access deliberately. A failed card retry is not theft, and
locking someone out of their own document over it is a worse error than a few
days of unpaid service.

### 2.3 Why the entitlement is not in the access token

It is tempting. The access token is already self-contained precisely so a
document request needs no D1 round trip, and adding `plan` to the payload would
keep it that way.

It must not be done, and the reason is in `crypto.ts` already: an access token
"cannot be revoked before it expires, which is why the lifetime is short and
revocation lives at the refresh layer." An entitlement claim inherits that
property. A downgrade, a cancellation, a refund, or a chargeback would keep
working for up to `ACCESS_TOKEN_TTL_SECONDS`, and there would be no mechanism
to shorten it. Worse, the refresh token lives 30 days, so a client that never
refreshes early is not bounded by 15 minutes in practice.

The D1 read is one indexed primary-key lookup on a row of scalars, in the same
region as the Worker, on a request that is already doing an R2 fetch. The cost
is not the reason to avoid it, and there is no other reason.

**Normative:** the access token payload stays `{ sub, iat, exp }`. Any proposal
to add a field to it is a change to `spec/threat-model-v1.md` §3.1 and needs
that document updated first.

## 3. The billing provider boundary

### 3.1 The interface

`EmailSender` is the established pattern for this and is named in
`spec/extension-boundaries.md` §5: the codebase depends on an interface, one
implementation talks to the vendor, and the tests use a recording double. The
billing provider follows it exactly.

New file, `apps/api/src/features/billing/provider.ts`:

```ts
/** A normalized billing event. The rest of the codebase sees only this. */
export interface BillingEvent {
  /** The provider's own event id. The idempotency key; never generated here. */
  id: string;
  /** When the provider created the event. Used for ordering, per §3.5. */
  createdAt: string;
  type: "entitlement_changed" | "entitlement_revoked" | "ignored";
  /** Absent when `type` is "ignored". */
  entitlement?: {
    providerCustomerId: string;
    providerSubscriptionId: string | null;
    plan: string;
    status: "active" | "past_due" | "canceled";
    currentPeriodEnd: string | null;
  };
}

export interface BillingProvider {
  readonly name: string;
  /**
   * Verifies the signature over the raw body, then parses.
   *
   * Verification and parsing are one call so no caller can obtain a parsed
   * event that was never verified. There is no `parse` to reach for.
   */
  verify(rawBody: string, signatureHeader: string | null): Promise<BillingEvent>;
}
```

`type: "ignored"` is load-bearing. A provider sends event types this service
does not care about, and those must be acknowledged with 200 rather than
errored — a provider that gets a 4xx retries, then disables the endpoint.
Mapping the uninteresting to an explicit variant means "we ignored this" is a
decision the code states rather than a gap it falls through.

The first implementation is Stripe (`stripe.ts`), because it is what most
projects reach for and its signature scheme is the one described below. Nothing
outside that file mentions it.

### 3.2 The route

`POST /v1/billing/webhook`, added to the `switch` in `apps/api/src/index.ts`.

It sits **below** the `requireSecrets` gate with the other routes, but
`requireSecrets` is **not** extended to cover `BILLING_WEBHOOK_SECRET`. Adding
it there would mean a missing billing secret takes down sign-in and sync for
everybody. The webhook checks its own secret and returns `NOT_READY` alone:

```ts
if (!env.BILLING_WEBHOOK_SECRET) {
  logger.error({ event: "billing_not_configured", outcome: "failure" });
  throw new ApiError(503, "NOT_READY", "This service is not ready yet.");
}
```

`BILLING_WEBHOOK_SECRET` and `BILLING_PROVIDER` are declared in
`apps/api/src/env.d.ts` beside the existing secrets, both optional, with the
comment that they are required only once billing is live.

This endpoint is unauthenticated by necessity — the payment provider holds no
kosong session. Its signature check is therefore the *whole* of its
authentication, which is why §3.3 is written as strictly as it is.

### 3.3 Signature verification happens before parsing, and over raw bytes

**Normative, and the single most important line in this document:**

```ts
const rawBody = await request.text();
const event = await provider.verify(rawBody, request.headers.get("stripe-signature"));
```

Never `await request.json()` first. A `Request` body is a stream and can be
read once; parsing first destroys the exact bytes the signature covers, and the
usual repair — re-serializing the parsed object — does not reproduce them,
because key order, whitespace, and number formatting all differ. Code that
does this appears to work in every test written against its own serializer and
fails against the provider.

The Stripe implementation:

1. Parse the `stripe-signature` header into `t=<unix seconds>` and one or more
   `v1=<hex>` values.
2. Reject if `|now - t|` exceeds `WEBHOOK_TOLERANCE_SECONDS` (300). This is
   what stops an attacker replaying a body and signature captured earlier.
3. Compute `HMAC-SHA256(BILLING_WEBHOOK_SECRET, "${t}.${rawBody}")`, hex.
4. Compare against each `v1` with the existing `constantTimeEqual` from
   `shared/crypto.ts`. Multiple `v1` values occur during a secret rotation and
   any match is sufficient.
5. Only then `JSON.parse(rawBody)`.

Every failure throws `ApiError.invalidInput()` and is logged as
`billing_signature_rejected`. No failure reports *which* step failed, for the
same reason `verifyAccessToken` returns null for everything.

### 3.4 Idempotency, and the order that makes it safe

New table in the same migration:

```sql
-- Provider events already applied. The primary key is the provider's own event
-- id, which is what makes a duplicate delivery a no-op.
CREATE TABLE billing_events (
  provider_event_id TEXT PRIMARY KEY,
  provider TEXT NOT NULL,
  event_type TEXT NOT NULL,
  -- Null when the event named a customer this service does not know.
  user_id TEXT,
  created_at TEXT NOT NULL
);

CREATE INDEX billing_events_created_idx ON billing_events(created_at);
```

The order is: **apply the entitlement, then record the event.**

Recording first and applying second is the obvious arrangement and it is wrong
here: if the apply then fails, the retry finds the id already present, skips,
and the entitlement is never written — a user who paid does not get what they
paid for, silently, and nothing is red.

Applying first risks processing a duplicate delivery twice, which is harmless
because the entitlement write sets **absolute state, never a delta**. Writing
`plan='pro', status='active', current_period_end=X` twice leaves the same row
as writing it once. There is no `credits = credits + n` anywhere in this
design, and there must not be; that is the only shape of write that would make
this ordering unsafe.

### 3.5 Out-of-order delivery

Webhook deliveries are not ordered, and retries make reordering likely rather
than theoretical. The guard is the `provider_event_at` column:

```sql
UPDATE entitlements
   SET plan = ?, status = ?, ..., provider_event_at = ?, updated_at = ?
 WHERE user_id = ?
   AND provider_event_at <= ?
```

An event older than the last one applied changes nothing. `meta.changes === 0`
after this statement means either "no such row" or "older event"; the caller
distinguishes them with a read, logs `billing_event_stale`, and returns 200 —
a stale event is correctly handled, not failed.

For the insert case, `INSERT ... ON CONFLICT(user_id) DO UPDATE SET ... WHERE
excluded.provider_event_at >= entitlements.provider_event_at` gives the same
protection in one statement.

### 3.6 What the webhook may never do

| Never | Because |
|---|---|
| Read a kosong `user_id` from the payload | The payload is attacker-shaped until verified, and even after verification it reflects the provider's state, not ours. The user is found via `provider_customer_id`. |
| Create a user | An account exists only after an email is verified. A webhook that could mint accounts is an unauthenticated account-creation endpoint. |
| Grant an entitlement to an unknown customer | Log `billing_customer_unknown`, record the event, return 200. Provider-side state that has no local account is the provider's to reconcile. |
| Trust `plan` as a free-form string from the payload | Map the provider's price/product id to a known plan name through a lookup in `stripe.ts`. An unrecognized id is `type: "ignored"` plus a `billing_unknown_price` log — never a plan the limits table has no row for. |
| Return non-2xx for an event it chose not to act on | Providers retry 4xx/5xx and eventually disable endpoints. "Ignored" is a success. |

### 3.7 Establishing the link

The webhook can only find a user if `provider_customer_id` is already
associated with one. That association is made when the user starts checkout,
not when the webhook arrives:

`POST /v1/billing/checkout` — authenticated with `requireCaller`, creates (or
reuses) the provider customer for `caller.userId`, writes the `entitlements`
row with `plan='free', status='canceled', current_period_end=NULL` and the
customer id, and returns the provider's hosted checkout URL. The CLI opens it
in a browser.

Writing the row before payment is intentional: it is the only moment both
identities are known to the same authenticated request. The row grants nothing
— §2.2 resolves it to `FREE_LIMITS` — it exists solely so the webhook has
something to find.

**kosong never handles card details.** The hosted page is the provider's. No
payment instrument, PAN, or CVV touches this service, the CLI, or D1. This is
the same boundary as `gh` and `wrangler`: the credential stays with the party
whose job it is to hold it.

## 4. Quotas

`shared/plans.ts` holds the table:

```ts
export interface PlanLimits {
  maxDocumentBytes: number;
  maxDocuments: number;
}

export const FREE_LIMITS: PlanLimits = {
  maxDocumentBytes: MAX_DOCUMENT_BYTES,  // 1 MiB, unchanged
  maxDocuments: 1,
};
```

Enforcement is in `DocumentService.write`, in the same place and the same style
as the existing size check, and it uses the limits rather than the constant:

```ts
const limits = await limitsFor(this.env, input.userId);
const bytes = this.decodeAndValidate(input.documentBase64, limits.maxDocumentBytes);
```

`decodeAndValidate` takes the limit as a parameter instead of reading
`MAX_DOCUMENT_BYTES` directly. `ApiError.tooLarge(limit)` already interpolates
the limit into its message, so a paid user is told their own limit and not
someone else's.

Two rules govern any future limit:

**The limit is checked at the point of use, against a value read this request.**
Not at sign-in, not from a cached plan, not from the token.

**A limit is enforced by the database wherever the database can express it.**
`documents.user_id PRIMARY KEY` is what makes "one document per user" true
today — not the absence of a second insert, but a constraint that would reject
one. When `maxDocuments` first exceeds 1, that primary key is dropped, and the
structural guarantee goes with it. The migration that drops it must add the
counting check to `write` in the same commit, and a test that a user at their
limit is refused. This is the single highest-risk change in this document and
it must not be split across two releases.

Exceeding a document count limit returns a new error code, `QUOTA_EXCEEDED`
(HTTP 402), added to the `ErrorCode` union in `shared/errors.ts`. Size
overruns keep returning `DOCUMENT_TOO_LARGE` — the CLI already handles it, and
a paid user hitting a larger ceiling has hit the same wall for the same reason.

402 rather than 403: the condition is *pay to remove it*, which is exactly what
the status means, and it is distinguishable by a script from a permission
failure that paying would not fix.

## 5. Abuse, and the Turnstile problem

### 5.1 Turnstile cannot be enabled as currently wired

`TURNSTILE_ENABLED` is `"false"` in `wrangler.jsonc`, and `assertHuman` is
correct: no token means `ApiError.invalidInput`, which fails closed.

**Setting it to `"true"` today would break every sign-in.** `crates/` contains
no reference to Turnstile; `ApiClient::request_code` posts `{ email }` and
nothing else. Every `kosong login` would receive "That request could not be
verified", including for users who already have accounts. The flag looks like
a switch that hardens the service and is in fact a switch that disables it.

This is not a defect in the Worker. Turnstile is a browser challenge, and the
sign-in path it protects has no browser in it.

**Normative:** `TURNSTILE_ENABLED` must not be set to `"true"` until either
§5.2 or §5.3 is implemented. A comment saying so belongs in `wrangler.jsonc`
next to the flag, because the flag's current comment — "Turnstile is off until
a sending domain and site key exist" — states a precondition that has since
been met and implies the only remaining work is configuration.

### 5.2 What the flag is actually for

Turnstile stays in the codebase and stays correct, because it guards the right
thing the moment there is a browser: the checkout page of §3.7, and any future
web sign-up. `turnstileEnabled(env)` gating `assertHuman` inside
`requestCode` is the wrong placement for a CLI-only client and the right
placement for a web one. When a web surface exists, the flag turns on for it.

### 5.3 What protects the CLI path instead

Until then, account creation is gated by controlling an email address, plus the
existing per-email and per-IP limits. That has been sufficient because a free
account is worth almost nothing: one document, 1 MiB, no compute. Farming
accounts gains an attacker a megabyte each.

That calculation changes the moment a free tier is worth having in bulk. Before
the first paid release, and only if the free tier grows beyond one document:

- Lower `MAX_CODES_PER_IP` from 20/hour. It was set for a service where a
  successful signup was worthless.
- Add a disposable-domain check on `normalizeEmail`'s output, as a *counted
  and logged* signal first and a rejection only after the counts justify it.
  Rejecting on a list on day one blocks real users of privacy-respecting mail
  providers, which is a population this project's users overlap with heavily.
- Consider requiring a verified payment method for any tier above free, which
  is a far stronger anti-abuse control than a CAPTCHA and one the payment
  provider already implements.

If the free tier stays at one document and one megabyte, **do none of this.**
The current limits are proportionate and adding friction to sign-in to protect
a megabyte is a bad trade.

## 6. Deletion, retention, and the invoice question

`deleteAccount` deletes everything the user has. That property is stated in
`SECURITY.md` and in `guide/`, and it survives this document unchanged.

The apparent conflict — that taking payment creates records which must be kept
for tax purposes, commonly seven years, and which therefore cannot be deleted
on request — is resolved by not holding those records here.

**The financial record lives with the payment provider.** Invoices, amounts,
tax, and the customer's billing identity are the provider's, retained under
their obligations. kosong stores `provider_customer_id`, which is an opaque
handle to that record, and deletes it with the account like anything else. The
provider retains its own row; deleting a kosong account does not, and must not
attempt to, delete the provider's ledger.

So `deleteAccount` gains exactly two things:

1. Cancel any active subscription at the provider before deleting local rows.
   A deleted account that keeps billing a card is the worst possible failure
   of this system and is worth a dedicated test.
2. `DELETE FROM entitlements WHERE user_id = ?`, and set
   `billing_events.user_id = NULL` for that user rather than deleting the rows
   — the events are a record that a webhook was processed, needed for
   idempotency if a late retry arrives after deletion, and once detached from
   the user they identify nobody.

`billing_events` is added to the retention `POLICIES` in `retention.ts` with
its own window, `BILLING_EVENT_RETENTION_SECONDS`. It must be comfortably
longer than any provider's retry window (days, not hours), so a retry can never
outlive the record that would suppress it. Ninety days matches
`SECURITY_EVENT_RETENTION_SECONDS` and is far beyond any provider's retry
schedule.

Confirm the retention obligation against the operator's own jurisdiction before
the first charge. The design above is what makes it *someone else's* obligation;
it is not a substitute for checking that it is.

## 7. Logging and redaction

`security_events.metadata_json` carries the comment that it "must never hold a
code, token, email address, or document bytes." Billing identifiers join that
list — a `provider_customer_id` is a durable cross-service identifier for a
person who has paid, and belongs in D1's `entitlements` row, not scattered
through an audit table with a 90-day tail.

`FORBIDDEN_KEY_PATTERNS` in `shared/logging.ts` gains entries so the structural
defence covers them rather than relying on call sites: `customer`,
`subscription`, `invoice`, `card`, `payment`. The existing mechanism drops any
field whose key contains a forbidden pattern, so this is one line and no new
concept.

New `SecurityEventType` values, appended (the type's comment says add, never
rename):

```
billing_checkout_started
billing_signature_rejected
billing_event_applied
billing_event_stale
billing_customer_unknown
entitlement_changed
quota_exceeded
```

`billing_signature_rejected` is the one to alert on. A steady rate of it means
someone is probing the webhook, and it is the only endpoint in this service
that an unauthenticated stranger is expected to reach.

## 8. Threat table

In the form of `spec/threat-model-v1.md` §3, to be merged into it on adoption.

| Threat | Control | Test |
|---|---|---|
| Forged webhook grants a paid plan | HMAC-SHA256 over raw bytes with a timestamped prefix, constant-time compare, 300s tolerance; verification and parsing are one call so an unverified event cannot be obtained | `billing.test.ts` → *rejects a body whose signature does not match*; *rejects a valid signature older than the tolerance* |
| Replayed webhook | Timestamp tolerance, plus `billing_events` keyed on the provider event id | *applies a duplicate delivery exactly once* |
| Reordered webhook resurrects a cancelled plan | `provider_event_at` monotonic guard on every write | *ignores an event older than the one already applied* |
| Forked CLI claims a paid plan | No entitlement is read from the client. `limitsFor` reads D1 on every enforced operation | *a request carrying a forged plan claim gets free limits* |
| Stale entitlement after downgrade or refund | Entitlement is never in the access token; `current_period_end` expires it without any webhook arriving | *a row past its period end resolves to free limits* |
| Webhook mints an account | The webhook has no path to `findOrCreateUser`; an unknown customer is recorded and ignored | *does not create a user for an unknown customer* |
| One user's entitlement reaching another | `user_id` comes from `requireCaller` on authenticated routes and from a `provider_customer_id` lookup on the webhook; never from a request body | *does not apply an event to a user named in the payload* |
| Quota bypass by concurrent writes | The document count limit is a database constraint wherever expressible, and the check is inside `write` | *refuses a write from a user at their document limit* |
| Billing identifiers in the audit log | `FORBIDDEN_KEY_PATTERNS` drops them structurally | *drops a customer id passed in event metadata* |
| A deleted account still being charged | `deleteAccount` cancels at the provider before deleting local rows | *cancels the subscription when the account is deleted* |

---

## Part II — Release integrity

Independent of Part I. Shippable first, and should be.

## 9. Attestations are generated but never verified

`release.yml` runs `actions/attest-build-provenance@v4`, and the npm job
publishes with `--provenance` under OIDC trusted publishing. Both are right and
neither is currently load-bearing, because nothing on the installing side ever
checks them.

`install.sh` fetches `SHA256SUMS` from the same GitHub Release as the tarball,
then verifies the tarball against it. That defends against a corrupted or
intercepted download. It does not defend against a compromised release: whoever
can replace the archive can replace the manifest beside it. The Homebrew
formula inherits the same property, since `formula.mjs` reads its checksums
from that release too.

For most projects this is an acceptable gap. For this one it is worth closing,
because the README's argument for `curl | sh` is precisely that the script
"downloads a fixed release and refuses to install anything whose checksum does
not match" — an argument that is stronger than the mechanism behind it.

**The change to `install.sh`:** after the checksum passes and before the
archive is unpacked, if `gh` is present and authenticated, run

```sh
gh attestation verify "$work/$archive" --repo "$REPO"
```

and fail the install if it fails. If `gh` is absent, say so in one line and
continue — the checksum check is what we have today and remains the floor. This
must not become a hard dependency on `gh`: an installer that requires the
GitHub CLI to install a tool whose whole promise is that it needs nothing is
self-defeating.

**The change to `README.md`:** the manual-install path currently shows
`shasum -a 256 -c SHA256SUMS --ignore-missing`. Add the attestation command
beside it, with one sentence on what each proves — the checksum proves the
bytes match the manifest, the attestation proves the manifest came from this
repository's workflow.

**Homebrew:** no change. A tap cannot run arbitrary verification, and the
formula's embedded checksums are what Homebrew's model provides. Say so in
`homebrew/README.md` rather than leaving the difference undocumented.

## 10. Workflow permissions

`release.yml` declares `contents: write` at the workflow level, so it applies
to the `build` job, which cross-compiles and needs no write access to anything.
The `npm` job already narrows its own permissions correctly and is the model.

Move `contents: write` to the `publish` job. Set the workflow-level default to
`contents: read`. `id-token: write` and `attestations: write` stay where the
attestation step needs them, on `build`.

This is hardening rather than a fix — no exploit path was identified — and it
belongs in the same commit as §9 because both touch release integrity and both
are cheap.

## 11. Secret hygiene

`resend.md` in the repository root holds a live Resend API key in plaintext.
It is listed in `.gitignore` and is untracked, so it has not reached the public
repository.

It is still an exposure. A working tree is copied, archived, backed up, and
indexed by tools that do not read `.gitignore`, and `git add -f` needs no
special intent to type. A key that grants the ability to send mail as the
project's sign-in domain is worth more than its inconvenience.

**Required, before any of the above ships:**

1. Rotate the key at Resend, invalidating the current one.
2. Set the new key with `wrangler secret put RESEND_API_KEY` and nowhere else.
3. Delete `resend.md`.
4. Remove the `resend.md` line from `.gitignore`. A gitignore entry for a file
   that should not exist documents the practice rather than preventing it, and
   the next key will be written to the same path because the entry says it is
   handled.
5. Confirm `.gitleaks.toml` would catch a Resend key if one were staged.
   `useDefault = true` carries gitleaks' own ruleset, which covers many
   providers but should not be assumed to cover this one. Verify with a
   throwaway string of the right shape; if it passes unflagged, add:

   ```toml
   [[rules]]
   id = "resend-api-key"
   description = "Resend API key"
   regex = '''re_[A-Za-z0-9]{8,}_[A-Za-z0-9]{16,}'''
   keywords = ["re_"]
   ```

   A rule that never fires because nobody checked it is the same as no rule,
   and this is the exact key that was sitting in the working tree.

The same applies to `BILLING_WEBHOOK_SECRET` when it is created: `wrangler
secret put`, never a file, and `apps/api/.dev.vars` for local development only
— which is already gitignored and already the documented pattern.

---

## 12. What this document does not do

In the form of `spec/extension-boundaries.md`, so the absences are on the
record rather than looking like oversights.

| Not here | Why |
|---|---|
| A price, a plan name, or a paid feature | Product decisions. The mechanism holds for any of them; picking one in a spec would date it immediately. |
| Metered or usage-based billing | Every write in §3.4 sets absolute state, which is what makes duplicate delivery safe. Metering needs accumulating counters and a different idempotency design. Do not add a counter to `entitlements`. |
| Team or shared-document plans | `documents.user_id PRIMARY KEY` and `documentKey(userId)` assume one owner per document. Sharing is a change to the authorization model, not to the billing model, and belongs in its own spec. |
| Proration, credits, refunds, dunning | The payment provider implements all four. kosong reads the resulting subscription state and nothing more. |
| Tax calculation | The provider's, and jurisdiction-specific. |
| A second payment provider | The `BillingProvider` interface exists so one can be added. Adding it before there is a reason is how an abstraction acquires the shape of exactly one implementation while claiming otherwise. |

## 13. Release gate

The first paid release ships only when all of the following are true.

- [ ] §11 complete. The Resend key is rotated, `resend.md` is gone, and
      `.gitleaks.toml` catches the pattern.
- [ ] §9 and §10 complete and verified by an actual install on a clean machine.
- [ ] `entitlements` and `billing_events` migrations applied to production D1,
      and `npm run migrate:local` run — per the warning in `wrangler.jsonc`,
      a re-keyed local database looks healthy and fails every insert.
- [ ] Every test in the §8 table exists and passes.
- [ ] The webhook has been exercised against the provider's own test-mode
      deliveries, including a deliberately reordered pair and a duplicate.
      Signature verification passing against a hand-rolled test fixture proves
      only that our serializer agrees with itself.
- [ ] `TURNSTILE_ENABLED` is still `"false"`, with the §5.1 comment beside it.
- [ ] `spec/threat-model-v1.md` §3 has absorbed the §8 table.
- [ ] `SECURITY.md` states what the payment provider holds and what kosong
      does not, in the same plain terms as its existing boundary section.
- [ ] The account-deletion test asserts the subscription is cancelled, not
      merely that the local rows are gone.
