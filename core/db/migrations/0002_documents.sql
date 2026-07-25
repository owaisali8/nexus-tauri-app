-- Document ingestion and retrieval.
--
-- Vectors are stored as BLOBs and compared by brute-force cosine rather than
-- through a vector index. For a single-user workspace this is the right trade:
-- a dedicated vector store (LanceDB) nearly tripled the dependency tree, and
-- scanning a few thousand 768-dimension vectors takes single-digit
-- milliseconds. If a corpus ever outgrows that, the retrieval interface is
-- narrow enough to swap without touching callers.

PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS documents (
  id          TEXT    PRIMARY KEY,
  title       TEXT    NOT NULL,
  -- Where the text came from: a file path, a URL, or a label for pasted text.
  source      TEXT    NOT NULL,
  mime_type   TEXT    NOT NULL DEFAULT 'text/plain',
  byte_count  INTEGER NOT NULL DEFAULT 0,
  created_at  INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE TABLE IF NOT EXISTS chunks (
  id           TEXT    PRIMARY KEY,
  document_id  TEXT    NOT NULL,
  -- Position within the document, for ordering and citation.
  seq          INTEGER NOT NULL,
  text         TEXT    NOT NULL,
  -- Character offset in the source text.
  offset       INTEGER NOT NULL DEFAULT 0,
  -- Little-endian f32 vector.
  embedding    BLOB    NOT NULL,
  -- Vectors from different models are not comparable, so retrieval filters on
  -- this rather than silently ranking incompatible vectors against each other.
  model        TEXT    NOT NULL,
  dimensions   INTEGER NOT NULL,
  FOREIGN KEY (document_id) REFERENCES documents(id) ON DELETE CASCADE,
  UNIQUE (document_id, seq)
);

CREATE INDEX IF NOT EXISTS idx_chunks_document ON chunks(document_id);
CREATE INDEX IF NOT EXISTS idx_chunks_model ON chunks(model);
