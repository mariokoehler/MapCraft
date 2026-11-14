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

   | Type | Long edge of the largest preview (of 300 files) |
   |---|---|
   | DNG | 1024 px in 291 (the Adobe converter's standard preview); 3504 to 5760 px in 9 |
   | CR2 | 5760 px in 116, 1936 px in 110, 1536 px in 72, 5184 px in 2 |
   | RAF | 4416 px in 211, 1920 px in 54, 2048 px in 35 |

   - Finding the preview means reading the TIFF directory (DNG, CR2) or RAF's header pointer, then
     the JPEG's own size. It took 60 to 80 ms per file over SMB, which is mostly the open and a few
     seeks. It shares one open with the EXIF read (§2.8).
   - **Pitfalls** (found in the spike):
     - A CR2's raw image is itself stored as a *lossless* JPEG (SOF3). Only baseline and
       progressive JPEGs (SOF0–SOF2) count as previews.
     - A preview is cut out by its recorded length: `JPEGInterchangeFormatLength` or
       `StripByteCounts` in TIFF, and the length in RAF's header. Searching for the end-of-image
       marker stops early, at the EXIF thumbnail inside the preview.
     - Cut out that way, the previews of two sample files per type all decoded at the expected
       size.
   - The preview is the camera's or converter's rendering, not a later edit. That's fine for a map.
   - CRW (27 files, a pre-TIFF Canon format) isn't covered by this reader and goes to LightCraft.
   - A JPEG's own EXIF thumbnail (often about 160 px) is too small; JPEGs are decoded instead (2.).
2. **Decoding in-process** (Rust `image` crate) for JPEG, PNG, TIFF and WebP.
3. **LightCraft** (`lightcraft-cli render <in> -o <out> --size 256`) for RAW and DNG without a
   usable preview, and for anything else the app can't decode. PhotoCraft reads many formats but
   not camera RAW (verified 2026-11-14, its README), so it is the fallback for HEIC and the
   layered formats (PSD, TIFF with layers) **(user, 2026-11-14)**.

**LightCraft is a fallback, so it runs in one long-lived session** **(user, 2026-11-14)**. Timings
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

**(d) As built in M3 (2026-11-14).** Measured on the user's Iceland set (4,778 photos, 3,800
with a position; read-only over SMB):
- **Order:** the embedded preview (the smallest one at least as large as needed, else the largest
  of at least 256 px), then the paired JPEG, then decoding the original (JPEG, PNG, TIFF, WebP).
  - The embedded preview comes *before* the paired JPEG **(unconfirmed)**. That departs from
    §2.8(c)'s "the JPEG gives the fast thumbnail". On the NAS the cost is each file's open
    (about 40 ms), and the RAW is opened anyway. Every CR2 in the set carries a 5760 px preview.
  - The downside: an edited JPEG export beside the RAW doesn't show; the camera's rendering does.
- **Speed:** a CR2 takes about 150 ms cold on one thread. With 4 workers it's 93 ms per photo,
  and with 8 it's 80 ms, so the NAS is the limit. MapCraft uses **4 workers** **(unconfirmed)**.
  Full decodes (TIFF, large PNG) run one at a time.
- **TIFFs** (681 in the set, median 126 MB, largest 2.5 GB) have no embedded preview and take
  about 2 s each.
  - Originals over **400 MB or 120 megapixels aren't decoded**: they get the placeholder
    **(unconfirmed)**. That was 9 of the 48 TIFFs with a position.
  - Colour: exports in ProPhoto (also linear, 32-bit float) or Adobe RGB are converted to sRGB,
    chosen by their ICC profile's description. Without that, thumbnails came out far too dark.
    Resizing is done in linear light.
- **Failures:**
  - A file with no usable source gets the placeholder marker (state `failed`).
  - A read error (the NAS asleep, a source offline) leaves the photo pending, and it is tried
    again on the next run.
- **Validity:** a thumbnail stays valid while three inputs are unchanged: the original's quick
  hash, the paired JPEG's name and the orientation.
  - A moved photo keeps it, and so does a sidecar-only edit.
  - Known limit: a paired JPEG re-exported under the same name doesn't remake the thumbnail. That
    only matters when the thumbnail came from that JPEG, which embedded-first makes rare.
- **Files:**
  - Paths are `thumbs/xx/yy/<id>_256.jpg` and `previews/xx/yy/<id>_1024.jpg` in the data folder.
    Each is written to a `.tmp` file and renamed.
  - Each run first deletes the files of photos that no longer exist.
  - A re-made thumbnail deletes the photo's preview, which is made again when it's next shown.
- **When it runs:**
  - At start-up when thumbnails are pending, after each import, and after a removal.
  - It is not restarted just because photos are still pending: an offline source would loop it.
  - Imports, rescans, folder changes and removals wait until it has stopped, because it writes by
    photo id.
- **Not in M3:** the LightCraft fallback (3. above) is deferred to M6 or later **(unconfirmed)**.
  Until then, CRW and RAWs without a usable preview get the placeholder.
- **Mac leftovers:** AppleDouble files (`._name`, magic `00 05 16 07`, 117 in the set) are
  ignored by the scan, and records from before that are removed by the next rescan. PSDs are no
  longer read errors: MapCraft reads no metadata from them.

### 2.3 Map view

- **Navigation:** drag to pan, mouse wheel to zoom around the cursor, double-click to zoom in,
  `+`/`-` keys, and "zoom to all photos" **(user, 2026-11-14)**.
- **Base map:** a tiled slippy map from the provider chosen in §3.5. The provider's attribution
  is always visible, bottom-right, and is never hidden or covered by panels (a requirement of
  every provider researched).
- **Photos:** a single photo is drawn as its thumbnail in a small frame (about 64 px **(user, 2026-11-14)**)
  with a pin pointing at its exact position. Hovering shows file name, date and camera. Clicking
  selects it and opens the detail panel (§2.6).
- **Position dots** (user, 2026-11-14): a photo appears as a small pin dot as soon as the import
  has read its position, and becomes its thumbnail marker once the thumbnail exists. Reading
  metadata costs one small read per file, but a thumbnail needs a decode, so thumbnails are made
  by a background job after the import (M3), and the map is useful from the first minute. In M2
  every photo is a dot. Dots closer than 5 px are drawn once. With 100,000 photos over Europe
  at continent zoom (the worst case), the release build draws about 30 frames per second, with
  8 ms of UI work per frame (measured 2026-11-14, `--fill-library` and `--perf`). M4's grouping
  replaces this.
- **Until M4's grouping (built in M3, unconfirmed):**
  - One photo per cell of a grid shows as a marker; the rest show as dots.
    - The grid has 80 px cells at the whole-number zoom. It is anchored to the world, not the
      screen, so panning never swaps which photo shows.
    - A cell's marker is the photo with the lowest id among those with a thumbnail, else a
      placeholder.
  - Dots right at a marker's pin are left out, so the pin stays readable.
  - Photos still waiting for their thumbnail stay dots.
- **Selection (built in M3, unconfirmed):**
  - Clicking the map's background or pressing Esc clears the selection.
  - The panel's × hides the panel and keeps the selection; Ctrl+I shows or hides it again.
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

**As built in M3 (2026-11-14):**
- Show in Explorer runs `explorer.exe /select,"<path>"`, and Open runs `explorer.exe "<path>"`,
  which opens the file with the app Windows associates with it.
  - The quoted path is passed as one raw argument and never through a shell. `cmd /c start`
    would interpret `&`, `%` and `^` in a file name.
  - The source's root is resolved on a background thread, because a sleeping NAS can take
    seconds.
- PhotoCraft and LightCraft are shown disabled until M6.
- Two things in the mockup are left out for now **(unconfirmed)**:
  - The source's online state: checking it needs the NAS, and the panel reads only the library.
  - The exposure row: the import doesn't read exposure values yet.
- The preview is made when the photo is first selected, then kept.

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
  a position, the sidecar wins, as in LightCraft and Lightroom. In the library the sidecars hold
  it as an attribute or as an element, always as degrees and decimal minutes (`53,32.6N`); all
  1,112 sidecars that mention a position are read (verified 2026-11-14). The paired JPEG's EXIF is
  read only when the original can't be read or has no capture date **(user, 2026-11-14)**: every
  extra file costs a disk seek on the NAS, and a camera that leaves GPS out of the RAW leaves it
  out of its JPEG too.
- **(b) Rescan picks up new GPS.** A photo whose file *or* sidecar changed is re-read, so GPS
  written later by a track tool appears after a rescan (§2.1(c)).
- **(c) RAW and JPEG pairs count as one photo.** Files with the same base name in the same folder
  are one photo, like Lightroom's "treat JPEG next to raw as one". The JPEG gives the fast
  thumbnail, and the RAW is the original that "open" refers to. Edited copies (`_bearbeitet`) stay
  separate photos.

  Found in the library (surveyed 2026-11-14, read-only): 5,170 names have more than one image
  file. Mostly DNG+JPEG (4,547) and CR2+JPEG (293), but also DNG+RAF (113), DNG+JPEG+RAF (66) and
  CR2+DNG (24): DNGs converted from a camera RAW, sitting beside it. Rules for these
  **(user, 2026-11-14)**:
  - The original is the **DNG** if there is one, else the camera RAW, else TIFF/PSD, else
    PNG/HEIC, else the JPEG. Here a DNG is mostly the converted, edited working copy.
  - The **preview** is a JPEG of the same name.
  - Any further files of the name (the camera RAW beside a DNG) belong to the photo too.
  - Names match case-insensitively, as Windows does.

  Sidecars come in both naming styles: `name.xmp` (1,525) and `name.ext.xmp` (282); 63 have no
  image. `name.<original ext>.xmp` wins over another `name.ext.xmp`, which wins over `name.xmp`
  **(user, 2026-11-14)**.
- **(d) Videos are left out of v1.**
- **(e) Excluded folders.** Each source has exclude patterns, by default `.dtrash`,
  `Lightroom Catalog Backup` and `*.lrdata`.
- **(f) Picking folders.** Since only part of the library is imported, adding a source shows its
  folder tree with checkboxes, so the user picks folders rather than whole roots.

  The pick is stored as **decisions**: (folder, included yes/no). A folder without one inherits
  from its nearest parent, and with no decisions everything is included. So:
  - a folder added later under an included folder is imported by the next rescan;
  - the files lying directly in a partly picked folder are a decision like any other;
  - unticking everything imports nothing.

  The import passes through left-out folders only to reach included ones below them. The tree
  starts with everything ticked **(unconfirmed)**. Leaving a folder out later ("Change
  folders…") removes its photos' records, after a confirmation that names the count; the files
  are never touched **(unconfirmed)**.
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

  **Decision (user, 2026-11-14): file metadata only in v1**, the XMP sidecar first and then EXIF
  (see (a)). MapCraft doesn't read the catalogue. The user saves the Lightroom positions to the
  files with route 1 at a later date. It doesn't block development: many photos already carry GPS
  in EXIF or a sidecar. Route 2 stays a later idea if photos turn up whose position exists only in
  the catalogue. Each photo records where its position came from (`gps_source`).
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

**walkers 0.65 covers what the OSM policy needs** (verified 2026-11-14, its source
`src/io/http.rs`, and M1 running):
- `HttpOptions.user_agent` is applied to every request. Its default is `walkers/<version>`, so
  MapCraft sets its own: `MapCraft/<version> (+https://github.com/mariokoehler/MapCraft)`.
- `HttpOptions.cache` is an on-disk HTTP cache (`http-cache-reqwest` with `cacache`, in its
  default mode, which follows the servers' caching headers). A second start on the same view
  added no cache entries. It has **no size limit**; see §3.5(d).
- It doesn't draw the attribution; MapCraft draws it.
- Its default wheel behaviour is Ctrl+wheel to zoom, so MapCraft sets `zoom_with_ctrl(false)`.

**Crates in use (M1, 2026-11-14):**
- `eframe`/`egui` 0.37 (the window and the UI) and `egui_extras` with `svg` (the icons);
- `walkers` 0.65 (the map);
- `serde` + `toml` (settings) and `serde_json` (the design tokens);
- `directories` (the AppData folders);
- `tracing` + `tracing-subscriber` (logging); `tempfile` (tests only).

**Added in M2:**
- `rusqlite` 0.42 with bundled SQLite (the library, §3.4);
- `quick-xml` 0.42 (XMP sidecars);
- `xxhash-rust` (the quick hash and the files' signature);
- `windows-sys` (`WNetGetConnectionW`, the UNC target of a mapped drive);
- `kamadak-exif` 0.6, only for PNG, HEIF and WebP, and as the reference in tests;
- `rfd` 0.18 (the native folder picker);
- `egui_kittest` 0.37 (tests only: drives the UI headless, with an accessibility check).

**EXIF is read by MapCraft's own small TIFF reader** (`src/metadata/tiff.rs`). Measured
2026-11-14 on 240 random files of the library (60 each of CR2, DNG, JPEG, RAF):
- `kamadak-exif` reads a whole TIFF-based file into memory first: 140–250 ms per CR2 or DNG over
  SMB. A partial read makes it fail on most RAWs, and it can't read RAF at all.
- Our reader takes the first 64 KB and seeks only to the Exif and GPS directories. JPEG and RAF
  go through the same code, via the EXIF block and the embedded JPEG.
- Positions and capture dates agree with a whole-file `kamadak-exif` read on every file, to the
  last digit.

**Planned crates (unconfirmed):**
- `image` (decode and resize);
- `rayon` or a small thread pool (thumbnail jobs).

**Assets in the repo:**
- IBM Plex Sans (Regular, SemiBold) and Mono (Regular) as static TTFs in `assets/fonts/`, from
  IBM's GitHub releases (`@ibm/plex-sans@1.1.0`, `@ibm/plex-mono@2.5.0`), under the SIL OFL 1.1
  (`assets/fonts/OFL.txt`). Google Fonts only has Plex Sans as a variable font, which egui can't
  split into weights.
- Lucide icons from `lucide-static` 1.65.0 in `assets/icons/`, ISC licence.
- The design system's `tokens.json` in `assets/design/`, copied from `MapCraft — Design system`
  (version `1794665619-d118`, 2026-11-14). Copy it again when the design system changes.

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

### 3.4 Data model (M2, unconfirmed)

The schema is `src/library/migrations/0001_init.sql`; in short:

```
source(id, root_path, unc_path, label, added_at, last_scan_at)
source_folder(source_id, rel_dir, included) -- folder decisions (§2.8(f)); none = everything
photo(id, source_id, rel_dir, stem,        -- one photo = the files of one name in one folder
      original_name, preview_name, sidecar_name,
      files_sig,                           -- hash of all members' names, sizes, times
      file_size, file_mtime, quick_hash,   -- of the original
      lat, lon, alt, gps_source /* exif | xmp; lrcat later, §2.8(g) */,
      taken_at, taken_at_offset, camera_make, camera_model, lens, width, height, orientation,
      state /* ok | missing */, read_error, thumb_state, lightcraft_id /* §3.6(c) */)
  unique(source_id, rel_dir_key, stem_key) -- lower-cased: Windows names are case-insensitive
```

- **Migrations** are numbered SQL files compiled into the app. `PRAGMA user_version` records
  how many have run.
- **WAL mode:** the import writes while the UI reads, through its own connection.
- **Clusters** are rebuilt in memory from `photo(lat, lon)` and not stored, unless measuring
  shows startup needs it.

**Import rules (M2, verified by tests):**
- **Unchanged** photos (same names, sizes and times of all files, `files_sig`) aren't read
  again. A rescan of 591 unchanged photos took 0.1 s, and a cancelled import resumes where it
  stopped.
- **Not found any more:** a known photo that a run doesn't find is judged only if its folder was
  seen well enough:
  - its folder was listed completely; or
  - the nearest completely listed parent no longer contains the folder (renamed or deleted).

  Otherwise it stays as it is: when its folder exists but couldn't be read, sits below an
  unreadable folder, or is outside the picked folders. A cancel or an unreachable source changes
  nothing.
- **Moved or missing:** a new name with the same size and quick hash as a vanished photo is that
  photo, so its record (and later its thumbnail) is kept. A renamed folder therefore moves its
  photos rather than duplicating them. The remaining vanished photos become missing, which
  takes them off the map but keeps their records.
- **Quick hash:** the size plus the first and last 64 KB (§2.1(c)), read on the same handle as
  the metadata, so each original is opened once.
- **I/O errors and format errors are handled differently:**
  - A file that can't be opened or read at all (NAS hiccup, locked file) is skipped and counted.
    Its record, or its absence, stays as it was, and the next rescan tries again, so a hiccup
    during a rescan can't wipe a position.
  - Unreadable metadata is recorded with the error, and the photo is read again on every
    rescan.
  - Reader panics are caught.
- **Speed:** 4 parallel readers **(user, 2026-11-14)**. On the NAS an import runs at about 25–30
  photos per second, whether with 1, 4 or 16 readers, and with or without the tail of the quick
  hash. The limit is the NAS's per-file latency (measured 2026-11-14), so a first import of the
  whole library would take about an hour.
- **Read-only, checked:** four real folders (598, 902, 1,736 and 818 files) had an identical
  fingerprint of every file's name, size and time before and after importing.
- **Mapped drives:** the UNC target of the user's mapped drive resolves and is readable
  (checked 2026-11-14). If the drive letter isn't mapped, the stored UNC path is used.

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

**The size limit isn't built yet.** walkers' cache has no limit (§3.2), so MapCraft has to trim
the cache folder itself. Proposal **(unconfirmed)**: at startup, remove the least recently used
entries until the folder is under the limit, through the cache library's own API (`cacache`
keeps an index next to the content, so deleting files by hand could corrupt it; to be checked in
M6, with the other providers). Until then the cache grows with every new area viewed.

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
- **The flow** **(user, 2026-11-14)**:
  - If LightCraft isn't running, MapCraft starts it with `--control` on a random free port. The
    GUI opens the user's last library.
  - If it is running without `--control`, MapCraft can't drive it and says so.
  - If the photo isn't in LightCraft's library, MapCraft offers to import it (`mode: "add"`) and
    doesn't do it silently. `importPreview` shows that case as a candidate with no `duplicate`
    and no `existing` (verified).
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
- **Claude makes the mockups itself**, as Claude Design canvases named `MapCraft — <screen>`,
  built from the design system's tokens and component styles (user, 2026-11-14). The user reviews
  them and comments on the canvas.
- Claude then builds the screen in egui from the canvas.
- Mockups are for the first layout, new screens and bigger changes. Small tweaks go straight into
  code.
- Canvases so far, each artboard with a dark/light switch:
  - `MapCraft — Main window` (approved 2026-11-14), with six states: browsing, a photo selected
    from a group strip, search, an import running, a source offline, and an empty library.
  - `MapCraft — Import` (M2, approved 2026-11-14), with five states: "Add photo folder" with the
    folder tree, the import running (with the proposed position dots), the finished summary, the
    sources list, and removing a source.

**A design system of MapCraft's own** (user, 2026-11-14) is its own canvas,
`MapCraft — Design system` (done 2026-11-14; the user made it in Claude Design). It holds:
- colour tokens for dark and light, the type scale, spacing, radii and strokes;
- the generic controls, the panels and overlays, and the map's own objects (markers, group
  badges, attribution).

It is shaped to map onto egui's `Style` and `Visuals`, so the `theme` module is a transcription.
Fonts and icons must be under open licences that allow bundling them in a public repo.

The egui theme mirrors those tokens in one place (a `theme` module) and is kept in lockstep with
the design system. Sensible starting points (**user, 2026-11-14**): a dark UI around a light map, as in
Lightroom's Map module, and a family resemblance to PhotoCraft and LightCraft, since they share
egui.

## 4. Open questions

Not blocking v1:
- **Videos:** left out of v1 (§2.8(d)); wanted later?

Product:
- **Where the photos live:** what NAS and share layout, mapped drive letters or UNC (§2.1(a),
  (e)), and is the library also in Lightroom Classic today (§3.6(a))?
- **Groups** (§2.4): the badge style.

Unconfirmed defaults awaiting review:
- §2.2(d) (M3):
  - the embedded preview before the paired JPEG;
  - 4 thumbnail workers;
  - no decoding of originals over 400 MB or 120 MP;
  - the LightCraft fallback deferred to M6 or later.
- §2.3 (M3):
  - one marker per 80 px world-grid cell until M4, the lowest id first;
  - deselecting by a background click or Esc;
  - × hides the panel, Ctrl+I toggles it.
- §2.6 (M3): no online state and no exposure row in the panel yet.
- §2.3: filters in M5 rather than v1.
- §3.2: the planned crates.
- §2.8(f): the tree starts all ticked; leaving a folder out later removes its records; a folder
  that is, lies inside, or contains an existing source can't be added (it would duplicate
  photos; compared case-insensitively and through UNC paths).
- §3.4: the data model.
- §3.5(d): trimming the tile cache at startup, in M6.
- §3.6(a): `photo.allMetadata` as the fallback EXIF reader.

## 5. Roadmap

Milestones are rough and get refined as we go.

- **M0 — Decisions & spikes** (done 2026-11-14). Done: the tile provider, the UI stack, offline as later
  (user, 2026-11-14), the library survey (§2.8), the licence (§1.3) and the public GitHub repo
  (https://github.com/mariokoehler/MapCraft), the Rust toolchain (§3.2), and the four spikes
  (2026-11-14): embedded previews and LightCraft render timing (§2.2(b)), `photocraft.exe <path>`
  (§2.6), and LightCraft path → id (§3.6(c)). Still to do:
  - answer the remaining questions in §4.
- **M1 — Window with a map** (done 2026-11-14):
  - the Cargo project;
  - the design system canvas, then the main window's mockup built on it (§3.9);
  - a window with the map, pan and zoom, tiles from OSM with a proper User-Agent, the disk cache
    and attribution;
  - settings with the bootstrap file.

  The window matches the mockup's "Empty library" state. Search, "Add folder…", settings, "zoom to
  all photos" and the map-style menu are drawn disabled until their milestones. Checked by
  screenshot and the tile cache; pan, zoom (buttons, `+`/`-` keys, wheel, double-click) and the
  attribution link still need a check by hand.
- **M2 — Library & import** (done 2026-11-14; tried by the user on 4,700 photos): SQLite schema, add a source, a background scan,
  EXIF/GPS reading, progress, rescan.
  - Done: the metadata reader, the library database and the headless import with rescan, moves
    and missing photos, checked on real folders (§3.4); folder picks as decisions (§2.8(f)).
  - Done, the UI (canvas `MapCraft — Import`, approved):
    - "Add photo folder" and "Change folders…" with the tree;
    - the sources list with rescan and remove;
    - the progress chip with cancel;
    - toasts with the numbers;
    - position dots;
    - "zoom to all photos".

    Checked by headless UI tests (`egui_kittest`) and by captures of the untouched window, on
    generated photos.
- **M3 — Thumbnails on the map** (built 2026-11-14; awaiting the user's try):
  - Done: the thumbnail pipeline (§2.2(d)) as a background job;
  - thumbnail markers (§2.3) with textures loaded off the UI thread;
  - the hover card;
  - selection and the detail panel with its preview, Show in Explorer and Open (§2.6).

  Checked on the real Iceland set for the thumbnails (probes, aggregates only). The UI is checked
  by headless tests and window captures on generated photos. The LightCraft fallback is deferred
  (§2.2(d)).
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
