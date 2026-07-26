# kosong Threat Model v1

**Status:** Release gate for Phase B, per PRD §10
**Last updated:** 2026-07-26

§10 makes threat-model review a gate "before meaningful user documents are
invited into remote storage". This is that review: what is being defended, from
whom, by what mechanism, and which test proves it.

---

## 1. What is being protected

| Asset | Where it lives | Worst case if lost |
|---|---|---|
| The user's document | `kosong.md`, and R2 if synced | Private writing becomes readable by someone else |
| Sign-in credential | OS keychain, or an owner-only file | Someone else acts as the user |
| Verification codes | D1, keyed hash only | An account is taken over |
| Provider credentials | `gh` and `wrangler` stores — **never kosong** | A repository or website is taken over |
| The published site | Cloudflare Pages | The user's public page says something they did not write |

## 2. Who is being defended against

**In scope**

- A network attacker between the user and the service.
- Another user of the service attempting to reach data that is not theirs.
- An attacker guessing or replaying sign-in codes.
- A hostile *document* — content crafted to execute when previewed or published.
- A hostile *name* — input crafted to escape into a command or a path.
- An attacker who obtains a stolen refresh token.

**Out of scope, and stated plainly**

- A compromised user device. Nothing survives that.
- A compromised email inbox. Email is the root of identity here, by design.
- A malicious or compelled service operator. v1 is not end-to-end encrypted.
- A compromised Cloudflare, GitHub, or npm.

## 3. Controls, and the test for each

### 3.1 Authentication

| Threat | Control | Test |
|---|---|---|
| Code guessing | Six digits from a CSPRNG **without modulo bias**; 10-minute expiry; 5 attempts then the code dies; per-email cooldown and per-email/per-IP limits | `auth.test.ts` → *stops accepting guesses after the attempt limit*; `contract.test.ts` → *cover the whole range* |
| Code disclosure at rest | Stored as HMAC-SHA256 under a server-held secret | *stores the code only as a hash* |
| Code replay | Consumption is `UPDATE … WHERE consumed_at IS NULL` with a changed-row check, so two concurrent verifications cannot both win | *refuses a code that has already been used* |
| Account enumeration | The users table is **not consulted** when a code is requested — accounts are created on verification. There is no branch to time | *responds identically whether or not the account exists* |
| Token theft | Access tokens are short-lived and self-contained; refresh tokens are opaque and hashed at rest | *stores refresh tokens only as hashes* |
| Token replay | Refresh rotates on every use; presenting a rotated token revokes the entire family | *revokes the whole family when a rotated token is presented again* |
| Forged tokens | HS256 verification is fixed by construction and never reads the header's `alg` | *cannot be bypassed with an unsigned token* |

**Residual risk.** Whoever controls the email inbox controls the account. This
is inherent to passwordless email sign-in and is stated in `SECURITY.md`.

### 3.2 Authorization

| Threat | Control | Test |
|---|---|---|
| Reading another user's document | The user id comes only from a verified access token, in one module. The R2 key is derived server-side from that id | `documents.test.ts` → *does not let one user read another's document* |
| Writing to another user's document | Same derivation. No request carries a user id or an object key | *does not let one user overwrite another's document* |
| Tampering | Signature verification precedes any use of the payload | *rejects a tampered access token* |

### 3.3 Data integrity

| Threat | Control | Test |
|---|---|---|
| Silent overwrite | `If-Match` is **required**; absent is a 400, not a default-to-overwrite | *requires an If-Match header* |
| Losing a concurrent edit | A mismatch returns 409 with remote metadata and writes nothing | *rejects a write based on a stale version and keeps both intact* |
| Losing local work | Conflicts write both versions into the workspace; the working file is untouched | `sync.rs` tests; verified end to end |
| Losing work to a crash | Every write is temp file → `fsync` → rename → `fsync` directory | `workspace_safety.rs` |
| Corrupting a document | Body preserved byte for byte; unknown front-matter keys preserved | `okf_conformance.rs` |

### 3.4 Execution safety

| Threat | Control | Test |
|---|---|---|
| Command injection | No shell anywhere. Executable plus separate arguments. There is no `sh -c` in the codebase | `process_safety.rs` → *shell metacharacters are inert arguments*, proven against a fake binary that echoes its argv |
| Editor injection | An `EDITOR` containing shell syntax is refused with an explanation, rather than passed through as baffling literal arguments | `editor.rs` → *shell syntax is refused with an explanation* |
| Unbounded subcommands | Provider allowlists are **closed enums**, not string filters. There is no variant meaning "run this string" | *only allowlisted provider subcommands are accepted* |
| Path traversal | Workspace containment plus name validation | *traversal outside the workspace is refused* |
| A hung provider | Bounded timeout, child killed on drop | *a hung process is stopped at the timeout* |
| Committing a user's files | git is called with explicit paths only, never `.` or `-A`; publish refuses on unrelated changes | *init stages only the files kosong wrote* |

### 3.5 Content safety

| Threat | Control | Test |
|---|---|---|
| Script in a previewed page | Raw HTML dropped; URL scheme allowlist | `render_policy.rs` |
| Script in a **published** page | The same Rust renderer produces the published HTML. The template contains no Markdown parser | `site_workflow.rs` → *the published content gets the same safety policy as preview*, plus a CI guard |
| Preview reachable off-machine | Binds `Ipv4Addr::LOCALHOST`, with no option to widen | `preview_server.rs` → *the listener binds only to loopback* |
| YAML expansion attack | Alias count capped; the size limit alone does not help, because expansion happens after bytes are counted | `documents.test.ts` → *survives a yaml alias bomb* |

> **This row is here because it was a real bug.** The first template parsed
> Markdown in JavaScript, so `<script>` and `javascript:` URLs survived into the
> published page even though preview stripped them. A page could look safe and
> publish something live. Two implementations of one policy will always drift,
> so there is now exactly one.

### 3.6 Disclosure

| Threat | Control | Test |
|---|---|---|
| Secrets in logs | Structured logging with a fixed field set; every value passes through redaction | `contract.test.ts` → *strips anything shaped like a credential* |
| Secrets in provider output | Child output is redacted before it is stored, displayed, or logged | `process_safety.rs` → *process output is redacted before a caller sees it* |
| Secrets in local state | `onboarding.toml`, `sync.toml`, and `site.toml` have no field that can hold one, and unknown keys are dropped on write | *a secret written by hand is dropped rather than preserved* |
| Email in the audit log | Only the domain is recorded, plus a keyed IP hash | *never writes a raw address into the security event log* |

## 4. Accepted risks

1. **Not end-to-end encrypted.** The operator can read a synced document.
   Mitigated by saying so in the README, `SECURITY.md`, and this document, and
   by making every local feature work without syncing.
2. **Email is the root of identity.** Inherent to passwordless sign-in.
3. **Redaction is targeted, not exhaustive.** Deliberate: blanking every long
   random string would destroy `git` and `gh` output — a commit SHA is not a
   secret — while protecting nothing that the prefix and label rules miss.
4. **A `--yes` flag exists.** Required for automation by §9.3. Every such
   operation still prints its full disclosure first.
5. **npm dependencies are trusted at their pinned versions.** Mitigated by exact
   pinning, `--ignore-scripts` on install, and `cargo deny` on the Rust side.

## 5. Gate status

| Phase B requirement | State |
|---|---|
| Authentication controls tested | Met |
| Cross-user authorization tested | Met |
| Conflict preservation tested | Met |
| Deletion tested and idempotent | Met |
| Privacy and redaction tested | Met |
| Threat-model review complete | This document |

**One item is outstanding before real user documents are accepted:** an
independent review. Everything above was designed and tested by the same author,
which catches mistakes but not blind spots.
