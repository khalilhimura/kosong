# Telemetry v1

**Status:** Normative for `kosong` v1
**Last updated:** 2026-07-27

PRD §12 requires that telemetry be "opt-in or minimized and documented before
public beta". This is that document. It states what `kosong` collects, what the
service necessarily records to operate, and what would have to be true before
either changed.

---

## 1. The rule

**`kosong` collects no product analytics.**

No usage events. No feature counters. No crash reporting. No install ping. No
version check. Nothing is sent because a command ran.

This is not a promise about the future being kept vague. It describes the code
as it stands, and §5 states what adding anything would require.

## 2. The CLI

The CLI makes a network request only for `login`, `logout`, `sync`, and
`delete-account` — each one started by the user — plus the session refresh those
may need to complete. Every request goes to the `kosong` service and nowhere
else. There is no analytics endpoint, no update check, and no background
process.

Requests carry `User-Agent: kosong/<version>`, so the service sees a version
string alongside a request the user asked for. That is the whole of it.

Everything local — `new`, `edit`, `show`, `preview`, `status`, `doctor`,
`site init`, `site publish` — works with no account and no network. `status` in
particular makes no network call at all, by contract (`spec/cli-v1.md` §5).

Local state (`config.toml`, `onboarding.toml`) stays on the machine. The
technical specification §5.4 lists the fields it may contain and forbids secrets
and document content among them. It is never transmitted.

## 3. The website

No analytics script, no tag manager, no third-party embeds. Fonts are served
from the same origin rather than a font CDN, so loading a page does not tell
anyone else that you did.

The site is static and ships zero `<script>` tags.

## 4. What the service records

Running an authenticated service means recording some things. None of it is
product analytics, and all of it is bounded.

### Structured logs

Built from a fixed field set — event, outcome, route *pattern*, method, status,
duration, error code, user id, hashed IP, email domain, detail — and every value
passes a redaction filter. A call site cannot add a field by accident.

Never logged: verification codes, bearer or refresh tokens, document bytes, raw
email addresses, authorization headers, provider credentials. Technical
specification §11 is the list; the filter is structural rather than a rule
people are asked to remember.

### Security events

A `security_events` row per auditable event. Each carries an event type, an
opaque user id, a request id, a hashed IP, and a timestamp. The full set:

| Group | Events |
|---|---|
| Codes | `code_requested`, `code_request_rate_limited`, `code_verified`, `code_verify_failed`, `code_verify_exhausted` |
| Sessions | `session_refreshed`, `refresh_reuse_detected`, `session_revoked` |
| Documents | `document_read`, `document_written`, `document_conflict`, `unauthorized_document_access` |
| Accounts | `account_deleted` |

The document events record **that** a document was read or written — never a
byte of its content. `document_written` additionally records the size in bytes;
`document_read` carries nothing beyond the common fields.

These exist to answer "is this account under attack", which is not answerable
without them. The list is exhaustive as of this document; the type is a closed
union in the code, so adding one is a visible change.

### How identifiers are handled

| Value | Stored as |
|---|---|
| IP address | HMAC with a server-held secret. Comparable across requests, not reversible |
| Email address | Domain part only in logs — `example.com`, never the mailbox |
| User | An opaque id, never the email |

Cloudflare, as operator of the edge, sees request metadata independently of any
of this. That is the provider boundary `SECURITY.md` already describes, and no
logging choice here changes it.

### Two things this does not yet do

Stated because a policy that omits its own gaps is worth less than one that
names them:

1. **`security_events` rows survive account deletion.** Deleting an account
   removes the sessions, the document, the user row, and any outstanding
   verification codes. The audit rows remain, holding an opaque user id whose
   mapping to an email is gone with the user row — so they are de-identified
   rather than deleted. That is a deliberate trade for an audit trail that
   cannot be erased by the account under investigation, and anyone who expects
   deletion to mean *every* row should know it.
2. **No retention window is implemented.** Security events accumulate. A
   documented expiry belongs here before public beta.

## 5. If this ever changes

Any collection beyond the above must, all of it:

- be **opt-in and off by default** — never opt-out, never on for a release and
  off in the next;
- be documented **here first**, before the code that sends anything;
- exclude document content, titles, filenames, and email addresses without
  exception;
- be inspectable by a single command that prints exactly what would be sent,
  because "we collect anonymous usage data" is not a statement anyone can check.

The product's argument is that a beginner should be able to see what their tools
do. A tool that quietly measured them while making that argument would be making
a different one.
