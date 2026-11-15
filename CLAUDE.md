# CLAUDE.md — MapCraft project notes

Memory that carries across sessions: how we work, where things live, and the pitfalls that cost
time before. Keep it short, because it's loaded into every session.

- What the app *is* and why belongs in `design.md`.
- Update the right file in the same session a decision or finding is made.
- Don't log features here: a new system gets one line under "What's built", and its details go in
  `design.md`.

## What this project is

**MapCraft** (`design.md` §1.2) is a Windows desktop app in **Rust** that shows
the user's photos on a world map, like Lightroom Classic's Map module. Photos are referenced by
path on local or SMB drives and never copied. Each gets a thumbnail in a configurable data folder.
Photos are placed by their EXIF GPS position and merge into groups that split as the user zooms in.
The companion apps PhotoCraft and LightCraft handle the formats we don't decode, RAW above all.

It is for non-commercial personal use, mostly the user's own. The code will be public on GitHub,
but the app isn't sold or advertised (user, 2026-11-14).

**Docs:**
- `design.md`: the concept, features (§2), architecture (§3, with the map-provider research in
  §3.5 and the companion-app findings in §3.6), open questions and every unconfirmed default (§4),
  and the roadmap (§5).
- The model for these docs is the user's project LaunchWindow (a sibling folder of this one; its
  `CLAUDE.md` and `design.md`): mined for conventions, not code.

## Current status (2026-11-15)

**v0.2.0 released (2026-11-15)**: M8 (data folder) done. v0.1.0 (M7) before it; updating
from GitHub was checked with a 0.0.9 copy. The user runs the release from `E:\MapCraft` and
tests the update 0.1.0 → 0.2.0. M6
(companion apps) awaits the user's try. M5 is done (tried by the user).
- Public GitHub repo: https://github.com/mariokoehler/MapCraft (since 2026-11-14), branch
  `main`. Commit doc changes too, so the history guards against overwrites from stale editor
  copies.
- Rust is installed for the Windows user (2026-11-14): `rustup` stable (1.100.0), MSVC target,
  with VS 2022 Build Tools (C++ workload). See "Build and run".
- Decided (2026-11-14): egui + walkers (§3.2); a tile-provider abstraction starting with OSM,
  online for v1 (§3.5(b)); licence MIT OR Apache-2.0 (§1.3). The user's library is surveyed
  (§2.8).
- M0 is done (2026-11-14): the spikes (results in `design.md` §2.2(b), §2.6, §3.6(c)), and GPS
  comes from EXIF and XMP only in v1 (§2.8(g)). The design system and the main
  window's mockup exist and are approved (§3.9).
- M1 and M2 are done (2026-11-14), tested by the user (M2 on about 4,700 Iceland photos).
- M3 is done (2026-11-14), tested by the user on the Iceland set (`design.md` §5, §2.2(d)).
- M4 is done (2026-11-14), tried by the user (`design.md` §2.4, §5).
- **The user's real photo library is read-only to us.** Never write, rename or create sidecars
  there. Its path stays out of the repo (it's in Claude's local memory).

**What's built:**
- The main window: top bar plus the OSM map (walkers) with zoom controls, scale bar,
  attribution and the empty-library card (`src/app.rs`, `src/ui/`).
- The theme, read from the design system's tokens (`src/theme/`, `assets/design/tokens.json`).
- The tile provider (`src/provider.rs`) and the bootstrap settings (`src/settings.rs`).
- The metadata reader: our own TIFF/EXIF reader for TIFF, DNG, CR2, JPEG and RAF, plus XMP
  sidecars (`src/metadata/`).
- The library (SQLite, `src/library/`) and the import with rescan, moves, missing photos and
  folder decisions (`src/import/`), run in the background by `src/jobs.rs`.
- The import UI: add folder / change folders, sources, progress, toasts, position dots
  (`src/ui/`). Generated test photos with real pixels: `src/demo.rs`.
- Thumbnails (`src/thumbs.rs`) made by `ThumbJob` (`src/jobs.rs`), the thumbnail markers
  (`src/ui/markers.rs`), the hover card and the detail panel (`src/ui/details.rs`), and Show in
  Explorer / Open (`src/launch.rs`).
- Grouping per zoom level (`src/cluster.rs`, built off the UI thread in `src/ui/map_view.rs`),
  group markers and the strip (`src/ui/strip.rs`).
- Search: the offline place index from GeoNames (`src/places.rs`), the search thread
  (`src/search.rs`) and the field with its dropdown (`src/ui/search.rs`).
- Filters (date, sources, cameras, file types): one SQL condition for map, counts and search
  (`src/filter.rs`), the panel, chips and no-match card (`src/ui/filters.rs`).
- Companion apps: PhotoCraft launch and the LightCraft control-port client
  (`src/companions.rs`), the Settings dialog (`src/ui/settings_dialog.rs`), the panel's buttons.
- The data folder: usage, change with move, free up space, remove old data
  (`src/datafolder.rs`, `src/ui/data_dialogs.rs`, the Data folder section of Settings).
- Releases: the tag-triggered workflow (`.github/workflows/release.yml`), the exe's icon and
  version (`build.rs`), the static C runtime (`.cargo/config.toml`), licence notices
  (`scripts/notices.ps1`, `about.toml`); self-update (`src/update.rs`,
  `src/ui/update_dialog.rs`).

## How we work

- **Explain Rust in Java terms.** The user has 25 years of Java experience and is new to Rust.
  - When a Rust concept first comes up, name the nearest Java equivalent and where it differs.
  - Examples: a `trait` is close to an `interface`; a `crate` is a Maven artifact and `Cargo.toml`
    is its `pom.xml`; `Result<T, E>` plus `?` replaces checked exceptions; ownership and borrowing
    have no Java equivalent (no GC); a `struct` plus `impl` blocks is a class without inheritance;
    an `enum` carries data, like a sealed interface with records; `Option<T>` is `Optional<T>`
    that the compiler enforces; `Arc<Mutex<T>>` is a shared object with `synchronized` access;
    channels are a `BlockingQueue`; a `mod` is a package.
  - Don't over-explain what Java already has under the same name.
- **Unspecified sub-details:** propose a concrete default in `design.md`, flagged
  **(unconfirmed)** and listed in §4, rather than stalling. Ask only about foundational or
  hard-to-reverse choices.
- **Facts about outside services** (tile providers, APIs, crates) are checked against a primary
  source and dated in `design.md`, as **(verified YYYY-MM-DD)** with the URL. Quotas and terms
  change, so re-check before relying on an old one.
- **Screens and the look: Claude Design** (user, 2026-11-14; `design.md` §3.9).
  - For a new screen, a new component or a bigger layout change, Claude makes the mockup itself:
    a Design canvas named `MapCraft — <screen>`, built on the design system
    `MapCraft — Design system` (find both with the Artifact tool's `list`). The user reviews and
    comments there; then Claude builds from it.
  - The design system serves `tokens.json` and `components/bundle.css` but no `tokens.css`, so a
    canvas carries its own generated `ds/mapcraft/tokens.css` (dark on `:root`, light under
    `.theme-light`). Regenerate it when the tokens change.
  - Small changes go straight into code.
  - Keep the egui `theme` module in lockstep with the design system whenever a canvas adds or
    changes a token.
- **Advisor before and after** any multi-file feature.
- **Flag unprompted:** security and privacy concerns (briefly). Photo locations are personal data
  (`design.md` §3.8).
- **The repo will be public:** no API keys, personal paths beyond the defaults, or personal photos
  in it. Test images are our own or generated. Exception: the README's screenshots
  (`docs/screenshots/`) show the user's Iceland photos on the map, at the user's wish
  (2026-11-15); they show thumbnails, places and the trip's year, never paths, file names or
  the detail panel. Take them from a scratch copy of the data folder.
- **Commits are not signed** in this repo (user, 2026-11-14): the GPG key is for the user's job
  only. `commit.gpgsign = false` is set in the repo's local git config. The global Kleopatra rule
  doesn't apply here.

## Tools outside the repo

- **LightCraft 0.21.1** (an open-source Lightroom, pure Rust/egui):
  - CLI: `C:\Program Files\LightCraft\lightcraft-cli.exe`; GUI: `lightcraft.exe` beside it. Both
    paths are configurable in the app.
  - `render <in> -o <out> --size N` develops any file, RAW included.
  - `run [--library DIR] [--import PATH] <cmd> key=value …` runs headless commands;
    `commands [--json]` lists them all.
  - `--connect` drives a GUI started with `--control <port>`. That port is loopback-only with no
    authentication.
  - Facts and open questions are in `design.md` §3.6.
  - The user's LightCraft is a fresh install with no collections yet (2026-11-14). We may create
    some while exploring, but ask before changing the user's real library: use a scratch
    `--library` folder for experiments. For the GUI, also set `LIGHTCRAFT_CONFIG_DIR` to a scratch
    folder, or it may remember the scratch library as "last opened". Headless `run --library`
    calls left the real settings folder (`%APPDATA%\LightCraft`) untouched (checked 2026-11-14).
  - The GUI's control port takes JSON lines; engine commands go through
    `engine.execute {command, params}`, and `ui.screenshot {path}` lets LightCraft capture its
    own window. Never start a second `lightcraft.exe`: it can't open a library already open.
  - Pitfall: `LIGHTCRAFT_CONFIG_DIR` alone isn't enough. A LightCraft started without
    `--library`, whose settings name no library (`libraryPath` in `ui.json`), opens the default
    `%USERPROFILE%\Pictures\LightCraft Library`: the user's real one. It happened once
    (2026-11-15) and upgraded that catalog's format. For a scratch run, set `libraryPath` in the
    scratch `ui.json` first, or pass `--library`.
  - Tests never start LightCraft: `Config.launch` off records the calls, and
    `src/companions.rs` tests talk to a pretend LightCraft on a local port.
  - Pitfall: Windows PowerShell 5.1 strips the quotes from JSON arguments (`bad JSON params`).
    Write JSON lines `{"command": id, "params": {…}}` to a file and pass `--script FILE` instead.
- **PhotoCraft 0.23.1** (an open-source Photoshop, pure Rust/egui):
  - CLI: `C:\Program Files\PhotoCraft\photocraft-cli.exe`; GUI: `photocraft.exe` beside it.
  - `convert <in> <out>`, `info`, `run`, `batch`, `serve`, `mcp`; `commands` lists about 1,000
    engine commands.
  - No camera RAW.
  - Test with `PHOTOCRAFT_CONFIG_DIR` set to a scratch folder, so the user's recent files stay
    untouched.
- Upstream repos: https://github.com/storytold/lightcraft, https://github.com/storytold/photocraft
  (docs in `docs/`, e.g. `docs/control-protocol.md`).

## Build and run

One Cargo project (`Cargo.toml` is its `pom.xml`; `Cargo.lock` pins every version and is
committed):
- **A release:** bump `version` in `Cargo.toml`, build once (so `Cargo.lock` follows), commit
  both, then tag `vX.Y.Z` and push the tag; the workflow checks the tag against `Cargo.toml`,
  tests, builds and publishes `mapcraft.exe`, its `.sha256` and `THIRD-PARTY-NOTICES.txt`.
  "Run workflow" in the Actions tab rehearses without publishing. Never push a tag without the
  user's go-ahead. Release notes say when a version migrates the library (one-way).
- `cargo run` builds a debug build and starts it. A console window shows the log, and
  `RUST_LOG=debug` makes it chattier. `cargo run --release` builds the optimised build, without
  a console window.
- `cargo test` runs the unit tests (`#[cfg(test)] mod tests` at the bottom of each file).
- Before each commit: `cargo fmt` (formatter), `cargo clippy --all-targets` (a linter, like
  SpotBugs/Checkstyle in one; keep it warning-free), `cargo test`.
- The first build takes about 1.5 minutes; incremental ones take seconds.
- On disk: `%APPDATA%\MapCraft\settings.toml` (created on first start) and
  `%LOCALAPPDATA%\MapCraft\tiles\` (the tile cache).
- Thumbnails and previews: `%LOCALAPPDATA%\MapCraft\thumbs\` and `previews\` (by photo id).
- The place index: `%LOCALAPPDATA%\MapCraft\geonames\places.sqlite`, downloaded when the user
  asks (`--data` folders have their own).
- `%LOCALAPPDATA%\MapCraft\library.sqlite` is created by the first normal start. From then on
  the migrations in `src/library/migrations/` are frozen: change the schema with a new numbered
  file, never by editing an existing one (`design.md` §3.4).
  - A new migration needs a fixture in `FIXTURES` (its schema's sample data) and, once the
    version holding it is released, its hash in `RELEASED` (both in `src/library/mod.rs`
    tests); the tests say so if one is missing or a released one changed.
- The crate is a library (`src/lib.rs`, all modules) plus a thin binary (`src/main.rs`), so tests
  and the dev probes in `examples/` can use everything.
- `mapcraft --data <folder>` runs the GUI on another data folder (trials never touch the real
  library).
- Dev command: `mapcraft --import <folder> --data <scratch folder> [--readers N]` imports one
  folder headless and prints the summary and GPS rates per file type. Run it from the release
  build and pipe its output (`cmd /c "… 2>&1"`): the release exe is a GUI-subsystem app and
  prints nothing to a console otherwise.
- More dev flags:
  - `--open add-folder[=<folder>]`, `--open sources`, `--open details`, `--open strip`,
    `--open filters[=<year>]`, `--open settings` or `--open search=<text>` starts with that dialog open, the first
    photo selected, the first strip open, the filter panel open (or only that year shown), or
    the text searched (for comparing with the mockups);
  - `--perf` logs the frame time every 3 s;
  - `--make-demo <folder> [n]` writes generated photos;
  - `--fill-library <scratch data> <n>` writes synthetic rows;
  - `--settings <file>` uses another settings file (kept across a restart). Trials of the
    data folder use a scratch one whose `data_folder` is scratch: Change… rewrites the
    settings file and the old data's removal deletes MapCraft's files from the old folder.
  - `--open data` opens Settings on the Data folder section; `--open change-data=<folder>`
    opens the Change dialog as if that folder had been picked (no system picker needed).
  - `--update-from <base url>` checks a pretend GitHub (`<base>/latest`, `<base>/dl/…`) instead
    of the real one. Try the updater only on a copy of the exe in the scratchpad, with
    `--data` scratch: it replaces the exe it runs from.
- `pwsh scripts/notices.ps1 [out]` writes the third-party notices (needs
  `cargo install cargo-about --features cli`).
- Pitfall: never let a test reach `update::install`: it would replace the test binary. Tests
  run with `launch` off, which records the install instead.
- UI tests: `egui_kittest` drives the whole app headless (`src/app.rs` tests), finding widgets
  by their accessible names. Its accessibility check fails on unnamed icon buttons, so use
  `widgets::ghost` or `widgets::named`.
  - Painted things (markers) need an `interact` rect with a name to be found. A `TextEdit` has
    no name of its own (the hint isn't one): set it with `ctx.accesskit_node_builder`, and
    click it before `type_text`.
  - Tests never reach the network: `Config.network` is off and downloads are only recorded.
  - Paint transient decorations (a spinner over an image) with `paint_at`, never `ui.put`/`add`:
    an extra widget shifts the layout and the ids below it, and clicks get lost.
  - Wait for the thumbnail job (`thumbs.is_none()`) before `h.run()`: its chip's spinner keeps
    the frame repainting.
- `cargo run --release --example make_icon` remakes `assets/brand/mapcraft.ico` and the 256 px
  window icon from the master `assets/brand/mapcraft-icon-1024.png`; commit the results.
- `cargo run --release --example cluster_probe -- [n]` times the grouping on synthetic photos.
- `places_probe -- <cities500.zip> <admin1> <countryInfo> <out.sqlite> [queries…]` builds the
  place index from downloaded GeoNames files and times searches (public data only).
- Probes (read-only, aggregates only): `cargo run --release --example exif_probe -- <list>`
  (our reader against `kamadak-exif`), `xmp_probe -- <folder>`, `unc_check -- <path>`.
- Pitfall: `kamadak-exif` reads a whole TIFF-based file (a 25 MB RAW) before parsing, and fails
  on a partial read. Use our `metadata` module for TIFF, DNG, CR2, JPEG and RAF.
- Pitfall: on the NAS, time goes to each file's open and first read (about 40 ms), not to
  parsing or bandwidth. Count opens, not bytes, when tuning the import.
- Pitfall: probes and scratch imports of the real library contain real positions. Keep them in
  the scratchpad and delete them afterwards; never print names or coordinates.
- Pitfall: the API of egui/eframe 0.37 differs from most examples online (for example
  `App::ui` instead of `update`, `Panel::top`, `Id::unique`). Check signatures in
  `%USERPROFILE%\.cargo\registry\src\` or let the compiler say, not from memory.
- Pitfall: never script real mouse or keyboard input to test the GUI. Windows often refuses to
  bring the app to the front, and the input lands in whatever the user has open (it did once,
  2026-11-14, in their browser). Test interaction headlessly (`egui_kittest`) or ask the user.
- Pitfall: capture a window with `PrintWindow` (its own contents), never by copying the screen
  area. A screen copy picks up whatever covers the window; it once captured the user's other
  work (2026-11-14). The capture script is in the scratchpad (`scripts\shot.ps1`), and every
  capture is deleted after use.
- Library queries that take a filter use named parameters (`:name`, `query_named`): the
  filter's condition adds its own, so positional `?1` would collide.
- Pitfall: a backslash in SQL inside a Rust string needs `\\` (`ESCAPE '\\'`); `'\'` is just
  a quote. Edit it with the Edit tool: shell and Python quoting layers eat it.
- Pitfall: a process that starts a GUI app (a probe starting LightCraft) never "finishes"
  in PowerShell while the app runs. Its output pipe is inherited by the child, and
  `Start-Process -Wait` also waits for children. Redirect to files and use
  `$p.WaitForExit()`.
- Pitfall: the borrow checker forbids two closures holding `&mut self` at once, for example a
  dialog's body and button row. Let the button row set local flags, and act on them after the
  dialog has drawn.

**Toolchain** (per Windows user, shared by all Rust projects):
- `rustup` manages Rust like SDKMAN manages JDKs. It lives in `%USERPROFILE%\.rustup`, with
  `cargo`, `rustc` and the tools in `%USERPROFILE%\.cargo\bin`, which is on the user PATH.
- `rustup update` upgrades to the latest stable.
- A shell started before the install may not see `cargo` yet. Prepend `%USERPROFILE%\.cargo\bin`
  to `PATH` for that session.
- Linking uses MSVC from VS 2022 Build Tools (C++ workload, Windows SDK). Without it, builds
  fail at the link step.
