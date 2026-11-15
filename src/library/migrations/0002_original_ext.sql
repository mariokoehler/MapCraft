-- The original's extension in lower case (what follows the last dot of `original_name`), for
-- the file-type filter (design.md §2.3). Stored, since computing it per row made a filter change
-- on 100,000 photos about 100 ms slower (2026-11-15).
ALTER TABLE photo ADD COLUMN original_ext TEXT NOT NULL DEFAULT '';

-- `rtrim` with every other character of the name strips back to the last dot; what's left is
-- removed from the front.
UPDATE photo SET original_ext =
    lower(replace(original_name, rtrim(original_name, replace(original_name, '.', '')), ''));
