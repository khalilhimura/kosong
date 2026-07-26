-- Document metadata. The bytes live in R2; this table holds only what is
-- needed to authorize access and detect concurrent writes.
--
-- One row per user: v1 stores exactly one document per account.

CREATE TABLE documents (
  user_id TEXT PRIMARY KEY REFERENCES users(id),
  -- Derived server-side as users/{user_id}/document-v1. The client never
  -- chooses or sees this, which is what keeps one user's key space from
  -- addressing another's.
  object_key TEXT NOT NULL UNIQUE,
  -- Opaque optimistic-concurrency token. Compared for equality only; never
  -- parsed, and never a basis for authorization.
  etag TEXT NOT NULL,
  byte_size INTEGER NOT NULL,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  deleted_at TEXT
);
