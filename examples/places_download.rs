//! Dev probe (M5): run the real place-name download into a scratch data folder, then search it.
//! Usage: cargo run --release --example places_download -- <scratch data folder> [query…]
//! Downloads public GeoNames data only (about 14 MB); nothing about the user is sent.

use std::time::Instant;

use mapcraft::places::{self, Places};

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let data = std::path::Path::new(&a[1]);
    let t = Instant::now();
    let last = std::cell::Cell::new(0u64);
    let n = places::download(data, &|done, total| {
        if done >> 22 != last.get() >> 22 {
            println!("  {} / {} MB", done >> 20, total >> 20);
        }
        last.set(done);
    })
    .expect("download");
    println!("{n} places in {:.1} s", t.elapsed().as_secs_f64());
    let p = Places::open(&places::index_path(data)).expect("the index opens");
    for q in &a[2..] {
        let labels: Vec<String> = p.search(q, 5).iter().map(|h| h.label(false)).collect();
        println!("{q:>10}: {labels:?}");
    }
}
