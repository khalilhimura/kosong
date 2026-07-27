# kosong CLI Contract v1

**Status:** Normative for `kosong` v1
**Last updated:** 2026-07-27

What scripts and other tools may rely on. Anything here changes only with a
version bump; anything not here may change without notice.

---

## 1. Exit codes

Contractual. Do not renumber.

| Code | Meaning |
|---:|---|
| `0` | Success |
| `2` | Invalid input, or an unmet local precondition |
| `3` | Authentication or session problem |
| `4` | An external tool or provider prerequisite |
| `5` | Network or remote service |
| `10` | An internal error that should not happen |

Notably `2`, not `4`, is returned when kosong refuses input *before* reaching a
provider — the distinction tells a script whether the tool was ever involved.

## 2. Streams

| Stream | Carries |
|---|---|
| stdout | Requested output: JSON, document text, status |
| stderr | Errors, warnings, and repair guidance |

`--json` output goes to stdout alone and is always valid JSON, **including when
the command exits non-zero**. A script must be able to read the reason, not just
observe a failure.

## 3. Global flags

| Flag | Effect |
|---|---|
| `--quiet` | Suppresses lessons and success prose. Never suppresses errors |
| `--workspace PATH` | Works in `PATH` instead of the current folder |
| `--help`, `--version` | Standard |

`--help` lists only commands that are implemented. A command that is not built
is absent, not present and broken.

## 4. Environment

| Variable | Effect |
|---|---|
| `KOSONG_CONFIG_DIR` | Overrides the settings directory. Also selects an isolated profile, so the session is kept beside it rather than in the shared keychain |
| `KOSONG_SESSION_FILE` | Forces the credential into this file. For tests and CI, which must never touch a real keychain |
| `KOSONG_API_URL` | Overrides the API base URL |
| `NO_COLOR` | Any value disables colour |
| `EDITOR`, `VISUAL` | Which editor `kosong edit` opens |

## 5. `status --json`

Schema `1`. Fields may be **added**; renaming or removing one requires a schema
bump. §17 names this the first step toward a read-only multi-harness interface.

```json
{
  "schema": 1,
  "okf_version": "0.1",
  "workspace": { "found": true, "path": "/absolute/path" },
  "document": {
    "exists": true, "valid": true, "managed": true,
    "path": "/absolute/path/kosong.md",
    "type": "Page", "title": "My First Site", "slug": "my-first-site",
    "visibility": "private", "id": "01J…ULID",
    "timestamp": "2026-07-26T10:30:00+08:00"
  },
  "onboarding": {
    "local_document_created": true, "preview_completed": false,
    "login_completed": false, "site_initialized": false,
    "site_published": false, "next_command": "kosong preview"
  },
  "session": { "signed_in": false }
}
```

Optional fields are omitted rather than null. `document.error` appears only when
`valid` is false. `session.stored_in` appears only when signed in.

**`status` makes no network call.** It must stay under a second and work
offline, so `signed_in` reports only whether a credential exists locally.

**Nothing here is ever a secret** — no token, no code, no email address.

## 6. `doctor --json`

Schema `1`.

```json
{
  "schema": 1,
  "okf_version": "0.1",
  "healthy": true,
  "checks": [
    { "name": "settings folder", "status": "ok", "detail": "/path" },
    { "name": "editor", "status": "warn", "detail": "not set", "repair": "export EDITOR=nano" }
  ]
}
```

`status` is `ok`, `warn`, or `fail`. `healthy` is true when nothing has failed.

Two guarantees:

- **Every non-`ok` check carries a `repair`.** A diagnostic that reports a
  problem without a next step is the experience kosong exists to replace.
- **A missing optional tool is `warn`, never `fail`.** Local work must succeed
  on a machine with no `git`, `gh`, `npm`, or `wrangler`.

## 7. Mutating operations

Anything reaching outside the computer must, per §12.4:

1. Print the exact executable and arguments, the working directory, the files
   involved, and the remote effect.
2. Stop on `--dry-run`, changing nothing.
3. Ask for confirmation unless `--yes`.

Purely local steps print a single line naming the exact command. Under
`--dry-run` everything is shown in full, because that is when the user asked to
see the plan.

`--dry-run` **never invokes a mutating tool.** Read-only checks may still run;
they are how the plan is built.

## 8. Limits

| Limit | Value |
|---|---|
| Document size | **1 MiB** — 1,048,576 bytes |

Contractual, because two independent implementations share it and a third
argument depends on it.

The CLI refuses an oversized document *before* the network, with
`DOCUMENT_TOO_LARGE` and exit `2` — a local precondition, per §1. The service
enforces the same number independently and answers `413 DOCUMENT_TOO_LARGE`, on
the principle that a client-side check is a courtesy and never a control.

Stating the number here matters more than it looks. The limit is enforced in
Rust and in TypeScript, and `spec/threat-model-v1.md` reasons about what it does
*not* protect against — a YAML alias bomb expands after the bytes are counted,
so the cap alone does not help. Three places depending on a constant that none
of them defines is how the two enforcement points quietly drift apart.

Size is measured in bytes of the document as stored, not characters.

## 9. Stability

**Stable:** exit codes; `--json` shapes at their schema version; the flags
above; the environment variables above; stream discipline.

**Not stable:** human-readable prose, colour, ordering of `doctor` checks, and
the exact wording of any message.

Parse the JSON. Do not parse the prose.
