# kosong OKF Profile v1

**Status:** Normative for `kosong` v1
**Profiles:** [Open Knowledge Format v0.1](https://github.com/GoogleCloudPlatform/knowledge-catalog/blob/main/okf/SPEC.md)
**Last updated:** 2026-07-27

---

## 1. What this document is

`kosong` does not define a file format. It **profiles** Google Cloud's Open Knowledge Format — a vendor-neutral specification for representing knowledge as a directory of Markdown files with YAML front matter, published under Apache-2.0.

A *profile* narrows a general specification for a specific use, without breaking conformance to it. This document states exactly what `kosong` writes, what it requires, and what it must tolerate.

### Upstream references

| | |
|---|---|
| Specification | <https://github.com/GoogleCloudPlatform/knowledge-catalog/blob/main/okf/SPEC.md> |
| Repository | <https://github.com/GoogleCloudPlatform/knowledge-catalog/tree/main/okf> |
| Announcement | [How the Open Knowledge Format can improve data sharing](https://cloud.google.com/blog/products/data-analytics/how-the-open-knowledge-format-can-improve-data-sharing), Sam McVeety and Amir Hormati, Google Cloud, 2026-06-13 |
| v0.1 spec text | Commit [`ee67a5ca`](https://github.com/GoogleCloudPlatform/knowledge-catalog/blob/ee67a5ca/okf/SPEC.md) |
| License | Apache-2.0 |

> **Version note.** `kosong` v1 targets **OKF v0.1**. Upstream published **v0.2** on 2026-07-24. See §6.

---

## 2. Upstream rules `kosong` must obey

These are not `kosong` decisions. They come from OKF v0.1 and are binding.

### 2.1 Required

A bundle conforms to OKF v0.1 if:

1. Every non-reserved `.md` file contains a parseable YAML front-matter block.
2. Every front-matter block contains a **non-empty `type` field**.
3. Reserved filenames follow the specified structure when present.

`type` is the *only* required field. Everything else is recommended or optional.

### 2.2 Consumer obligations

A conformant consumer **must not** reject a bundle for:

- missing optional front-matter fields;
- unknown `type` values;
- unknown additional front-matter keys;
- broken cross-links;
- a missing `index.md`.

Unknown fields **must be preserved**. This is the rule that makes the format portable, and it is the one a naive `serde` implementation breaks. See §5.

### 2.3 Reserved filenames

| Filename | Meaning |
|---|---|
| `index.md` | Directory listing for progressive disclosure |
| `log.md` | Chronological update history |

These are never concept documents. `kosong` must refuse to create or manage a document at either path.

### 2.4 Cross-links

Two forms, both untyped directed edges:

- **Bundle-relative** — begins with `/`, resolved from bundle root. Recommended; survives file moves.
- **Relative** — standard Markdown relative paths.

Broken links are legal and may represent not-yet-written knowledge.

---

## 3. The `kosong` profile

### 3.1 Bundle shape

`kosong` v1 manages a **single-concept bundle**. The workspace root is the bundle root.

```text
workspace/
├── kosong.md          # the one concept document
├── index.md           # optional, generated; never required
└── .kosong/           # non-OKF local state, not part of the bundle
```

`.kosong/` holds CLI state. It contains no `.md` concept documents and is not part of the bundle surface.

### 3.2 Document `kosong.md`

```markdown
---
type: Page
title: "My First Site"
description: "A page published with kosong."
tags: [kosong]
timestamp: "2026-07-26T10:30:00+08:00"
kosong:
  profile: 1
  id: "01J7ZQ8F3K9XG2VW6M4T1B5NRY"
  slug: "my-first-site"
  visibility: "private"
  created_at: "2026-07-26T10:30:00+08:00"
---

# My First Site

Start here.
```

### 3.3 OKF core fields as used by `kosong`

| Field | Written by `kosong` | Required to read | Notes |
|---|---|---|---|
| `type` | `Page` | **yes** | Any other value is read without error or warning. |
| `title` | yes | no | Falls back to filename stem, per OKF. |
| `description` | yes | no | Single sentence. |
| `resource` | on publish | no | Set to the deployment URL. |
| `tags` | `[kosong]` | no | User entries preserved; `kosong` appends only on creation. |
| `timestamp` | yes | no | RFC 3339. Last meaningful change. |

`timestamp` carries the meaning of what earlier `kosong` drafts called `updated_at`. There is no separate `updated_at` field.

### 3.4 The `kosong` extension block

OKF permits arbitrary producer keys. All `kosong` state lives under **one** top-level key, `kosong`, so that:

- it cannot collide with a future OKF core field;
- another producer's tooling can drop or ignore it as a single unit;
- the document stays readable to any OKF consumer that has never heard of `kosong`.

| Field | Type | Required | Rules |
|---|---|---:|---|
| `profile` | integer | yes | Must equal `1`. Versions this block, **not** OKF. |
| `id` | ULID | yes | Generated locally at creation. Immutable in v1. |
| `slug` | string | yes | Lowercase, URL-safe. Derived from `title` unless given. |
| `visibility` | enum | yes | `private` \| `public`. Affects template and deployment only — **never** remote object authorization. |
| `created_at` | RFC 3339 | yes | Immutable after creation. |

Unknown keys nested inside `kosong` round-trip unchanged, exactly like unknown top-level keys.

### 3.5 Managed and unmanaged documents

| State | Condition | Behaviour |
|---|---|---|
| **Managed** | Valid OKF **and** a valid `kosong` block | All commands available. |
| **Unmanaged** | Valid OKF, no `kosong` block | `show`, `preview`, `status` work. `status` offers adoption. |
| **Invalid** | Not conformant OKF | Clear validation error plus a repair suggestion. |

Adoption writes the `kosong` block and touches nothing else. The body is preserved byte-for-byte.

This matters: it is what lets a user point `kosong` at a document produced by some other OKF tool, and it is why "no `kosong` block" is not an error.

---

## 4. Validation

### 4.1 Errors — reject

| Condition | Reason |
|---|---|
| No front-matter block | Fails conformance criterion 1 |
| Front matter is not parseable YAML | Fails criterion 1 |
| Front matter is not a YAML mapping | `type` cannot exist |
| `type` absent, empty, or whitespace-only | Fails criterion 2 |
| `kosong` present but not a mapping | Malformed extension |
| `kosong.profile` != 1 | Unknown extension version |
| `kosong` present and missing a required subfield | Malformed extension |
| Document path is `index.md` or `log.md` | Reserved filename |
| Not valid UTF-8 | OKF is a text format |

### 4.2 Never an error — tolerate

| Condition | Behaviour |
|---|---|
| `type` is an unrecognised value | Accept as-is |
| `title`/`description`/`tags`/`timestamp` absent | Accept; derive `title` from filename |
| Unknown top-level keys | Preserve |
| Unknown keys inside `kosong` | Preserve |
| v0.2 fields present | Preserve, do not interpret |
| Broken cross-links | Accept |
| Missing `index.md` | Accept |
| Empty body | Accept |

### 4.3 Preservation guarantees

1. **Body is byte-for-byte identical** across any metadata-only operation, including line endings, trailing whitespace, and absence of a trailing newline.
2. **Front-matter keys and values survive** a read/write cycle, including key order.
3. Front-matter *formatting* — indentation, quoting style, comments — is **not** guaranteed, because the YAML is re-serialized. Only key order and semantic content are.

Point 3 is a deliberate, documented limitation. YAML comments in front matter are lost on write.

---

## 5. Implementation notes

The preservation requirement in §2.2 rules out the obvious approach of deserializing front matter into a fixed `struct`, which silently drops unknown keys on write.

`kosong-core` therefore keeps the **entire front matter as an order-preserving YAML mapping** and layers typed accessors over it. Writes mutate individual keys in place, leaving every other key untouched.

The body is captured as the exact byte range following the closing delimiter and is never re-parsed or re-emitted.

---

## 6. OKF v0.2 and the migration path

Upstream published v0.2 on 2026-07-24. `kosong` v1 targets v0.1 deliberately: at the time this profile was written v0.2 was two days old, and its new families are not needed by a single-page publishing tool.

**Revisit when** any of these is true, whichever comes first:

- **2026-10-24** — v0.2 plus ninety days. "Days old" is the whole argument above, and it stops being true on its own without anyone deciding anything. This date exists so the decision gets made rather than inherited.
- The upstream sample bundles in `GoogleCloudPlatform/knowledge-catalog` move to v0.2. §7 tests `kosong` against them, so this is the point where staying on v0.1 costs something concrete.
- A user brings a v0.2 document that `kosong` handles worse than it should.

Re-reading this section at that point is cheap: the migration below is already written, and §4.2's unknown-key rule means v1 documents keep working either way. What must not happen is v0.1 becoming the target by default, unexamined, because a sentence about novelty aged out of being true.

### Breaking changes

| v0.1 | v0.2 | Impact on `kosong` |
|---|---|---|
| `timestamp` | `generated: { by, at }` | v0.2 consumers may fall back to `timestamp`, so v1 documents stay readable. |
| Body `# Citations` list | Front-matter `sources` | None; `kosong` v1 does not emit citations. |

### Additions (v1 preserves, does not interpret)

- **Trust** — `verified`, and `generated`.
- **Lifecycle** — `status` (`draft`/`stable`/`deprecated`), `stale_after`.
- **Provenance** — `sources`.
- **Attested Computation** — a concept type with `runtime`, `parameters`, `executor`, `attester`.
- **Version declaration** — `okf_version: "0.2"`, permitted in a bundle-root `index.md` only.

### Requirements on the v1 implementation

1. All v0.2 fields listed above **must round-trip unchanged**. §4.2 already guarantees this via the general unknown-key rule; the fields are named here so tests exist for them specifically.
2. OKF version handling stays behind **one module boundary**, so adding v0.2 is additive rather than a rewrite.
3. `kosong` must not fork the format. Product-specific needs go in the `kosong` block, or upstream as a proposal.

---

## 7. Conformance tests

The implementation must prove:

- [ ] A document with **only** `type: Page` and a body parses and is valid.
- [ ] `type` missing, empty, or whitespace-only is rejected with a repair suggestion.
- [ ] Unknown top-level keys survive a read/write cycle.
- [ ] Unknown keys inside `kosong` survive a read/write cycle.
- [ ] Every v0.2 field in §6 survives a read/write cycle uninterpreted.
- [ ] Front-matter key order is preserved.
- [ ] Body is byte-identical after a metadata write — including CRLF, no trailing newline, and Unicode.
- [ ] `index.md` and `log.md` are refused as concept documents.
- [ ] An unmanaged document is readable, and adoption preserves the body byte-for-byte.
- [ ] Unrecognised `type` values are accepted.
- [ ] The upstream sample bundles in `GoogleCloudPlatform/knowledge-catalog` parse without error.
