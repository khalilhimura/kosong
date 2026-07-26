-- Authentication: users, one-time verification codes, refresh sessions,
-- and the security event log.
--
-- Storage rules, from technical specification §10:
--   * Email is stored only where authentication needs it.
--   * Codes, refresh tokens, and IP context are stored as hashes, never raw.
--   * Document content never appears in D1.
--
-- SQLite has no native boolean or timestamp type. Timestamps are TEXT in
-- RFC 3339 form, which sorts correctly as a string.

CREATE TABLE users (
  id TEXT PRIMARY KEY,
  email_normalized TEXT NOT NULL UNIQUE,
  created_at TEXT NOT NULL,
  -- Set during account deletion. The row is removed outright, so this exists
  -- for the window in which deletion is in progress.
  deleted_at TEXT
);

CREATE TABLE verification_codes (
  id TEXT PRIMARY KEY,
  email_normalized TEXT NOT NULL,
  -- HMAC-SHA256 of the code under a server-held secret. Never the code.
  code_hash TEXT NOT NULL,
  expires_at TEXT NOT NULL,
  consumed_at TEXT,
  attempt_count INTEGER NOT NULL DEFAULT 0,
  -- HMAC of the request IP. Enough to rate-limit, not enough to identify.
  request_ip_hash TEXT,
  created_at TEXT NOT NULL
);

-- Serves the "find the active code for this email" lookup on every verify.
CREATE INDEX verification_codes_active_idx
  ON verification_codes(email_normalized, expires_at, consumed_at);

-- Serves per-IP rate limiting, which counts recent rows by ip hash.
CREATE INDEX verification_codes_ip_idx
  ON verification_codes(request_ip_hash, created_at);

CREATE TABLE refresh_sessions (
  id TEXT PRIMARY KEY,
  user_id TEXT NOT NULL REFERENCES users(id),
  -- SHA-256 of the opaque token. A database leak does not yield usable tokens.
  token_hash TEXT NOT NULL UNIQUE,
  -- Groups every token descended from one login, so presenting a rotated-away
  -- token can revoke the whole lineage.
  family_id TEXT NOT NULL,
  device_name TEXT,
  expires_at TEXT NOT NULL,
  revoked_at TEXT,
  created_at TEXT NOT NULL,
  last_used_at TEXT
);

CREATE INDEX refresh_sessions_user_idx ON refresh_sessions(user_id, revoked_at);
CREATE INDEX refresh_sessions_family_idx ON refresh_sessions(family_id);

CREATE TABLE security_events (
  id TEXT PRIMARY KEY,
  user_id TEXT,
  event_type TEXT NOT NULL,
  request_id TEXT NOT NULL,
  ip_hash TEXT,
  created_at TEXT NOT NULL,
  -- Schema-controlled and redacted at the writing layer. Must never hold a
  -- code, token, email address, or document bytes.
  metadata_json TEXT
);

CREATE INDEX security_events_user_idx ON security_events(user_id, created_at);
CREATE INDEX security_events_type_idx ON security_events(event_type, created_at);
