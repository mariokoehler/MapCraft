//! Writes generated photos without a position, taken every few minutes from a start time, for
//! trying geotagging on a scratch folder (design.md §2.9):
//! `cargo run --example geotag_demo -- <folder> <first YYYY:MM:DD HH:MM:SS> <minutes apart> <n>`

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let [folder, first, step, n] = a.as_slice() else {
        eprintln!("usage: geotag_demo <folder> <first YYYY:MM:DD HH:MM:SS> <minutes apart> <n>");
        std::process::exit(2);
    };
    let (step, n): (i64, usize) = (step.parse().expect("minutes"), n.parse().expect("count"));
    let t0 = mapcraft::geotag::time::parse(&first.replacen(':', "-", 2))
        .expect("a start time")
        .0;
    std::fs::create_dir_all(folder).expect("the folder");
    for i in 0..n {
        let t = t0 + (i as i64 * step * 60) as f64;
        let stamp = mapcraft::geotag::time::format_utc(t);
        let exif = format!("{} {}", stamp[..10].replace('-', ":"), &stamp[11..19]);
        let jpeg = mapcraft::demo::photo_with(0.0, 0.0, &exif, i as f64 / n as f64, None);
        let path = std::path::Path::new(folder).join(format!("WALK_{i:03}.jpg"));
        std::fs::write(path, jpeg).expect("a photo");
    }
    println!("{n} photos written");
}
