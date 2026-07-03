-- Reset persisted online flag on all contacts.
-- The flag is session-only and should never be true at startup.
UPDATE users SET online = 0;

CREATE INDEX IF NOT EXISTS idx_dht_cache_quality
    ON dht_cache (peer_id, fail_count ASC, cached_at DESC);
