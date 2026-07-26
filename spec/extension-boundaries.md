# Extension Boundaries v1

**Status:** Normative for `kosong` v1
**Last updated:** 2026-07-26

§17 defines interfaces that exist so v1 does not paint itself into a corner —
not so v1 can grow into a platform. This records what may be extended, what may
not, and what has to be true first.

---

## 1. Three independent version axes

None may be inferred from another.

| Axis | Current | Governs |
|---|---|---|
| OKF specification | `0.1` | The document format. Set upstream by Google Cloud |
| `kosong.profile` | `1` | The extension block inside the document |
| API schema | `1` | `status --json`, `doctor --json`, HTTP contracts |

A user on OKF v0.1 with profile 1 talking to API schema 1 is the v1 case. Any of
the three can move without the others.

## 2. The format is not ours to fork

`kosong` **profiles** the Open Knowledge Format; it does not extend it.

- Product-specific state goes in the `kosong:` block, never as a new top-level
  field. A top-level field would collide with a future OKF core field.
- Unknown keys round-trip unchanged, including keys from OKF v0.2 that v1 does
  not interpret.
- If kosong needs something the format lacks, the move is a proposal upstream,
  not a private extension.

The reason is the product promise. A document that only kosong can read is a
document the user cannot leave with.

## 3. Provider access stays a closed set

Allowlists are **closed Rust enums**, not filtered strings. Adding an operation
means adding a variant with a fixed argument vector, reviewed on its own.

Explicitly refused for v1:

- A generic pass-through such as `kosong gh <anything>`. §6.2 puts this out of
  scope, and the ownership promise means a user can run `gh` themselves.
- `gh api` with a caller-supplied endpoint. Named as permissible for "narrowly
  implemented endpoints only"; v1 needs none, so none exists.
- Any operation that accepts a provider credential. Architectural rule 2 in §2
  forbids kosong from holding one, and no type here has a field for it.

## 4. What v1 will not do, and why

### Cloudflare Pages rollback

Verified against Wrangler 4.114 and current Cloudflare documentation: **there is
no Wrangler command for it.** `pages deployment` offers `list`, `create`,
`tail`, `delete`; the top-level `wrangler rollback` is for Workers. Cloudflare
documents Pages rollback as a dashboard action.

The REST API can do it, but that needs an API token in kosong's hands, which
rule 2 forbids. So `site rollback` lists the real deployment history and points
at the dashboard. Deleting the newer deployment and calling that a rollback
would be worse than being honest.

**Revisit when:** Wrangler ships a Pages rollback command.

### An MCP server

Not until authorization scopes, read/write separation, audit events,
cross-harness integration tests, and a separate architecture decision all exist.

The first step, if earned, is **read-only**: `status --json`, `show --json`, and
a documented file layout. `status --json` is already stable for this reason.

**Revisit when:** there is demonstrated demand, not anticipated demand.

### Multiple documents

v1 stores exactly one document per user. The R2 key is `users/{id}/document-v1`
and the `documents` table is keyed by user.

Supporting more is not a schema change alone — it changes the product from "one
page you understand" to "a file manager", which is a different thing.

**Revisit when:** users who completed the v1 loop ask for it.

### End-to-end encryption

Needs device keys, encrypted recovery, key rotation, multi-device pairing, an
answer for lost keys, and independent review. Shipping something that looks
encrypted but is not would be worse than the current honest boundary.

**Revisit when:** all of the above is designed, not before.

## 5. Interfaces defined but minimally implemented

`kosong-core` keeps the seams that make replacement possible:

| Seam | Why it exists |
|---|---|
| `SessionStore` | Keychain, file, and in-memory implementations already differ per platform |
| `EmailSender` (API) | §4 requires a replaceable provider boundary; Resend is one implementation |
| `Operation` | Every provider operation describes itself the same way, so disclosure and dry run work identically for all of them |

The Cloudflare implementation must be replaceable **without changing user
command semantics**. `kosong sync` means the same thing regardless of what is
behind it.

## 6. Adding an operation

1. Add a variant to the provider enum with a fixed argument vector.
2. Implement `summary`, `mutating`, and — if it reaches outside the machine —
   `remote_effect`. A mutating operation with no stated effect fails a test.
3. Add a fake-binary test asserting the exact argv.
4. If it is mutating, verify it is disclosed and refuses under `--dry-run`.
5. Update `spec/cli-v1.md` if it changes the contract.

Step 2 is the one that matters. If an operation cannot be explained to a
beginner in one sentence, it probably should not be in kosong.
