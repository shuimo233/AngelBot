-- Migration 012: Knowledge graph for entity-relationship reasoning

CREATE TABLE IF NOT EXISTS knowledge_nodes (
    id TEXT PRIMARY KEY,
    entity_type TEXT NOT NULL CHECK(entity_type IN ('person','topic','tool','file','project','memory','event')),
    label TEXT NOT NULL,
    properties TEXT,  -- JSON blob for extensibility
    created_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS knowledge_edges (
    id TEXT PRIMARY KEY,
    source_id TEXT NOT NULL REFERENCES knowledge_nodes(id),
    target_id TEXT NOT NULL REFERENCES knowledge_nodes(id),
    predicate TEXT NOT NULL,
    weight REAL DEFAULT 1.0,
    evidence TEXT,
    created_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_kn_type ON knowledge_nodes(entity_type);
CREATE INDEX IF NOT EXISTS idx_ke_source ON knowledge_edges(source_id);
CREATE INDEX IF NOT EXISTS idx_ke_target ON knowledge_edges(target_id);
