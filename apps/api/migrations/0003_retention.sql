-- Indexes for the retention sweep.
--
-- Both tables are already indexed, but on leading columns the sweep does not
-- have: security_events on (user_id, created_at) and (event_type, created_at),
-- verification_codes on (email_normalized, expires_at, consumed_at). A
-- retention delete filters on created_at alone, which none of those serve, so
-- without these it scans the whole table — and it scans it on the one day the
-- table is largest.

CREATE INDEX security_events_created_idx ON security_events(created_at);
CREATE INDEX verification_codes_created_idx ON verification_codes(created_at);
