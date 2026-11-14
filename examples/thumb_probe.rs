//! Dev probe (M3): how fast are thumbnails, per stage and per source?
//! Usage: cargo run --release --example thumb_probe -- <library copy> <out folder> <per kind> [workers]
//! Reads originals only; writes thumbnails into <out folder>. Prints aggregates only.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::Instant;

use mapcraft::thumbs::{self, PhotoFiles, THUMB};

struct Row {
    id: i64,
    dir: PathBuf,
    original: String,
    preview: Option<String>,
    orientation: Option<u16>,
}

fn rows(db: &Path, sql: &str) -> Vec<Row> {
    let c = rusqlite::Connection::open(db).unwrap();
    let root: String = c
        .query_row("SELECT root_path FROM source LIMIT 1", [], |r| r.get(0))
        .unwrap();
    let mut s = c.prepare(sql).unwrap();
    s.query_map([], |r| {
        Ok(Row {
            id: r.get(0)?,
            dir: Path::new(&root).join(r.get::<_, String>(1)?),
            original: r.get(2)?,
            preview: r.get(3)?,
            orientation: r.get(4)?,
        })
    })
    .unwrap()
    .map(Result::unwrap)
    .collect()
}

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    if a[1] == "files" {
        // Mode 2: `files <list.txt> <out>`: render every listed file (one per line) and count
        // the sources and failure reasons per extension.
        let list = std::fs::read_to_string(&a[2]).unwrap();
        let mut stats: std::collections::BTreeMap<
            String,
            std::collections::BTreeMap<String, usize>,
        > = Default::default();
        for (i, line) in list.lines().filter(|l| !l.trim().is_empty()).enumerate() {
            let p = Path::new(line);
            let (dir, name) = (
                p.parent().unwrap(),
                p.file_name().unwrap().to_string_lossy().to_string(),
            );
            let ext = name.rsplit('.').next().unwrap_or("").to_lowercase();
            let orientation = mapcraft::metadata::read_image(p)
                .ok()
                .and_then(|m| m.orientation);
            let f = PhotoFiles {
                dir,
                original: &name,
                preview: None,
                orientation,
            };
            let key = match thumbs::render(&f, THUMB) {
                Ok((jpeg, src)) => {
                    std::fs::write(Path::new(&a[3]).join(format!("{ext}-{i}.jpg")), jpeg).unwrap();
                    format!("{src:?}")
                }
                Err(e) => format!("{e}").chars().take(60).collect(),
            };
            *stats.entry(ext).or_default().entry(key).or_insert(0) += 1;
        }
        for (ext, m) in stats {
            println!("{ext:5} {m:?}");
        }
        return;
    }
    let (db, out, per) = (
        PathBuf::from(&a[1]),
        PathBuf::from(&a[2]),
        a[3].parse::<usize>().unwrap(),
    );
    let workers: usize = a.get(4).and_then(|w| w.parse().ok()).unwrap_or(1);
    std::fs::create_dir_all(&out).unwrap();

    let base = "SELECT id, rel_dir, original_name, preview_name, orientation FROM photo \
                WHERE lat IS NOT NULL AND original_name NOT LIKE '.\\_%' ESCAPE '\\' AND ";
    let kinds = [
        (
            "cr2",
            format!(
                "{base} lower(original_name) LIKE '%.cr2' AND preview_name IS NULL ORDER BY random() LIMIT {per}"
            ),
        ),
        (
            "cr2+jpg",
            format!(
                "{base} lower(original_name) LIKE '%.cr2' AND preview_name IS NOT NULL ORDER BY random() LIMIT {per}"
            ),
        ),
        (
            "tif",
            format!("{base} lower(original_name) LIKE '%.tif' ORDER BY random() LIMIT {per}"),
        ),
        (
            "jpg",
            format!("{base} lower(original_name) LIKE '%.jpg' ORDER BY random() LIMIT {per}"),
        ),
    ];
    if workers > 1 {
        // Throughput with several workers on one kind (the CR2s).
        let list = rows(&db, &kinds[0].1);
        let t = Instant::now();
        let queue = std::sync::Mutex::new(list.iter());
        std::thread::scope(|s| {
            for _ in 0..workers {
                s.spawn(|| {
                    loop {
                        let Some(r) = queue.lock().unwrap().next() else {
                            break;
                        };
                        let f = PhotoFiles {
                            dir: &r.dir,
                            original: &r.original,
                            preview: None,
                            orientation: r.orientation,
                        };
                        let _ = thumbs::render(&f, THUMB);
                    }
                });
            }
        });
        println!(
            "{workers} workers: {} CR2s in {:.1} s = {:.0} ms each",
            list.len(),
            t.elapsed().as_secs_f64(),
            ms(t) / list.len() as f64
        );
        return;
    }
    for (kind, sql) in &kinds {
        let list = rows(&db, sql);
        if list.is_empty() {
            continue;
        }
        let (mut find, mut read, mut dec, mut fitenc, mut total, mut fails, mut kb) =
            (0.0, 0.0, 0.0, 0.0, 0.0, 0, 0u64);
        let mut sources = std::collections::BTreeMap::new();
        for r in &list {
            let t0 = Instant::now();
            let files = PhotoFiles {
                dir: &r.dir,
                original: &r.original,
                preview: r.preview.as_deref(),
                orientation: r.orientation,
            };
            // Stage timings for the embedded-preview path (CR2); other kinds only get totals.
            if kind.starts_with("cr2") {
                let t = Instant::now();
                let mut f = File::open(r.dir.join(&r.original)).unwrap();
                let p = mapcraft::metadata::embedded_previews(&mut f).unwrap();
                find += ms(t);
                let best = p
                    .iter()
                    .filter(|e| e.width.max(e.height) >= THUMB)
                    .min_by_key(|e| e.width.max(e.height))
                    .copied();
                if let Some(e) = best {
                    let t = Instant::now();
                    let mut buf = vec![0; e.len as usize];
                    f.seek(SeekFrom::Start(e.offset)).unwrap();
                    f.read_exact(&mut buf).unwrap();
                    read += ms(t);
                    kb += e.len / 1024;
                    let t = Instant::now();
                    let img = thumbs::decode_jpeg(&buf, THUMB).unwrap();
                    dec += ms(t);
                    let t = Instant::now();
                    let _ = thumbs::encode(&thumbs::fit(thumbs::orient(img, r.orientation), THUMB));
                    fitenc += ms(t);
                }
            }
            match thumbs::render(&files, THUMB) {
                Ok((jpeg, src)) => {
                    *sources.entry(format!("{src:?}")).or_insert(0) += 1;
                    std::fs::write(
                        out.join(format!(
                            "{kind}-{}-o{}.jpg",
                            r.id,
                            r.orientation.unwrap_or(1)
                        )),
                        jpeg,
                    )
                    .unwrap();
                }
                Err(e) => {
                    fails += 1;
                    println!(
                        "  {kind} failure: {}",
                        format!("{e}").chars().take(70).collect::<String>()
                    );
                }
            }
            total += ms(t0);
        }
        let n = list.len() as f64;
        println!(
            "{kind:8} n={:3} total {:6.0} ms/photo | find {:4.0} read {:4.0} ({:5.0} KB) decode {:4.0} fit+encode {:3.0} | sources {sources:?} fails {fails}",
            list.len(),
            total / n,
            find / n,
            read / n,
            kb as f64 / n,
            dec / n,
            fitenc / n
        );
    }
}
