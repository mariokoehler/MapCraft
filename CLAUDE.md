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

## Current status (2026-11-14)

**Design phase; nothing is built.**
- Public GitHub repo: https://github.com/mariokoehler/MapCraft (since 2026-11-14), branch
  `main`. Commit doc changes too, so the history guards against overwrites from stale editor
  copies.
- Rust is installed for the Windows user (2026-11-14): `rustup` stable (1.100.0), MSVC target,
  with VS 2022 Build Tools (C++ workload). See "Build and run".
- Decided (2026-11-14): egui + walkers (§3.2); a tile-provider abstraction starting with OSM,
  online for v1 (§3.5(b)); licence MIT OR Apache-2.0 (§1.3). The user's library is surveyed
  (§2.8).
- The M0 spikes are done (2026-11-14; results in `design.md` §2.2(b), §2.6, §3.6(c)). Next: the
  Lightroom-GPS question in §4, then M1 (needs the main window's Claude Design pass, §3.9).
- **The user's real photo library is read-only to us.** Never write, rename or create sidecars
  there. Its path stays out of the repo (it's in Claude's local memory).

**What's built:** nothing yet.

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
  - For a new screen, a new component or a bigger layout change, Claude writes a self-contained
    Claude Design prompt in a copyable code block. It covers purpose, content, states, existing
    tokens and components to reuse, and the egui constraints.
  - The user pastes it into Claude Design. Claude then opens the resulting canvas itself (the
    Artifact tool's `list`, canvases named `MapCraft — <screen>`) and builds from it.
  - Small changes go straight into code.
  - Keep the egui `theme` module in lockstep with the design system whenever a canvas adds or
    changes a token.
- **Advisor before and after** any multi-file feature.
- **Flag unprompted:** security and privacy concerns (briefly). Photo locations are personal data
  (`design.md` §3.8).
- **The repo will be public:** no API keys, personal paths beyond the defaults, or personal photos
  in it. Test images are our own or generated.
- **Commits are not signed** in this repo (user, 2026-11-14): the GPG key is for the user's job
  only. `commit.gpgsign = false` is set in the repo's local git config. The global Kleopatra rule
  doesn't apply here.

## Tools outside the repo

- **LightCraft 0.20.0** (an open-source Lightroom, pure Rust/egui):
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
  - Pitfall: Windows PowerShell 5.1 strips the quotes from JSON arguments (`bad JSON params`).
    Write JSON lines `{"command": id, "params": {…}}` to a file and pass `--script FILE` instead.
- **PhotoCraft 0.22.0** (an open-source Photoshop, pure Rust/egui):
  - CLI: `C:\Program Files\PhotoCraft\photocraft-cli.exe`; GUI: `photocraft.exe` beside it.
  - `convert <in> <out>`, `info`, `run`, `batch`, `serve`, `mcp`; `commands` lists about 1,000
    engine commands.
  - No camera RAW.
- Upstream repos: https://github.com/storytold/lightcraft, https://github.com/storytold/photocraft
  (docs in `docs/`, e.g. `docs/control-protocol.md`).

## Build and run

Not set up yet. Planned: one Cargo project, `cargo run` for a dev build, `cargo test`, `cargo
clippy` (a linter, like SpotBugs/Checkstyle in one) and `cargo fmt` before each commit. Fill this
in at M1.

**Toolchain** (per Windows user, shared by all Rust projects):
- `rustup` manages Rust like SDKMAN manages JDKs. It lives in `%USERPROFILE%\.rustup`, with
  `cargo`, `rustc` and the tools in `%USERPROFILE%\.cargo\bin`, which is on the user PATH.
- `rustup update` upgrades to the latest stable.
- A shell started before the install may not see `cargo` yet. Prepend `%USERPROFILE%\.cargo\bin`
  to `PATH` for that session.
- Linking uses MSVC from VS 2022 Build Tools (C++ workload, Windows SDK). Without it, builds
  fail at the link step.
