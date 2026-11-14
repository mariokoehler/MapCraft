-- MapCraft library, schema version 1 (design.md §3.4).
-- Paths are stored relative to their source's root; `*_key` columns are the lower-cased forms
-- used for matching, because Windows file names are case-insensitive.

CREATE TABLE source (
    id           INTEGER PRIMARY KEY,
    root_path    TEXT NOT NULL,            -- as the user picked it: `Z:\Photos` or `\\nas\photos`
    unc_path     TEXT,                     -- the UNC target of a mapped drive, when there is one
    label        TEXT NOT NULL,
    added_at     TEXT NOT NULL,
    last_scan_at TEXT
);

-- The folders picked for import (design.md §2.8(f)); each includes its subfolders.
-- No rows: the whole root.
CREATE TABLE source_folder (
    source_id INTEGER NOT NULL REFERENCES source(id) ON DELETE CASCADE,
    rel_dir   TEXT NOT NULL,
    PRIMARY KEY (source_id, rel_dir)
);

-- One photo: the files of one name in one folder (design.md §2.8(c)), e.g. a RAW, its JPEG and
-- its XMP sidecar.
CREATE TABLE photo (
    id              INTEGER PRIMARY KEY,
    source_id       INTEGER NOT NULL REFERENCES source(id) ON DELETE CASCADE,
    rel_dir         TEXT NOT NULL,          -- '' for the root itself
    rel_dir_key     TEXT NOT NULL,
    stem            TEXT NOT NULL,          -- the file name without extension
    stem_key        TEXT NOT NULL,
    original_name   TEXT NOT NULL,          -- the file "open" refers to
    preview_name    TEXT,                   -- a JPEG next to it, the fast source for thumbnails
    sidecar_name    TEXT,                   -- the XMP sidecar, if any
    files_sig       INTEGER NOT NULL,       -- hash of every member's name, size and time
    file_size       INTEGER NOT NULL,       -- of the original
    file_mtime      INTEGER NOT NULL,       -- of the original, ms since 1970
    quick_hash      INTEGER,                -- xxh3 of the original's first and last 64 KB
    lat             REAL,
    lon             REAL,
    alt             REAL,
    gps_source      TEXT,                   -- 'xmp' or 'exif' (later 'lrcat', §2.8(g))
    taken_at        TEXT,                   -- local time, ISO 8601
    taken_at_offset TEXT,
    camera_make     TEXT,
    camera_model    TEXT,
    lens            TEXT,
    width           INTEGER,
    height          INTEGER,
    orientation     INTEGER,
    state           TEXT NOT NULL DEFAULT 'ok',   -- 'ok' or 'missing'
    read_error      TEXT,                   -- why the metadata couldn't be read, if it couldn't
    thumb_state     TEXT NOT NULL DEFAULT 'none',
    lightcraft_id   INTEGER,                -- §3.6(c)
    UNIQUE (source_id, rel_dir_key, stem_key)
);

CREATE INDEX photo_position ON photo (lat, lon) WHERE lat IS NOT NULL;
CREATE INDEX photo_move ON photo (source_id, file_size, quick_hash);
