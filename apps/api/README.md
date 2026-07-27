# kosong API

The Cloudflare Worker behind `kosong login`, `kosong sync`, and
`kosong delete-account`. Nine routes, a hand-written router, D1 for metadata
and R2 for document bytes.

Everything in the CLI that does not need an account works without this service
running at all. That is the point, and it is worth remembering when something
here is broken.

## Running it locally

```bash
npm install
cp .dev.vars.example .dev.vars     # then fill in the three secrets
npm run migrate:local              # required before the first run
npm run dev
```

Point the CLI at it:

```bash
KOSONG_API_URL=http://127.0.0.1:8787 kosong login --email you@example.com
```

Leave `RESEND_API_KEY` unset locally. The Worker then writes the verification
code to its own log instead of sending mail, and says loudly that it did.
**That fallback is disabled when `ENVIRONMENT` is `production`** — a deployed
service with no email provider refuses to send rather than publish sign-in
codes to anyone who can read a log.

## Tests

```bash
npm test          # 92 tests, no network, no real email
npm run typecheck # wrangler types, then tsc
```

Both run in CI. `tsc` catches things the test run does not — unchecked indexed
access, over-narrow casts — so a green `npm test` alone is not enough.

## Two traps worth knowing

**Changing `database_id` re-keys the local database.** miniflare stores local
D1 state per database id, so editing that line in `wrangler.jsonc` silently
points `wrangler dev` at a fresh, unmigrated database. It will look healthy —
`/ready` only runs `SELECT 1`, which an empty database answers perfectly well —
while every insert fails and every route returns 500. Run
`npm run migrate:local` again.

**`vitest` inherits `vars` from `wrangler.jsonc`,** including
`ENVIRONMENT: "production"`. A test that does not inject an email sender gets
whichever one the configuration implies, not the friendly local default. Inject
a `RecordingEmailSender` when the test needs a send to succeed.

## Deploying

```bash
npm run migrate:remote
npm run deploy
```

Secrets are never in a file. Set each with `wrangler secret put`:
`CODE_HMAC_SECRET`, `TOKEN_HMAC_SECRET`, `IP_HASH_SECRET`, `RESEND_API_KEY`,
and `TURNSTILE_SECRET` only if `TURNSTILE_ENABLED` is `"true"`.
