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
    // How close do neighbouring markers get on screen, per level (at that whole zoom)?
    let mut worst = Vec::new();
    for z in 0..=mapcraft::cluster::MAX_LEVEL {
        let (_, nodes) = c.level(z as f64);
        let scale = 256.0 * 2f64.powi(z as i32);
        let mut grid: std::collections::HashMap<(i64, i64), Vec<(f64, f64)>> = Default::default();
        for n in nodes {
            let (x, y) = (n.x * scale, n.y * scale);
            grid.entry(((x / 100.0) as i64, (y / 100.0) as i64))
                .or_default()
                .push((x, y));
        }
        let mut min = f64::MAX;
        for (&(cx, cy), list) in &grid {
            for &(x, y) in list {
                for dx in -1..=1 {
                    for dy in -1..=1 {
                        for &(x2, y2) in grid.get(&(cx + dx, cy + dy)).into_iter().flatten() {
                            let d = (x - x2).hypot(y - y2);
                            if d > 0.0 && d < min {
                                min = d;
                            }
                        }
                    }
                }
            }
        }
        let under = |lim: f64| {
            nodes.len() // count nodes having a neighbour closer than lim
                - nodes
                    .iter()
                    .filter(|n| {
                        let (x, y) = (n.x * scale, n.y * scale);
                        let (cx, cy) = ((x / 100.0) as i64, (y / 100.0) as i64);
                        !(-1..=1).any(|dx| {
                            (-1..=1).any(|dy| {
                                grid.get(&(cx + dx, cy + dy)).into_iter().flatten().any(|&(x2, y2)| {
                                    let d = (x - x2).hypot(y - y2);
                                    d > 0.0 && d < lim
                                })
                            })
                        })
                    })
                    .count()
        };
        worst.push(format!(
            "z{z}: min {:.0} px, {} of {} under 68 px",
            if min == f64::MAX { 0.0 } else { min },
            under(68.0),
            nodes.len()
        ));
    }
    for w in worst {
        println!("{w}");
    }
}
