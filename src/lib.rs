//! MapCraft as a library: everything except `main`. The `mapcraft` binary (`main.rs`), the tests
//! and the dev probes in `examples/` all use it.

pub mod app;
pub mod cluster;
pub mod companions;
pub mod datafolder;
pub mod demo;
pub mod filter;
pub mod geotag;
pub mod import;
pub mod jobs;
pub mod launch;
pub mod library;
pub mod metadata;
pub mod paths;
pub mod places;
pub mod provider;
pub mod search;
pub mod settings;
pub mod theme;
pub mod thumbs;
pub mod ui;
pub mod update;
