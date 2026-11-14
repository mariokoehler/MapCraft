//! Dev probe (M2): does a mapped drive's UNC target resolve and is it readable?
//! Usage: cargo run --example unc_check -- <path on a mapped drive>
//! Prints only yes/no and the target's shape, never the server or share name.

fn main() {
    let path = std::path::PathBuf::from(std::env::args().nth(1).expect("path"));
    match mapcraft::paths::unc_target(&path) {
        Some(unc) => {
            let readable = std::fs::read_dir(&unc).is_ok();
            let shape = unc.starts_with(r"\\") && unc.matches('\\').count() >= 3;
            println!(
                "resolves: yes; looks like \\\\server\\share\\…: {shape}; readable: {readable}"
            );
        }
        None => println!("resolves: no (not a mapped network drive)"),
    }
}
