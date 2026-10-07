-- Migration 006: Vector store support tables
-- Creates the rowid mapping table for sqlite-vec memory_vectors virtual table.
-- The actual vec0 virtual table is created programmatically after sqlite-vec is loaded.

CREATE TABLE IF NOT EXISTS memory_vector_map (
    memory_id TEXT PRIMARY KEY,
    rowid INTEGER UNIQUE NOT NULL,
    FOREIGN KEY(memory_id) REFERENCES memories(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_memory_vector_map_rowid ON memory_vector_map(rowid);
