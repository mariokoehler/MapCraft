//! Dev probe (M4): how long does grouping take? Synthetic photos only (no library is read).
//! Usage: cargo run --release --example cluster_probe -- [count]

use std::time::Instant;

use mapcraft::cluster::Clusters;
use mapcraft::library::{PhotoPoint, Thumb};

fn main() {
    let n: usize = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(100_000);
    // Like `--fill-library`: trips around 200 places in Europe, photos scattered around each.
    let mut seed: u64 = 0x5eed;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed % 1_000_000) as f64 / 1_000_000.0
    };
    let places: Vec<(f64, f64)> = (0..200)
        .map(|_| (36.0 + next() * 30.0, -10.0 + next() * 40.0))
        .collect();
    let points: Vec<PhotoPoint> = (0..n)
        .map(|i| {
            let (lat, lon) = places[i % places.len()];
            PhotoPoint {
                id: i as i64,
                lat: lat + (next() - 0.5) * 0.05,
                lon: lon + (next() - 0.5) * 0.08,
                thumb: Thumb::Ok,
                taken: 20_200_101_000_000 + i as i64,
            }
        })
        .collect();
    let t = Instant::now();
    let c = Clusters::build(points.clone());
    let build = t.elapsed();
    let t = Instant::now();
    let _ = c.same_positions(&points) && c.with_points(points).points().len() == n;
    let reps = t.elapsed();
    let nodes: Vec<usize> = [0.0, 4.0, 8.0, 12.0, 16.0, 19.0]
        .iter()
        .map(|&z| c.level(z).1.len())
        .collect();
    println!("{n} photos: build {build:?}, same-positions check + new representatives {reps:?}");
    println!("nodes at zoom 0/4/8/12/16/19: {nodes:?}");
}
