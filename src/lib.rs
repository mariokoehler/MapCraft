//! MapCraft as a library: everything except `main`. The `mapcraft` binary (`main.rs`), the tests
//! and the dev probes in `examples/` all use it.

pub mod app;
pub mod import;
pub mod library;
pub mod metadata;
pub mod paths;
pub mod provider;
pub mod settings;
pub mod theme;
pub mod ui;
