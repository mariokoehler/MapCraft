# MapCraft

A Windows desktop app, written in Rust, that shows your photos on a world map, like the Map module
in Lightroom Classic.

- Photos stay where they are, on local drives or SMB shares. MapCraft only records their paths
  and keeps a small thumbnail of each.
- Photos are placed by their EXIF GPS position and merge into groups that split as you zoom in.
- RAW files and other formats it doesn't decode itself are handed to the companion apps
  [LightCraft](https://github.com/storytold/lightcraft) and
  [PhotoCraft](https://github.com/storytold/photocraft).

**Status:** early development. Milestone 1 is done: a window with the OpenStreetMap map. Photos
aren't imported yet. See [`design.md`](design.md) for the concept, features, architecture and
roadmap.

## Building

You need Rust (via [rustup](https://rustup.rs)) and, on Windows, the Visual Studio Build Tools
with the C++ workload. Then `cargo run`.

MapCraft is a personal, non-commercial project. It is **not affiliated** with the storytold /
ArtCraft team that makes LightCraft and PhotoCraft.

## Licence

Licensed under either of

- Apache License, Version 2.0 ([`LICENSE-APACHE`](LICENSE-APACHE) or
  <https://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([`LICENSE-MIT`](LICENSE-MIT) or <https://opensource.org/licenses/MIT>)

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in
the work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any
additional terms or conditions.

## Third-party material

- Map data © OpenStreetMap contributors (ODbL). Place names from GeoNames (CC-BY 4.0), once
  place search is built.
- [IBM Plex](https://github.com/IBM/plex) Sans and Mono fonts, © IBM Corp., under the SIL Open Font
  License 1.1 ([`assets/fonts/OFL.txt`](assets/fonts/OFL.txt)).
- [Lucide](https://lucide.dev) icons, © Lucide Icons and Contributors, under the ISC licence
  ([`assets/icons/LICENSE`](assets/icons/LICENSE)).
