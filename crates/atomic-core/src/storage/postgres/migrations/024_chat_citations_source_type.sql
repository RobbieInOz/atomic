-- Migration 024: chat citations carry their source type and their number.
--
-- `source_type` says how to read `atom_id`: 'atom' (an atom id, the default
-- every pre-existing row keeps), 'wiki' (the tag id whose article was
-- cited), 'finding' (the finding atom's id). One id column, one discriminator
-- — exactly one id is ever meaningful per row.
--
-- `citation_index` is the `[N]` marker the answer actually wrote, which
-- SQLite has stored since v1 but Postgres never had: the read path numbered
-- rows by arrival order instead. That was survivable while citations were an
-- access log written in registration order; now that only *cited* evidence is
-- stored, an answer citing [2] and [5] must read back as 2 and 5 or every
-- citation click resolves to the wrong source. Existing rows are backfilled
-- in row order, which is the same guess the read path was already making.

ALTER TABLE chat_citations ADD COLUMN IF NOT EXISTS source_type TEXT NOT NULL DEFAULT 'atom';
ALTER TABLE chat_citations ADD COLUMN IF NOT EXISTS citation_index INTEGER NOT NULL DEFAULT 0;

UPDATE chat_citations c
SET citation_index = numbered.rn
FROM (
    SELECT id, row_number() OVER (PARTITION BY message_id, db_id) AS rn
    FROM chat_citations
) AS numbered
WHERE c.id = numbered.id AND c.citation_index = 0;

INSERT INTO schema_version (version) VALUES (24);
