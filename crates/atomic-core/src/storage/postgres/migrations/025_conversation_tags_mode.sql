-- Migration 025: a conversation's scope tags carry the role they play.
--
-- 'include' (any of them admits an atom), 'require' (all of them must be
-- present), 'exclude' (none may be). Existing rows default to 'include',
-- which is exactly the OR scope they already meant — boolean scope makes
-- today's behavior the degenerate case rather than a new mode.

ALTER TABLE conversation_tags ADD COLUMN IF NOT EXISTS mode TEXT NOT NULL DEFAULT 'include';

INSERT INTO schema_version (version) VALUES (25);
