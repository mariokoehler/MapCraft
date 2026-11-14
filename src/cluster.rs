//! Grouping photos per zoom level (design.md §2.4): hierarchical greedy clustering, the approach
//! of Mapbox's Supercluster, written for MapCraft's needs (the representative rule, a group's
//! extent, "can it split?", its members for the strip).
//!
//! Built once per set of positions, for every whole zoom level 0..=`MAX_LEVEL`, from the most
//! detailed level up: at level `z`, every group of level `z + 1` swallows the ungrouped ones
//! within `RADIUS` screen pixels of it. So groups depend only on the zoom, never on the pan,
//! and levels nest: a group at level `z` is made of whole groups of level `z + 1`.
//!
//! Positions decide the groups; a photo's thumbnail state and date only decide which photo
//! represents a group (`with_points` recomputes just that). The input is sorted by id first, so
//! the same photos always give the same groups.
//!
//! Each level is one flat `Vec<Node>`; a node's children are a contiguous range of the level
//! below (ordered that way while building), so there are no per-node allocations. In Java terms:
//! arrays of small value records with offsets, instead of a tree of objects.

use std::cmp::Reverse;
use std::collections::HashMap;
use std::sync::Arc;

use crate::import::Bounds;
use crate::library::{PhotoPoint, Thumb};

/// The most detailed level: the base map's tiles end at zoom 19 (OpenStreetMap). Photos still
/// grouped there are "at one spot" (within about 20 m) and open the strip instead of zooming.
pub const MAX_LEVEL: usize = 19;

/// Markers closer than this many screen pixels at the level's zoom merge into a group: a
/// 68 px marker plus a little room, so no two markers overlap (design.md §2.4, "about 64 px";
/// 72 is unconfirmed).
pub const RADIUS: f64 = 72.0;

/// A photo or a group at one level.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Node {
    /// Web Mercator position in 0..1 (the world's pixel position divided by its size); for a
    /// group, the photo-weighted centre of its members.
    pub x: f64,
    pub y: f64,
    /// Photos in it (1: a single photo).
    pub count: u32,
    /// The representative photo (an index into `Clusters::points`).
    pub rep: u32,
    /// min lat, min lon, max lat, max lon of the members.
    bbox: [f64; 4],
    /// Children: a range of the level below (or of `leaves` at `MAX_LEVEL`).
    first: u32,
    len: u32,
}

impl Node {
    pub fn lat_lon(&self) -> (f64, f64) {
        unmercator(self.x, self.y)
    }

    /// The area its members cover.
    pub fn bounds(&self) -> Bounds {
        let [min_lat, min_lon, max_lat, max_lon] = self.bbox;
        Bounds {
            min_lat,
            min_lon,
            max_lat,
            max_lon,
        }
    }
}

/// Every level's groups for one snapshot of the photos.
pub struct Clusters {
    /// The snapshot this was built from, sorted by id: node indices refer to it.
    points: Arc<Vec<PhotoPoint>>,
    /// Point indices, ordered so each `MAX_LEVEL` node's members are a range.
    leaves: Vec<u32>,
    /// `levels[z]`: the nodes at zoom level `z`.
    levels: Vec<Vec<Node>>,
}

/// Web Mercator in 0..1 (x east, y south).
pub fn mercator(lat: f64, lon: f64) -> (f64, f64) {
    let lat = lat.clamp(-85.051_128_78, 85.051_128_78).to_radians();
    let x = (lon + 180.0) / 360.0;
    let y = (1.0 - (lat.tan() + 1.0 / lat.cos()).ln() / std::f64::consts::PI) / 2.0;
    (x, y)
}

pub fn unmercator(x: f64, y: f64) -> (f64, f64) {
    let lon = x * 360.0 - 180.0;
    let lat = (std::f64::consts::PI * (1.0 - 2.0 * y))
        .sinh()
        .atan()
        .to_degrees();
    (lat, lon)
}

/// How well a photo represents its group, higher is better (design.md §2.4, unconfirmed): one
/// with a made thumbnail first, then the newest (undated photos count as oldest), then the
/// lowest id.
fn rank(p: &PhotoPoint) -> (bool, i64, Reverse<i64>) {
    (p.thumb == Thumb::Ok, p.taken, Reverse(p.id))
}

impl Clusters {
    pub fn build(mut points: Vec<PhotoPoint>) -> Self {
        points.sort_unstable_by_key(|p| p.id);
        let points = Arc::new(points);
        // The leaf level: one node per photo.
        let mut cur: Vec<Node> = points
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let (x, y) = mercator(p.lat, p.lon);
                Node {
                    x,
                    y,
                    count: 1,
                    rep: i as u32,
                    bbox: [p.lat, p.lon, p.lat, p.lon],
                    first: i as u32,
                    len: 0,
                }
            })
            .collect();
        let mut leaves: Vec<u32> = (0..points.len() as u32).collect();
        let mut levels: Vec<Vec<Node>> = vec![Vec::new(); MAX_LEVEL + 1];

        for z in (0..=MAX_LEVEL).rev() {
            let r = RADIUS / (256.0 * 2f64.powi(z as i32));
            let (mut next, mut order) = merge(&cur, r);
            // Merge the new nodes again until a pass merges nothing. One pass only keeps what
            // lies within `r` of each group's first member, so neighbouring centres can end up
            // half of `r` apart (measured: a third of the markers overlapped at zooms 11-14).
            // At the fixed point, no two centres are within `r`.
            for _ in 0..32 {
                let (again, order2) = merge(&next, r);
                if again.len() == next.len() {
                    break;
                }
                // Re-base the merged nodes' children onto the level below.
                let mut flat = Vec::with_capacity(order.len());
                let mut rebased = again;
                for b in &mut rebased {
                    let start = flat.len() as u32;
                    for &a in &order2[b.first as usize..(b.first + b.len) as usize] {
                        let a = next[a as usize];
                        flat.extend_from_slice(
                            &order[a.first as usize..(a.first + a.len) as usize],
                        );
                    }
                    b.first = start;
                    b.len = flat.len() as u32 - start;
                }
                next = rebased;
                order = flat;
            }
            // Reorder the level below so each new node's children are a range of it.
            if z == MAX_LEVEL {
                leaves = order.iter().map(|&j| leaves[j as usize]).collect();
            } else {
                levels[z + 1] = order.iter().map(|&j| cur[j as usize]).collect();
            }
            cur = next;
        }
        levels[0] = cur;
        let mut c = Self {
            points,
            leaves,
            levels,
        };
        c.choose_reps();
        c
    }

    /// The same groups for a new snapshot of the same positions (thumbnail states or dates
    /// changed): only the representatives are chosen again.
    pub fn with_points(&self, mut points: Vec<PhotoPoint>) -> Self {
        points.sort_unstable_by_key(|p| p.id);
        let mut c = Self {
            points: Arc::new(points),
            leaves: self.leaves.clone(),
            levels: self.levels.clone(),
        };
        c.choose_reps();
        c
    }

    /// True when `points` has exactly this snapshot's ids and positions (in any order).
    pub fn same_positions(&self, points: &[PhotoPoint]) -> bool {
        if points.len() != self.points.len() {
            return false;
        }
        let mut sorted: Vec<(i64, f64, f64)> =
            points.iter().map(|p| (p.id, p.lat, p.lon)).collect();
        sorted.sort_unstable_by_key(|t| t.0);
        sorted
            .iter()
            .zip(self.points.iter())
            .all(|(a, b)| a.0 == b.id && a.1 == b.lat && a.2 == b.lon)
    }

    fn choose_reps(&mut self) {
        let points = &self.points;
        let best = |a: u32, b: u32| {
            if rank(&points[b as usize]) > rank(&points[a as usize]) {
                b
            } else {
                a
            }
        };
        for n in &mut self.levels[MAX_LEVEL] {
            let r = n.first as usize..(n.first + n.len) as usize;
            n.rep = self.leaves[r]
                .iter()
                .copied()
                .reduce(best)
                .expect("a node has members");
        }
        for z in (0..MAX_LEVEL).rev() {
            let (upper, lower) = self.levels.split_at_mut(z + 1);
            for n in &mut upper[z] {
                let r = n.first as usize..(n.first + n.len) as usize;
                n.rep = lower[0][r]
                    .iter()
                    .map(|c| c.rep)
                    .reduce(best)
                    .expect("children");
            }
        }
    }

    pub fn points(&self) -> &[PhotoPoint] {
        &self.points
    }

    /// The level used at map zoom `zoom`, and its nodes.
    pub fn level(&self, zoom: f64) -> (usize, &[Node]) {
        let z = (zoom.floor().max(0.0) as usize).min(MAX_LEVEL);
        (z, &self.levels[z])
    }

    /// The first level at which node `i` of level `z` falls apart into more than one marker;
    /// `None` if it never does (photos at one spot: the strip).
    pub fn split_level(&self, z: usize, i: usize) -> Option<usize> {
        let (mut z, mut n) = (z, self.levels[z][i]);
        loop {
            if n.count == 1 {
                return None; // a single photo doesn't split
            }
            if z == MAX_LEVEL {
                return None;
            }
            if n.len > 1 {
                return Some(z + 1);
            }
            z += 1;
            n = self.levels[z][n.first as usize];
        }
    }

    /// The photos in node `i` of level `z` (indices into `points`).
    pub fn members(&self, z: usize, i: usize) -> Vec<u32> {
        // Each node's children are a range, but neighbouring nodes' ranges needn't be adjacent
        // (a level is reordered for the level above it), so walk node by node.
        let mut out = Vec::with_capacity(self.levels[z][i].count as usize);
        let mut stack = vec![(z, i)];
        while let Some((z, i)) = stack.pop() {
            let n = self.levels[z][i];
            let r = n.first as usize..(n.first + n.len) as usize;
            if z == MAX_LEVEL {
                out.extend_from_slice(&self.leaves[r]);
            } else {
                stack.extend(r.map(|j| (z + 1, j)));
            }
        }
        out
    }
}

/// One greedy pass: the nodes of a level from the nodes of the level below, merging those within
/// `r` (in 0..1 world units). Returns the new nodes and the order of the old ones (each new
/// node's children are a range of that order).
fn merge(cur: &[Node], r: f64) -> (Vec<Node>, Vec<u32>) {
    let cell = |x: f64, y: f64| ((x / r).floor() as i64, (y / r).floor() as i64);
    let mut grid: HashMap<(i64, i64), Vec<u32>> = HashMap::with_capacity(cur.len());
    for (i, n) in cur.iter().enumerate() {
        grid.entry(cell(n.x, n.y)).or_default().push(i as u32);
    }
    let mut taken = vec![false; cur.len()];
    let mut next = Vec::new();
    let mut order: Vec<u32> = Vec::with_capacity(cur.len());
    let r2 = r * r;
    for i in 0..cur.len() {
        if taken[i] {
            continue;
        }
        taken[i] = true;
        let a = cur[i];
        let first = order.len() as u32;
        order.push(i as u32);
        let (cx, cy) = cell(a.x, a.y);
        for dx in -1..=1 {
            for dy in -1..=1 {
                let Some(list) = grid.get(&(cx + dx, cy + dy)) else {
                    continue;
                };
                for &j in list {
                    let b = cur[j as usize];
                    if !taken[j as usize] && (b.x - a.x).powi(2) + (b.y - a.y).powi(2) <= r2 {
                        taken[j as usize] = true;
                        order.push(j);
                    }
                }
            }
        }
        let kids = &order[first as usize..];
        let mut node = Node {
            x: 0.0,
            y: 0.0,
            count: 0,
            rep: a.rep,
            bbox: a.bbox,
            first,
            len: kids.len() as u32,
        };
        for &k in kids {
            let c = cur[k as usize];
            let w = c.count as f64;
            node.x += c.x * w;
            node.y += c.y * w;
            node.count += c.count;
            node.bbox = [
                node.bbox[0].min(c.bbox[0]),
                node.bbox[1].min(c.bbox[1]),
                node.bbox[2].max(c.bbox[2]),
                node.bbox[3].max(c.bbox[3]),
            ];
        }
        node.x /= node.count as f64;
        node.y /= node.count as f64;
        next.push(node);
    }
    (next, order)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pt(id: i64, lat: f64, lon: f64) -> PhotoPoint {
        PhotoPoint {
            id,
            lat,
            lon,
            thumb: Thumb::Ok,
            taken: 0,
        }
    }

    /// A scatter over a few cities, with a pile of photos at one spot.
    fn sample() -> Vec<PhotoPoint> {
        let mut seed: u64 = 7;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % 100_000) as f64 / 100_000.0
        };
        let cities = [
            (64.14, -21.94),
            (38.72, -9.14),
            (53.55, 9.99),
            (48.21, 16.37),
        ];
        let mut v = Vec::new();
        for id in 0..2_000 {
            let (lat, lon) = cities[id % cities.len()];
            v.push(pt(
                id as i64,
                lat + (next() - 0.5) * 0.5,
                lon + (next() - 0.5) * 0.8,
            ));
        }
        for id in 2_000..2_010 {
            v.push(pt(id, 53.543_6, 9.989_3)); // ten photos at one spot
        }
        v
    }

    #[test]
    fn mercator_round_trips() {
        for (lat, lon) in [
            (0.0, 0.0),
            (53.5436, 9.9893),
            (-33.86, 151.2),
            (64.1, -21.9),
        ] {
            let (x, y) = mercator(lat, lon);
            let (a, o) = unmercator(x, y);
            assert!((a - lat).abs() < 1e-9 && (o - lon).abs() < 1e-9);
        }
    }

    #[test]
    fn every_level_holds_every_photo_and_levels_nest() {
        let c = Clusters::build(sample());
        let n = c.points().len();
        for z in 0..=MAX_LEVEL {
            let nodes = &c.levels[z];
            let total: u32 = nodes.iter().map(|n| n.count).sum();
            assert_eq!(total as usize, n, "level {z}");
            // Each node's members are its count, and members are distinct across the level.
            let mut seen = vec![false; n];
            for (i, node) in nodes.iter().enumerate() {
                let m = c.members(z, i);
                assert_eq!(m.len(), node.count as usize);
                for p in m {
                    assert!(!seen[p as usize]);
                    seen[p as usize] = true;
                }
            }
            // Fewer (or as many) groups further out.
            if z > 0 {
                assert!(c.levels[z - 1].len() <= nodes.len());
            }
        }
        assert!(c.levels[0].len() < 10, "the whole world is a few groups");
        assert!(
            c.levels[MAX_LEVEL].len() > 1_900,
            "street level splits nearly all"
        );
    }

    #[test]
    fn no_two_markers_overlap_at_any_level() {
        let c = Clusters::build(sample());
        for (z, nodes) in c.levels.iter().enumerate() {
            let scale = 256.0 * 2f64.powi(z as i32);
            for (i, a) in nodes.iter().enumerate() {
                for b in &nodes[i + 1..] {
                    let d = ((a.x - b.x) * scale).hypot((a.y - b.y) * scale);
                    assert!(d > RADIUS, "level {z}: two markers {d:.0} px apart");
                }
            }
        }
    }

    #[test]
    fn the_order_of_the_input_doesnt_matter() {
        let a = Clusters::build(sample());
        let mut shuffled = sample();
        shuffled.reverse();
        shuffled.swap(3, 1_500);
        let b = Clusters::build(shuffled);
        assert_eq!(a.levels, b.levels);
        assert_eq!(a.leaves, b.leaves);
    }

    #[test]
    fn photos_at_one_spot_never_split_and_others_do() {
        let c = Clusters::build(sample());
        let spot: Vec<i64> = (2_000..2_010).collect();
        let (z, nodes) = c.level(25.0);
        assert_eq!(z, MAX_LEVEL);
        let i = nodes
            .iter()
            .position(|n| spot.contains(&c.points()[n.rep as usize].id))
            .unwrap();
        assert_eq!(nodes[i].count, 10);
        assert_eq!(c.split_level(z, i), None, "the strip, not a zoom");
        // At city level the first group splits further in.
        let (z, nodes) = c.level(8.0);
        let big = nodes.iter().position(|n| n.count > 20).unwrap();
        let s = c.split_level(z, big).unwrap();
        assert!(s > z && s <= MAX_LEVEL);
        let parts = c.levels[s]
            .iter()
            .enumerate()
            .filter(|(j, _)| {
                let m = c.members(s, *j);
                c.members(z, big).contains(&m[0])
            })
            .count();
        assert!(parts > 1, "it really falls apart at {s}");
        // A single photo doesn't split either.
        let single = c.levels[MAX_LEVEL]
            .iter()
            .position(|n| n.count == 1)
            .unwrap();
        assert_eq!(c.split_level(MAX_LEVEL, single), None);
    }

    #[test]
    fn the_representative_is_the_newest_with_a_thumbnail() {
        let mut v = vec![
            pt(1, 53.5436, 9.9893),
            pt(2, 53.5436, 9.9893),
            pt(3, 53.5436, 9.9893),
            pt(4, 53.5436, 9.9893),
        ];
        v[0].taken = 20230519120000;
        v[1].taken = 20240101000000; // newest, but no thumbnail yet
        v[1].thumb = Thumb::Pending;
        v[2].taken = 20230803194200; // newest with a thumbnail
        v[3].taken = 0; // undated
        let c = Clusters::build(v.clone());
        let (_, nodes) = c.level(10.0);
        assert_eq!(nodes.len(), 1);
        assert_eq!(c.points()[nodes[0].rep as usize].id, 3);
        // Once 2's thumbnail is made, it represents the group; the groups stay as they were.
        v[1].thumb = Thumb::Ok;
        assert!(c.same_positions(&v));
        let d = c.with_points(v.clone());
        assert_eq!(d.points()[d.level(10.0).1[0].rep as usize].id, 2);
        // A moved photo is a different set of positions.
        v[0].lat += 0.01;
        assert!(!c.same_positions(&v));
    }
}
