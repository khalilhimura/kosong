# Security

## What v1 protects, and what it does not

`kosong` v1 protects your document in transit and at rest, and enforces that
only you can reach your own data.

**It is not end-to-end encrypted.** This is stated first because the alternative
— burying it — is how people end up trusting a system with something it was
never built to hold.

### The boundary

| Protected against | Not protected against |
|---|---|
| Someone reading your traffic | The service operator reading a synced document |
| Someone reading the storage bucket | Anyone with access to your unlocked computer |
| Another user reaching your document | Anyone who controls your email inbox |
| Guessing your sign-in code | A compromised provider |

If a document must remain private from the service operator, **do not sync it**.
Everything local — create, edit, preview, publish — works with no account.

### Why not end-to-end encryption

Doing it properly needs device keys, encrypted recovery, key rotation,
multi-device pairing, an answer for lost keys, and independent cryptographic
review. Shipping something that merely *looks* encrypted would be worse than
being honest, because people would trust it with more than it can carry.

## How your credentials are handled

**Your provider credentials never reach kosong.** GitHub and Cloudflare
credentials stay in `gh` and `wrangler`. kosong only ever asks those tools
whether you are signed in. No code path accepts a provider token — the operation
types have nowhere to put one.

**Your kosong sign-in** is a refresh token in your operating-system keychain.
Access tokens are short-lived and never written to disk. If no keychain is
available, the token goes in a file readable only by you, and kosong says so
prominently rather than quietly downgrading.

**No passwords exist.** Sign-in is a six-digit code sent by email, valid once,
expiring in ten minutes, with a limited number of attempts.

## Controls

| Threat | Control |
|---|---|
| Guessing a sign-in code | CSPRNG code without modulo bias, keyed hash at rest, 10-minute expiry, 5 attempts, per-email and per-IP rate limits |
| Discovering whether an account exists | Identical response and identical work either way; the users table is not consulted when a code is requested |
| A stolen refresh token | Short access lifetime, hashed rotating refresh tokens, whole-family revocation on reuse |
| Reading another user's document | User identity derived only from a verified token; the storage key is built server-side and never sent by the client |
| Losing work to a concurrent edit | ETag concurrency; a mismatch preserves both versions and asks |
| Command injection through kosong | No shell, anywhere. Executable plus separate arguments only |
| A credential appearing in output | Provider output is redacted before it is displayed, logged, or stored |
| A preview being reachable off-machine | The preview server binds loopback only, with no option to widen it |
| Executable content in your published page | Raw HTML is dropped and dangerous URL schemes are blocked, by the same renderer that backs local preview |

Each row has a test. See `crates/kosong-core/tests/process_safety.rs`,
`apps/api/test/auth.test.ts`, and `spec/threat-model-v1.md`.

## What is never logged

Verification codes, access or refresh tokens, document contents, raw email
addresses, `Authorization` headers, and provider credentials.

Logs record an email's **domain** and a **keyed hash** of the client IP — enough
to spot an attack pattern, not enough to identify a person.

## Reporting a vulnerability

Email **security@mesolitica.com**. Please include what you did, what happened,
and what you expected. Expect an acknowledgement within three working days.

Please do not open a public issue for anything exploitable.

We will not take legal action against good-faith research that stays within your
own account and does not degrade the service for others.

## Deleting your account

```bash
kosong delete-account --dry-run   # see exactly what goes
kosong delete-account
```

This removes your sign-in, every session on every computer, and the private copy
of your page on the server. It happens immediately and cannot be undone.

It deletes nothing on your computer, and **it does not take a published site
offline** — that lives in your own GitHub repository and Cloudflare account,
which kosong has no credentials for. Remove it there if you want it gone.

## Known limitations in v1

1. **No end-to-end encryption.** As above.
2. **One session per machine.** Signing in on a second machine is a separate
   session; there is no device list yet.
3. **Account deletion is immediate and irreversible.** There is no grace period.
4. **Redaction is targeted, not exhaustive.** Known credential shapes and
   labelled values are removed. Long random-looking strings are deliberately
   left alone — a 40-character hex run is far more often a commit SHA than a
   secret, and blanking those would break tool output while protecting nothing.
5. **Front-matter comments are lost on write.** kosong preserves every key,
   value, and their order, but re-serializes the YAML. Your Markdown body is
   preserved byte for byte.
