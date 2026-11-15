//! Build script (runs before the crate compiles, like a Maven plugin bound to an early phase):
//! puts the icon and the version info into `mapcraft.exe` (design.md M7).

fn main() {
    const ICON: &str = "assets/brand/mapcraft.ico";
    println!("cargo:rerun-if-changed={ICON}");
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut res = winresource::WindowsResource::new();
    // Until the icon exists, the exe gets the version info only.
    if std::path::Path::new(ICON).exists() {
        res.set_icon(ICON);
    }
    res.set("FileDescription", "MapCraft")
        .set("ProductName", "MapCraft")
        .set("LegalCopyright", "MIT OR Apache-2.0");
    if let Err(e) = res.compile() {
        panic!("can't embed the Windows resources: {e}");
    }
}
