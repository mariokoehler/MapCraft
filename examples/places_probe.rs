//! Dev probe (M5): build the place index from downloaded GeoNames files and time it.
//! Usage: cargo run --release --example places_probe -- <cities500.zip> <admin1CodesASCII.txt>
//!        <countryInfo.txt> <out.sqlite> [query…]

use std::fs::File;
use std::io::BufReader;
use std::time::Instant;

use mapcraft::places::{self, Places};

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let out = std::path::Path::new(&a[4]);
    let t = Instant::now();
    let cities =
        places::unzip_first(BufReader::new(File::open(&a[1]).unwrap()), 400 << 20).unwrap();
    let n = places::build(
        cities,
        File::open(&a[2]).unwrap(),
        File::open(&a[3]).unwrap(),
        out,
    )
    .unwrap();
    let size = std::fs::metadata(out).unwrap().len();
    println!(
        "{n} places in {:.1} s, index {:.1} MB",
        t.elapsed().as_secs_f64(),
        size as f64 / 1e6
    );
    let p = Places::open(out).unwrap();
    for q in &a[5..] {
        let t = Instant::now();
        let hits = p.search(q, 5);
        let labels: Vec<String> = hits.iter().map(|h| h.label(false)).collect();
        println!(
            "{q:>10}: {:5.1} ms {labels:?}",
            t.elapsed().as_secs_f64() * 1000.0
        );
    }
}
