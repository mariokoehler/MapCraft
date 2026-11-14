# MapCraft — Design Document

What the app is, how each feature behaves, the architecture, and the roadmap. It describes the
app **as decided or built now**. How a decision was reached belongs in git history once the repo
exists. Update this file in the same session a decision is made. Section numbers are stable
anchors that code and the other docs cite, so never renumber them: add new sections at the end of
their chapter.

**Markers.**
- **(unconfirmed)** marks a concrete default proposed to keep work moving that the user hasn't
  signed off yet. Every one is also listed in §4.
- **(user, YYYY-MM-DD)** marks a decision the user made.
- **(verified YYYY-MM-DD)** marks a fact checked against a primary source or a run on this PC.
- **(unverified)** marks a fact recalled or read second-hand and not yet checked.

## 1. Concept

**MapCraft** (§1.2) shows the user's photos on a world map, like the Map module
in Adobe Lightroom Classic, as a standalone Windows desktop app (user, 2026-11-14).

- **Photos stay where they are.** The user imports folders from local drives or network storage
  (mounted SMB shares). The app records each photo's path and its GPS position from EXIF, and
  stores only a small thumbnail in its own data folder. It never copies the originals (user,
  2026-11-14).
- **The map is the app.** The map is the central view: pan and zoom with the mouse, and search
  for places and photos. Photos appear as thumbnails at their positions. Where too many would
  overlap on screen they merge into groups, and the groups split back into single photos as the
  user zooms in (§2.4).
- **Companion apps do the heavy lifting.** **PhotoCraft** (an open-source Photoshop) and
  **LightCraft** (an open-source Lightroom) are installed alongside. The app uses their CLIs for
  what it shouldn't reimplement, such as decoding RAW files for thumbnails. Later it may also
  open a photo in either app (§2.6, §3.6). LightCraft has no Map module of its own: its README
  lists "Map view" as not started (verified 2026-11-14). That gap is the niche this app fills.
- **Audience:** non-commercial, personal use, mostly the user. The code is in a public GitHub
  repo, but the app isn't sold or advertised (user, 2026-11-14).
- **Tech base: Rust** (user, 2026-11-14). The user is new to Rust and has 25 years of Java
  experience, so explanations relate Rust concepts to their Java equivalents (`CLAUDE.md`).

### 1.1 Scope of version 1 (non-goals)

- **No manual placement.** Photos without GPS can't be put on the map in v1, though that may
  come later (user, 2026-11-14). They are still recorded (§2.1(d)), so a later version can place
  them without a re-import.
- **No editing.** The app doesn't develop, retouch or rate photos; the companion apps do that.
- **Originals are read-only.** The app never writes to an original or creates a sidecar next to
  it, not even an XMP file **(user, 2026-11-14)**. Writing GPS back (for example after manual
  placement or GPX tagging) would be a later, explicit feature.
- **Single user, single machine, one library** **(user, 2026-11-14)**. There is no sync and no
  server.

### 1.2 Name

**MapCraft** (user, 2026-11-14; the working title was "Image Mapper"). It fits the close
integration with PhotoCraft and LightCraft.

- **Not affiliated.** MapCraft isn't affiliated with the storytold / ArtCraft team that makes the
  Craft apps. The README says so explicitly (user, 2026-11-14). The Craft projects encourage people
  to build their own tools with AI.
- **No clash found** (checked 2026-11-14):
  - there is no `storytold/mapcraft` repo and no `mapcraft` crate on crates.io;
  - the nearest namesake is *Mapcrafter*, a Minecraft map renderer;
  - the other GitHub "MapCraft" repos are small and unrelated.
- **Names in code:**
  - the Cargo package and exe are `mapcraft` **(user, 2026-11-14)**;
  - the user-visible name is "MapCraft";
  - the Windows folders are `MapCraft` (§3.3).

### 1.3 Licence

**MIT OR Apache-2.0**, dual-licensed: anyone using the code picks either (user, 2026-11-14).
- It is the Rust ecosystem's usual licence and the one PhotoCraft and LightCraft use (verified
  2026-11-14, their READMEs and `LICENSE-MIT`), so it fits most of our crates.
- Apache-2.0 adds an explicit patent grant; MIT stays compatible with GPLv2-only projects.
- The repo carries `LICENSE-MIT` and `LICENSE-APACHE`, and `Cargo.toml` says
  `license = "MIT OR Apache-2.0"`. The README states that contributions are dual-licensed the
  same way.
- Data the app *displays* has its own terms, which bind the app's behaviour rather than our
  code's licence:
  - OSM tiles: ODbL, attribution on the map (§3.5);
  - GeoNames: CC-BY 4.0, attribution in About (§2.5).

## 2. Features & behaviour

### 2.1 Library & import

**(a) Sources.** The user adds a **source**: a root folder on a local drive, a mapped network
drive (`Z:\Photos`) or a UNC path (`\\nas\photos`). The app scans it recursively. A photo's path
is stored relative to its source root, so a NAS that moves or a drive letter that changes is fixed
by re-pointing the root once, not by re-importing **(user, 2026-11-14)**. A mapped drive letter is
stored together with its UNC target, since mappings differ per Windows session and per user
**(user, 2026-11-14)**.

**(b) What gets imported.** Every file with a supported image extension (§2.2(b)). For each
photo the app records:
- its relative path, file size and modification time;
- its EXIF fields: GPS latitude, longitude and altitude, capture date and time, camera, lens and
  orientation;
- its pixel dimensions and a thumbnail (§2.2).

**(c) Rescan.** "Rescan source" finds new, changed, moved and missing files. A file counts as
changed when its size or modification time differs, and the thumbnail and EXIF are then read
again. A file counts as moved when a new file has the same size and the same quick hash (the
first and last 64 KB) as a missing one, so its record is kept **(user, 2026-11-14)**. Rescans run only
when the user asks, never on a schedule **(user, 2026-11-14)**. Automatic watching is a later idea.

**(d) Photos without GPS** are recorded with `gps = none` and counted on the import summary ("812
imported, 37 without a location"). They don't appear on the map, and they get no thumbnail in v1
**(user, 2026-11-14)**, which saves time on large imports without GPS. A list view of them is a later
idea.

**(e) Unreachable sources.** A source whose root can't be read (NAS off, share unmounted, USB
drive unplugged) is shown as **offline**. Its photos stay on the map, because the thumbnails are
local. Opening an original then says the source is offline instead of failing. Nothing is deleted
because a source is offline: a missing *file* under a reachable root is marked missing, and the
user removes missing photos explicitly **(user, 2026-11-14)**.

**(f) Import runs in the background** with progress and a cancel button. The map stays usable,
and photos appear as their thumbnails land (§3.7).

### 2.2 Thumbnails

**(a) Storage.** Thumbnails live in the data folder (§3.3), never next to the originals. Each
photo gets one **256 px long edge** thumbnail for the map, plus an optional **1024 px** preview
for the detail panel, made on demand and cached **(user, 2026-11-14)**. The format is JPEG at quality
80, or WebP if the UI stack handles it natively **(user, 2026-11-14)**. EXIF orientation is applied
when the thumbnail is made, so it is always stored upright.

**(b) How a thumbnail is made, fastest first** **(user, 2026-11-14)**:
1. **The embedded preview.** It is used when it is at least 256 px. In the user's library
   **every sampled RAW file has an embedded JPEG preview of at least 1024 px**, so it serves the
   256 px thumbnail *and* the 1024 px detail preview (verified 2026-11-14: 300 random files each of
   DNG, CR2 and RAF, read-only over SMB):

   | Type | Long edge of the largest preview |
   |---|---|
   | DNG | always 1024 px (the Adobe converter's standard preview) |
   | CR2 | 2360 to 5760 px, the median 2622 |
   | RAF | 1920 or 4416 px |

   - Finding the preview means reading the TIFF directory (DNG, CR2) or RAF's header pointer, then
     the JPEG's own size. It took about 80 ms per file over SMB, which is mostly the open and a few
     seeks. It shares one open with the EXIF read (§2.8).
   - The preview is the camera's or converter's rendering, not a later edit. That's fine for a map.
   - CRW (27 files, a pre-TIFF Canon format) isn't covered by this reader and goes to LightCraft.
   - A JPEG's own EXIF thumbnail (often about 160 px) is too small; JPEGs are decoded instead (2.).
2. **Decoding in-process** (Rust `image` crate) for JPEG, PNG, TIFF and WebP.
3. **LightCraft** (`lightcraft-cli render <in> -o <out> --size 256`) for RAW and DNG without a
   usable preview, and for anything else the app can't decode. PhotoCraft reads many formats but
   not camera RAW (verified 2026-11-14, its README), so it is the fallback for HEIC and the
   layered formats (PSD, TIFF with layers) **(user, 2026-11-14)**.

**LightCraft is a fallback, so it runs in one long-lived session** **(unconfirmed)**. Timings
on 8 copies each of DNG, CR2, RAF and JPEG from the library, 256 px JPEG output (verified
2026-11-14):

| | One `render` process per file | One `run --script` session |
|---|---|---|
| RAW (DNG, CR2, RAF) | about 1.9 to 2.0 s | about 0.3 to 0.45 s, after a one-off 2 s warm-up |
| JPEG | about 1.6 s | about 60 ms |

- Most of a per-file process is start-up. A session imports the files into a **scratch LightCraft
  library of MapCraft's own** in the data folder (`mode: "add"`, so it only references them) and
  calls `app.export {ids, dir, longEdge: 256}` per photo. It never touches the user's own
  LightCraft library.
- Neither way wrote anything next to the originals: no sidecar and no temp file. That keeps the
  read-only rule (§1.1).
- Copying a file over SMB takes about 10 ms per MB (a 20 to 30 MB RAW in 0.2 to 0.35 s).

**(c) Without the companion apps** (paths not set, or the exe missing), JPEG, PNG, TIFF and WebP
still import fully. RAW files import with whatever embedded preview they have, and a photo with no
preview at all gets a placeholder tile marked "needs LightCraft" **(user, 2026-11-14)**.

### 2.3 Map view

- **Navigation:** drag to pan, mouse wheel to zoom around the cursor, double-click to zoom in,
  `+`/`-` keys, and "zoom to all photos" **(user, 2026-11-14)**.
- **Base map:** a tiled slippy map from the provider chosen in §3.5. The provider's attribution
  is always visible, bottom-right, and is never hidden or covered by panels (a requirement of
  every provider researched).
- **Photos:** a single photo is drawn as its thumbnail in a small frame (about 64 px **(user, 2026-11-14)**)
  with a pin pointing at its exact position. Hovering shows file name, date and camera. Clicking
  selects it and opens the detail panel (§2.6).
- **Groups** (§2.4): drawn as the thumbnail of one representative photo, with a count badge.
  Clicking zooms to the group's extent. If a group can't split any further (photos taken at the
  same spot), clicking opens a strip of its photos instead **(user, 2026-11-14)**.
- **Selection** is shared between the map and the detail panel **(user, 2026-11-14)**.
- **Filters** (date range, source, camera) narrow what the map shows **(unconfirmed for v1; maybe
  M5)**.

### 2.4 Grouping (clustering)

**Behaviour:** two photos are in the same group when their markers would overlap on screen at the
current zoom, about 64 px apart **(user, 2026-11-14)**. Groups depend only on the zoom level, not on
the pan, so a group doesn't change while the user drags. Zooming in by one level halves the
distance on the ground, and groups split. The representative of a group is its newest photo
(user, 2026-11-14).

**Algorithm (user, 2026-11-14):** hierarchical greedy clustering per integer zoom level (0 to about
20), the approach used by Mapbox's Supercluster. It is precomputed once over a spatial index (a
KD-tree) after each import, so a frame only looks up the groups in view. Supercluster handles
hundreds of thousands of points in well under a second, so library size isn't a worry for this
approach. The real size of the user's library is asked in §4. A Rust port may exist as a crate
(unverified); otherwise it is a few hundred lines to write ourselves. Between two zoom levels,
groups fade or animate apart **(user, 2026-11-14; nice-to-have)**.

### 2.5 Search

One search box with two kinds of results **(user, 2026-11-14)**:
- **Places:** "Lisbon", "Yosemite" moves the map there. The default is **offline** search against
  the GeoNames cities dataset (CC-BY 4.0, about 3 MB), the same data LightCraft downloads for its
  reverse geocoding (verified 2026-11-14, `metadata.downloadPlaceNames`). It is instant, free, and
  allows autocomplete. It sends nothing to anyone, which matters because the photos' locations are
  private. It knows only cities, not addresses or landmarks. Online geocoding (Nominatim or the
  tile provider's own) is an optional add-on; see §3.5(c) for why it can't drive autocomplete.
- **Photos:** file name, folder, date ("2019-08"), camera. A result centres the map on the photo
  and selects it.

### 2.6 Photo detail & opening originals

The detail panel shows the 1024 px preview, the file path, the source (and whether it's online),
date, camera, lens, coordinates, and actions **(user, 2026-11-14)**:
- **Show in Explorer** (`explorer /select,<path>`): always available.
- **Open with the default app**: always available.
- **Open in PhotoCraft:** launch `photocraft.exe <path>`. It opens the file in a window titled
  after it. If PhotoCraft is already running, the file opens as a new tab there, and the second
  process exits within about 200 ms (verified 2026-11-14, PhotoCraft 0.22.0 and its `--help`).
  `--new-instance` forces a separate window.
- **Open in LightCraft:** selects the photo in a running LightCraft (§3.6(c)).

### 2.7 Settings

- **Data folder** (§3.3), with a "move data folder" action that copies and then switches
  **(user, 2026-11-14)**.
- **Companion apps:** the paths to `photocraft-cli.exe` and `lightcraft-cli.exe` (and their GUI
  exes, found next to the CLIs). Defaults: `C:\Program Files\PhotoCraft\` and
  `C:\Program Files\LightCraft\` (user, 2026-11-14). A "Test" button runs `--version`.
- **Map:** the provider, its API key if one is needed, and the tile cache's size limit.
- **Sources:** add, remove, re-point a root (§2.1(a)), rescan.

### 2.8 The user's library (surveyed 2026-11-14)

The user's main photo store is a folder on a NAS share, mapped as a drive letter. The exact path is
kept out of this public repo, in Claude's local memory. Only a fraction of it will be imported. A
read-only survey (a full directory walk plus EXIF reads of a random sample of 800 photos) found:

- **Size.** About 117,000 files, roughly 1.6 TB, in about 270 top-level folders, mostly named
  `YYYY-MM-DD_Place`.

  | Type | Files | Size |
  |---|---|---|
  | JPEG | 62,000 | 204 GB |
  | DNG | 40,000 | 1.1 TB |
  | CR2 | 6,600 | 106 GB |
  | RAF | 1,600 | 47 GB |
  | TIFF | 1,100 | 92 GB |
  | PNG | 700 | |

  Also a few PSD and CRW, about 550 videos (AVI, MOV, MP4), and 1,870 XMP sidecars. No HEIC turned
  up.
- **RAW and JPEG pairs.** Many shoots hold the same frame twice, as `DSCF8440.dng` and
  `DSCF8440.JPG` side by side. There are also edited copies such as `IMG_4544_bearbeitet.JPG`.
- **GPS.** About 16 % of the sampled photos have a GPS position in EXIF:

  | Type | With GPS |
  |---|---|
  | JPEG | 19 % |
  | DNG | 9 % |
  | CR2 | 31 % |
  | RAF | 0 % |

  Most hits are phone photos (the `iOSGallery` and `Handy_*` folders). Another 13 % have an empty
  GPS block (the tag is present but has no position). Of the XMP sidecars, **59 % contain
  `GPSLatitude`**, so GPS often lives only in the sidecar.
- **Lightroom history.** There is a `Lightroom Catalog Backup` folder and `.lrprev` files: the
  library was managed in Lightroom Classic at some point.
- **Speed over SMB.** Walking the whole tree took 35 to 65 seconds. Reading EXIF headers took about
  40 ms per file, single-threaded. A first full import of 100,000 files would take about an hour
  for EXIF alone at that rate, so it needs parallel reads and must be resumable (§3.7).
- **GPS will be added later.** Many photos have no GPS yet. Today's workflow (user, 2026-11-14):
  download the GPX from Komoot (or Garmin), then let Lightroom Classic's Map module match it to
  the photos by capture time, with a time-zone offset where needed.
- **Where Lightroom puts that GPS** (Lightroom Classic's documented behaviour; not yet checked
  against the user's catalogue):
  - Auto-tagging writes the positions into the **catalogue** (`.lrcat`) only.
  - They reach the files only when metadata is saved, either with *Metadata › Save Metadata to
    File* (Ctrl+S) or with the catalogue setting *Automatically write changes into XMP*.
  - For proprietary RAW (CR2, RAF) they go into an **XMP sidecar**. For JPEG, TIFF, PSD and DNG
    they are written **into the file itself**.

  That fits the survey: GPS turned up in most sidecars but in few RAW files. It also means some
  tagged photos may have their position **only in the catalogue**.
- **The catalogue** exists, but not on this PC. The user can make it reachable if needed (user,
  2026-11-14).

What follows for the design (all **(user, 2026-11-14)**):
- **(a) GPS from sidecars.** GPS is read from the XMP sidecar as well as from EXIF. When both have
  a position, the sidecar wins, as in LightCraft and Lightroom.
- **(b) Rescan picks up new GPS.** A photo whose file *or* sidecar changed is re-read, so GPS
  written later by a track tool appears after a rescan (§2.1(c)).
- **(c) RAW and JPEG pairs count as one photo.** Files with the same base name in the same folder
  are one photo, like Lightroom's "treat JPEG next to raw as one". The JPEG gives the fast
  thumbnail, and the RAW is the original that "open" refers to. Edited copies (`_bearbeitet`) stay
  separate photos.
- **(d) Videos are left out of v1.**
- **(e) Excluded folders.** Each source has exclude patterns, by default `.dtrash`,
  `Lightroom Catalog Backup` and `*.lrdata`.
- **(f) Picking folders.** Since only part of the library is imported, adding a source shows its
  folder tree with checkboxes, so the user picks folders rather than whole roots.
- **(g) Getting GPS out of Lightroom.** Two routes:
  1. **Simplest, no code.** In Lightroom, select all photos and choose *Save Metadata to File*
     once. Every position then sits in the files or sidecars, where (a) reads it. This writes to
     the originals and sidecars, which is the user's call to make in Lightroom.
  2. **Read the catalogue.** A `.lrcat` is a SQLite database, so MapCraft can read it
     (read-only, on a copy) as an extra GPS source. GPS should be in `AgHarvestedExifMetadata`
     (`gpsLatitude`, `gpsLongitude`, `hasGps`). It joins to the file path through `Adobe_images`
     → `AgLibraryFile` → `AgLibraryFolder` → `AgLibraryRootFolder`. That schema is unverified:
     check it on the real catalogue. The catalogue's root paths may name old drive letters, so
     each Lightroom root is mapped to a MapCraft source when importing.

  Default: file metadata first (sidecar, then EXIF), and the catalogue only fills photos that
  have no position. Each photo records where its position came from (`gps_source`).
- **(h) Track matching inside MapCraft (later).** The GPX step could move into MapCraft itself
  through LightCraft's `photo.autoTagTracklog`, which takes a GPX file and a camera clock offset.
  MapCraft would then *write* GPS, which §1.1 rules out for v1.

## 3. Architecture

### 3.1 High-level shape

One desktop process, Windows first **(user, 2026-11-14: Windows only for v1; most of the stack would
also run on macOS and Linux)**. The parts:
- **UI**: the map, panels and dialogs.
- **Map engine**: tiles, projection, markers, groups.
- **Library**: SQLite, holding sources, photos and EXIF.
- **Importer**: scanning, EXIF reading, thumbnail jobs.
- **Companion bridge**: launches and talks to the CLIs.
- **Settings.**

The UI thread never does I/O. Import, thumbnails and tile downloads run on worker threads and post
results back (§3.7).

### 3.2 Tech stack

**Rust** (user, 2026-11-14), stable toolchain via `rustup`, target `x86_64-pc-windows-msvc`. On
Windows it needs Microsoft's linker and the Windows SDK, which come with the Visual Studio Build
Tools' C++ workload. Installed on this PC: Rust 1.100.0 with clippy and rustfmt, and Build Tools
2022 17.14 with Windows SDK 10.0.26100 (verified 2026-11-14, a test build).

**UI stack** (the hardest decision to reverse). The options as compared:

| | **egui + walkers** | **Tauri + MapLibre GL JS** | **Slint / Iced + own map** |
|---|---|---|---|
| What | Immediate-mode Rust GUI; `walkers` is a Leaflet-like map widget for it (MIT; OSM, Mapbox-style raster, offline PMTiles, vector tiles via wgpu; verified 2026-11-14, its README) | Rust backend, web UI in the system WebView2; MapLibre is the leading open web map renderer | Retained-mode Rust GUIs |
| Language | 100 % Rust | Rust plus TypeScript/JS for the UI | 100 % Rust |
| Map maturity | Good, smaller community (about 290 stars) | Excellent (vector styles, clustering, labels) | We'd write the map ourselves |
| Many thumbnails | Textures on the GPU, fast | Needs WebGL symbol layers; DOM markers slow down past a few thousand | Depends |
| Look | Functional and "tool-like"; same toolkit as LightCraft and PhotoCraft (verified, their READMEs) | Anything CSS can do | Native-ish |
| For a Rust newcomer | Immediate mode is unlike Swing/JavaFX, but simple: the UI is a function redrawn every frame | Split brain: two languages, an IPC bridge | Fine |

**Decision (user, 2026-11-14): egui + walkers.** It keeps the whole app in one language, matching the
user's wish to learn Rust. It is the toolkit the companion apps use, and it draws thousands of
thumbnail textures cheaply. The risk is that walkers' feature set falls short and we extend it.
The M1 spike settles that, starting with two things the OSM policy requires that walkers' README
doesn't mention: setting our own User-Agent, and a disk tile cache that honours `Cache-Control`
(unverified).

**Other crates (unconfirmed, versions picked at setup):** `rusqlite` (SQLite, bundled), `kamadak-exif`
(EXIF from JPEG, TIFF, HEIF, PNG and WebP; TIFF-based RAW such as DNG, NEF and ARW should work;
Canon CR3 may not; unverified), `image` (decode and resize), `serde` + `toml` (settings), `rayon`
or a small thread pool (thumbnail jobs), `reqwest` (tiles, if walkers doesn't fetch them itself),
`tracing` (logging).

### 3.3 Where data lives

- **Bootstrap settings:** `%APPDATA%\MapCraft\settings.toml` **(user, 2026-11-14)**. This file has a
  fixed place because it holds the one setting that can't live inside the data folder: where the
  data folder is. It also holds the window layout and companion paths.
- **Data folder:** configurable (user, 2026-11-14). Default `%LOCALAPPDATA%\MapCraft\`
  **(user, 2026-11-14)**. It contains:
  - `library.sqlite`: sources, photos, EXIF, groups cache.
  - `thumbs/ab/cd/<photo-id>_256.jpg`: sharded so no folder holds more than a few thousand files.
  - `previews/…`: the 1024 px previews, evictable.
  - `tiles/`: the map tile cache (§3.5(d)), evictable.
  - `logs/`.
- A data folder on a NAS works but is slow, and SQLite over SMB risks corruption if two machines
  open it at once. The app warns and doesn't forbid it **(user, 2026-11-14)**.

### 3.4 Data model (first draft, unconfirmed)

```
source(id, root_path, unc_path, label, added_at, last_scan_at)
photo(id, source_id, rel_path, file_size, file_mtime, quick_hash,
      lat, lon, alt, gps_source /* exif | xmp | lrcat, §2.8(g) */, taken_at, taken_at_offset, camera_make, camera_model, lens,
      width, height, orientation, state /* ok | missing | no_gps */, thumb_state,
      lightcraft_id /* nullable, §3.6(c) */)
  unique(source_id, rel_path); index on (lat, lon)
schema_version(version)
```

Migrations are numbered SQL files applied at startup. Clusters are rebuilt in memory from
`photo(lat, lon)` and not stored, unless measuring shows startup needs it.

### 3.5 Map providers & geocoding (researched 2026-11-14)

The first decision of the project (user, 2026-11-14). **Audience: non-commercial, personal use,
mostly the user; the code is in a public GitHub repo, but the app isn't sold or advertised**
(user, 2026-11-14). That fits the free, non-commercial tiers below. Because the repo is public, no
API key is ever committed: each user enters their own key in Settings.

**(a) Tile providers**

- **OpenStreetMap Standard** (`tile.openstreetmap.org`). Free, no key, but donation-funded and
  best effort. Its usage policy (verified 2026-11-14,
  https://operations.osmfoundation.org/policies/tiles/):
  - The app needs its own identifying User-Agent (a library default gets blocked).
  - Attribution must be visible, never hidden behind toggles or panels.
  - Cache headers must be honoured, or tiles kept at least 7 days.
  - Forbidden: bulk downloading, prefetching tiles the user isn't viewing, and **offline use**.
  - Access may be withdrawn without notice.

  Fine for development and a personal tool; not something to ship to many users.
- **MapTiler Cloud** (verified 2026-11-14, https://www.maptiler.com/cloud/pricing/). The free plan
  is for "testing, PoC, prototyping, personal, or non-commercial use":
  - 5k map sessions and 100k API requests a month; service pauses when used up.
  - Needs a MapTiler logo on the map.
  - Paid from $30/month.
  - API key required. Whether a key embedded in a desktop app is acceptable, and its caching
    terms, aren't on the page (unverified).
- **Stadia Maps** (verified 2026-11-14, https://stadiamaps.com/pricing): free plan with 200,000
  credits a month, no commercial use; Starter $20/month for 1M credits. Caching rights are tied to
  paid plans; desktop use isn't addressed.
- **Thunderforest**: has a free hobby plan (OSM wiki); quota not confirmed (unverified).
- **Google Map Tiles API:**
  - Needs billing enabled on the project (a credit card) and a key on every request.
  - Capped at 15,000 2D tile requests per project per day.
  - Verified 2026-11-14 for both:
    https://developers.google.com/maps/documentation/tile/usage-and-billing
  - Policies (verified 2026-11-14,
    https://developers.google.com/maps/documentation/tile/policies): no pre-fetching or caching
    beyond `Cache-Control`, no offline use, Google logo plus data attribution, visualisation only.
  - Price: 100,000 free 2D tile requests a month, then $0.60 per 1,000 (unverified, from search
    summaries of https://developers.google.com/maps/billing-and-pricing/pricing).
  - Highest friction and most rules; the satellite imagery is the main draw.
- **Self-hosted Protomaps PMTiles** (verified 2026-11-14, https://docs.protomaps.com/basemaps/downloads):
  - One file holds the whole planet's vector tiles: about 120 GB, zoom 0–15, ODbL, OSM
    attribution required.
  - `pmtiles extract` cuts a region or a lower max zoom; each zoom level roughly doubles the size,
    so zoom 0–10 worldwide is a few GB.
  - No key, no quota, no cost, works **offline**, and sends no locations anywhere.
  - Needs vector rendering (walkers has it behind a feature flag), and street detail stops at
    zoom 15.

**(b) Decision (user, 2026-11-14):** a provider abstraction from day one, starting with OSM.
Online with a disk cache is enough for v1; offline PMTiles comes later (user, 2026-11-14). The
three implementations:
1. **OSM Standard** for development and the first personal builds.
2. **A keyed provider** (MapTiler or Stadia) where the user pastes their own free key in Settings.
   Each user then brings their own quota, which sidesteps the "embedded key" question.
3. **Local PMTiles** as the offline, private option. A small worldwide low-zoom file could ship
   with the app as the fallback when offline.

Google only if satellite imagery becomes a must.

**(c) Geocoding (place search).** Nominatim's public server (verified 2026-11-14,
https://operations.osmfoundation.org/policies/nominatim/):
- at most 1 request a second across *all* users of the app;
- **client-side autocomplete is forbidden**;
- results must be cached, and the app must be able to switch it off.

Hence the offline GeoNames default in §2.5, with an online provider (Nominatim, or the keyed tile
provider's geocoder) as an explicit "search online" action, not search-as-you-type
**(user, 2026-11-14)**.

**(d) Tile cache.** Tiles are cached on disk in the data folder as long as the provider's headers
allow, with a size limit (default 500 MB **(user, 2026-11-14)**, least recently used evicted first).
There is no prefetching for OSM or Google, by their policies.

**(e) Privacy.** Every tile request tells the provider which area the user is looking at, which
for a photo library is close to where the user has been. That is accepted as not an issue (user,
2026-11-14).

### 3.6 Companion apps (checked 2026-11-14)

Installed: **PhotoCraft 0.22.0** and **LightCraft 0.20.0**, both pure Rust, egui, MIT/Apache-2.0,
alpha (verified 2026-11-14, `--version` and their READMEs). Repos:
https://github.com/storytold/photocraft, https://github.com/storytold/lightcraft.

**(a) LightCraft CLI** (`lightcraft-cli --help`, `commands`: 778 lines of commands):
- `render <in> -o <out> --size N`: develops any supported file, RAW included, and exports it.
  About 2 s per file, mostly start-up, so MapCraft's fallback renders use a `run --script`
  session and `app.export` instead (§2.2(b)).
- `run --library DIR --import PATH <commands…>`: headless, against a real library; `--connect`
  drives a running app instead.
- `mcp`: an MCP server, headless or attached to a running app.
- `photo.allMetadata {id}`: every EXIF, TIFF and GPS tag. A fallback EXIF reader for formats our
  crate can't parse **(unconfirmed)**.
- `photo.autoTagTracklog` (positions from a GPX track by capture time) and
  `metadata.reverseGeocode` (Fill Location from GPS, offline via GeoNames): both potentially
  useful later.
- `album.*` (albums are LightCraft's collections): `album.create {name, addSelected?}`,
  `album.addPhotos {id, ids}`, `album.createSmart` with rules. These could later turn a group or a
  map selection into a LightCraft album, once the path → id mapping works (§3.6(c), §5 Later).
- `library.importLightroom {path: .lrcat}`: imports a Lightroom Classic catalogue. Not ours to use
  directly, but a sign the user's library may come from Lightroom.

**(b) PhotoCraft CLI** (`photocraft-cli --help`):
- `convert <in> <out>` handles PSD, PNG, JPG, TIFF, WebP, EXR and more (`--quality` for
  JPEG/WebP).
- `serve` keeps one headless session answering JSON lines.
- No camera RAW (verified, README).
- `photocraft --control` gives an authenticated loopback control channel for the GUI.

**(c) Opening a photo in LightCraft: it works** (verified 2026-11-14, LightCraft 0.20.0, against a
scratch library and a scratch settings folder):
- **Lookup without changes.** `library.importPreview {paths}` reports each file the library
  already has as `{duplicate: "path" | "content", existing: <photo id>}` and adds nothing. A
  `content` match also finds a copy at another path, which helps when MapCraft and LightCraft know
  the same NAS under different drive letters or UNC paths. Avoid `library.import` here: it does
  report `duplicates[].existing` the same way, but it *adds* a file the library doesn't have.
- **Select.** `library.select {ids: [id], active: id}` over `lightcraft-cli run --connect
  127.0.0.1:<port>` selects it in the GUI. Lookup plus select took about 235 ms in all.
- **Back from id to path.** `photo.inspect {id}` returns `source.path`, so `photo.lightcraft_id`
  can be cached and checked.
- **The control channel** exists only when the GUI was started with `lightcraft --control <port>`
  (or `LIGHTCRAFT_CONTROL_PORT`). It is loopback-only with **no authentication**
  (`docs/control-protocol.md`), so it shouldn't stay on by default.
- **The flow** **(unconfirmed)**:
  - If LightCraft isn't running, MapCraft starts it with `--control` on a random free port. The
    GUI opens the user's last library.
  - If it is running without `--control`, MapCraft can't drive it and says so.
  - If the photo isn't in LightCraft's library, MapCraft offers to import it (`mode: "add"`) and
    doesn't do it silently.
- **Not found:** a command that switches the GUI to its Develop or Loupe view. The photo is
  selected, but the view stays where it was. An upstream issue for a `library.reveal {path}` that
  also switches the view would make this one call.
- `lightcraft --library DIR` and `LIGHTCRAFT_CONFIG_DIR` keep experiments away from the user's
  real library and its "last opened" setting.

### 3.7 Background work

- A job queue with a fixed pool of worker threads, sized to the CPU but at most 4 for CLI renders
  **(user, 2026-11-14)**. In Java terms: an `ExecutorService` with a `BlockingQueue`; in Rust,
  `std::thread` plus channels (`crossbeam-channel`) or `rayon`.
- Workers send results to the UI over a channel. The UI drains it at the start of each frame and
  never blocks.
- Jobs are cancellable. Import progress is persisted per file, so a cancelled or crashed import
  resumes rather than restarts.

### 3.8 Security & robustness notes

- GPS coordinates and file paths are personal data. Nothing leaves the machine except tile requests
  (and online geocoding if the user enables it) (§3.5(e)). Logs don't contain coordinates
  **(user, 2026-11-14)**.
- API keys are stored in `settings.toml` in plain text **(user, 2026-11-14)**, which is outside the
  repo. The repo is public (§3.5), so a key never goes into code, tests or committed config. The
  Windows Credential Manager is the better place if this ever matters.
- LightCraft's control port has no authentication: only enable it on demand, and turn it off
  again (§3.6(c)).
- Untrusted files: a malformed image must not crash the app. Decoding runs in workers that catch
  panics (`catch_unwind`), and the CLIs run as separate processes with a timeout.

### 3.9 Visual design

The look and the structure of the screens are designed in **Claude Design** (user, 2026-11-14).
- Claude writes a prompt for a screen or component, and the user pastes it into Claude Design.
- The mockup lands on a canvas Claude can open directly (the Artifact tool lists them).
- Claude then builds it in egui.
- Mockups are for the first layout, new screens and bigger changes. Small tweaks go straight into
  code.

The first canvases set a small design system:
- colours (light and dark), type sizes, spacing;
- the marker, group-badge and panel styles.

The egui theme mirrors those tokens in one place (a `theme` module) and is kept in lockstep with
the design system. Sensible starting points (**user, 2026-11-14**): a dark UI around a light map, as in
Lightroom's Map module, and a family resemblance to PhotoCraft and LightCraft, since they share
egui.

## 4. Open questions

Foundational, blocking the architecture:
- **Lightroom GPS** (§2.8(g)): save metadata to the files once in Lightroom (route 1), or should
  MapCraft read the `.lrcat` (route 2, which needs the catalogue on this PC)? Possibly both:
  route 1 now, and route 2 only if photos turn up whose position is only in the catalogue.
- **Videos:** left out of v1 (§2.8(d)); wanted later?

Product:
- **Where the photos live:** what NAS and share layout, mapped drive letters or UNC (§2.1(a),
  (e)), and is the library also in Lightroom Classic today (§3.6(a))?
- **Groups** (§2.4): the badge style.

Unconfirmed defaults awaiting review:
- §2.3: filters in M5 rather than v1.
- §3.2: the crate list.
- §3.4: the data model.
- §2.2(b): LightCraft as the fallback renderer in one long-lived session, on a scratch library of
  MapCraft's own.
- §3.6(a): `photo.allMetadata` as the fallback EXIF reader.
- §3.6(c): the Open-in-LightCraft flow (start it with `--control`, ask before importing).

## 5. Roadmap

Milestones are rough and get refined as we go.

- **M0 — Decisions & spikes** (now). Done: the tile provider, the UI stack, offline as later
  (user, 2026-11-14), the library survey (§2.8), the licence (§1.3) and the public GitHub repo
  (https://github.com/mariokoehler/MapCraft), the Rust toolchain (§3.2), and the four spikes
  (2026-11-14): embedded previews and LightCraft render timing (§2.2(b)), `photocraft.exe <path>`
  (§2.6), and LightCraft path → id (§3.6(c)). Still to do:
  - answer the remaining questions in §4.
- **M1 — Window with a map:**
  - the Cargo project;
  - a first Claude Design pass for the main window's layout and the design system (§3.9);
  - a window with the map, pan and zoom, tiles from OSM with a proper User-Agent, the disk cache
    and attribution;
  - settings with the bootstrap file.
- **M2 — Library & import:** SQLite schema, add a source, a background scan, EXIF/GPS reading,
  progress, rescan.
- **M3 — Thumbnails on the map:** the thumbnail pipeline (§2.2(b)) with LightCraft for RAW,
  markers as thumbnails, hover and select, the detail panel.
- **M4 — Grouping:** clustering per zoom (§2.4), count badges, click to zoom, the strip for photos
  at one spot.
- **M5 — Search & filters:** GeoNames place search, photo search, date and source filters.
- **M6 — Companion links & providers:** Open in PhotoCraft, Show in Explorer, Open in LightCraft
  if M0's spike found a way; the keyed provider and PMTiles offline.
- **M7 — Packaging:** an installer or a portable zip, an icon, the GitHub repo and CI.

**Later:** manual placement of photos without GPS (and optionally writing GPS back), GPX track
tagging (maybe via LightCraft's `photo.autoTagTracklog`), "make a LightCraft album from this group or
selection" (`album.create`, §3.6(a)), watched folders, a timeline/date slider, heatmap
mode, multiple libraries, travel routes drawn between photos in time order.
