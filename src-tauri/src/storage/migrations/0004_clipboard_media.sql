CREATE TABLE clipboard_items (
    id TEXT PRIMARY KEY,
    content_kind TEXT NOT NULL CHECK (content_kind IN ('text','image')),
    text_content TEXT,
    content_sha256 TEXT NOT NULL CHECK (length(content_sha256) = 64),
    source_app TEXT CHECK (source_app IS NULL OR length(source_app) <= 260),
    pinned INTEGER NOT NULL DEFAULT 0 CHECK (pinned IN (0,1)),
    captured_at INTEGER NOT NULL CHECK (captured_at >= 0),
    last_seen_at INTEGER NOT NULL CHECK (last_seen_at >= captured_at),
    byte_size INTEGER NOT NULL CHECK (byte_size >= 0),
    UNIQUE(content_kind, content_sha256),
    CHECK (
        (content_kind = 'text' AND text_content IS NOT NULL) OR
        (content_kind = 'image' AND text_content IS NULL)
    )
);

CREATE INDEX clipboard_items_list_idx
ON clipboard_items(pinned DESC, last_seen_at DESC, id);

CREATE TABLE clipboard_assets (
    id TEXT PRIMARY KEY,
    clipboard_item_id TEXT NOT NULL UNIQUE REFERENCES clipboard_items(id) ON DELETE CASCADE,
    asset_name TEXT NOT NULL UNIQUE,
    mime_type TEXT NOT NULL CHECK (mime_type = 'image/png'),
    width INTEGER NOT NULL CHECK (width BETWEEN 1 AND 8192),
    height INTEGER NOT NULL CHECK (height BETWEEN 1 AND 8192),
    sha256 TEXT NOT NULL CHECK (length(sha256) = 64),
    byte_size INTEGER NOT NULL CHECK (byte_size BETWEEN 1 AND 20971520),
    created_at INTEGER NOT NULL CHECK (created_at >= 0)
);
