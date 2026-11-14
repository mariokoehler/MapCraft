//! Import tests on generated files in a temp folder (never real photos).

use std::fs;
use std::path::Path;
use std::sync::atomic::AtomicBool;

use super::*;
use crate::demo::{jpeg_with_gps, tiff_with_gps};

const XMP_GPS: &str = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
<rdf:Description xmlns:exif="http://ns.adobe.com/exif/1.0/" exif:GPSLatitude="48,8.4N" exif:GPSLongitude="2,21E"/>
</rdf:RDF></x:xmpmeta>"#;

/// The test library: a folder with photos of every interesting kind.
fn setup(root: &Path) {
    fs::create_dir_all(root.join("trip")).unwrap();
    fs::create_dir_all(root.join("trip").join(".dtrash")).unwrap();
    // South and west, in EXIF.
    fs::write(
        root.join("trip").join("A.jpg"),
        jpeg_with_gps(-33.8688, -70.6, "2023:05:19 21:06:14"),
    )
    .unwrap();
    // A "RAW" (TIFF structure, no GPS) with a JPEG beside it and a sidecar with the position.
    fs::write(
        root.join("trip").join("B.dng"),
        tiff_with_gps(0.0, 0.0, "2022:01:02 03:04:05"),
    )
    .unwrap();
    fs::write(
        root.join("trip").join("B.jpg"),
        jpeg_with_gps(10.0, 10.0, "2022:01:02 03:04:05"),
    )
    .unwrap();
    fs::write(root.join("trip").join("B.xmp"), XMP_GPS).unwrap();
    // No metadata at all; a corrupt TIFF; a video; something in the trash.
    fs::write(root.join("trip").join("C.png"), b"\x89PNG\r\n\x1a\n").unwrap();
    fs::write(root.join("trip").join("D.tif"), b"II*\0\xff\xff\xff\x7f").unwrap();
    fs::write(root.join("trip").join("clip.mov"), b"x").unwrap();
    fs::write(
        root.join("trip").join(".dtrash").join("E.jpg"),
        jpeg_with_gps(1.0, 1.0, "2020:01:01 00:00:00"),
    )
    .unwrap();
}

fn import(lib: &mut Library, src: i64) -> Summary {
    run(lib, src, &AtomicBool::new(false), 4, |_| {}).unwrap()
}

fn photo(lib: &Library, stem: &str) -> Option<library::Existing> {
    let src = lib.sources().unwrap()[0].id;
    lib.existing(src)
        .unwrap()
        .into_iter()
        .find(|((_, s), _)| s == &stem.to_lowercase())
        .map(|(_, e)| e)
}

#[test]
fn fixture_is_valid_exif_for_another_reader() {
    // Cross-check the hand-made EXIF with kamadak-exif, so the fixtures don't only agree with
    // the reader they test.
    let jpeg = jpeg_with_gps(-33.8688, -70.6, "2023:05:19 21:06:14");
    let exif = exif::Reader::new()
        .read_from_container(&mut std::io::Cursor::new(jpeg))
        .unwrap();
    let r = exif
        .get_field(exif::Tag::GPSLatitudeRef, exif::In::PRIMARY)
        .unwrap();
    assert_eq!(r.display_value().to_string(), "S");
    let d = exif
        .get_field(exif::Tag::DateTimeOriginal, exif::In::PRIMARY)
        .unwrap();
    assert_eq!(d.display_value().to_string(), "2023-05-19 21:06:14");
}

#[test]
fn first_import_reads_positions_pairs_and_sidecars() {
    let dir = tempfile::tempdir().unwrap();
    setup(dir.path());
    let mut lib = Library::open_in_memory().unwrap();
    let src = lib
        .add_source(dir.path(), None, "test", &scope::Scope::all())
        .unwrap();
    let s = import(&mut lib, src);
    // A, B (dng + jpg + xmp), C, D; not the video, not the trash.
    assert_eq!((s.photos, s.new, s.failed), (4, 4, 1), "{s:?}");
    assert_eq!(s.without_position, 2, "C and D have none");

    let points = lib.positions().unwrap();
    assert_eq!(points.len(), 2);
    let a = points.iter().find(|p| p.lat < 0.0).unwrap();
    assert!((a.lat - -33.8688).abs() < 1e-6 && (a.lon - -70.6).abs() < 1e-6);
    // The sidecar wins over the preview JPEG's EXIF (10, 10).
    let b = points.iter().find(|p| p.lat > 0.0).unwrap();
    assert!((b.lat - 48.14).abs() < 1e-9 && (b.lon - 2.35).abs() < 1e-9);
    assert_eq!(lib.counts().unwrap().with_position, 2);
}

#[test]
fn rescan_skips_unchanged_and_finds_moves_and_missing() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    setup(root);
    let mut lib = Library::open_in_memory().unwrap();
    let src = lib
        .add_source(root, None, "test", &scope::Scope::all())
        .unwrap();
    import(&mut lib, src);
    let a_id = photo(&lib, "A").unwrap().id;

    let again = import(&mut lib, src);
    // D's metadata is unreadable, so it is read again on every rescan (it might be a bug fixed
    // in a later version), but still counts as unchanged; the toast notes it as unreadable.
    assert_eq!(
        (again.unchanged, again.new, again.changed, again.failed),
        (4, 0, 0, 1),
        "{again:?}"
    );

    // Move A to another folder under a new name; delete C.
    fs::create_dir_all(root.join("sorted")).unwrap();
    fs::rename(
        root.join("trip").join("A.jpg"),
        root.join("sorted").join("A_renamed.jpg"),
    )
    .unwrap();
    fs::remove_file(root.join("trip").join("C.png")).unwrap();
    let s = import(&mut lib, src);
    assert_eq!((s.moved, s.missing, s.new), (1, 1, 0), "{s:?}");
    assert_eq!(
        photo(&lib, "A_renamed").unwrap().id,
        a_id,
        "a moved photo keeps its record"
    );
    assert!(photo(&lib, "C").unwrap().missing);

    // C comes back.
    fs::write(root.join("trip").join("C.png"), b"\x89PNG\r\n\x1a\n").unwrap();
    let s = import(&mut lib, src);
    assert_eq!(s.restored, 1, "{s:?}");
    assert!(!photo(&lib, "C").unwrap().missing);
}

#[test]
fn a_renamed_folder_moves_its_photos() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    setup(root);
    let mut lib = Library::open_in_memory().unwrap();
    let src = lib
        .add_source(root, None, "test", &scope::Scope::all())
        .unwrap();
    import(&mut lib, src);
    let before: Vec<i64> = ["A", "B", "C", "D"]
        .map(|s| photo(&lib, s).unwrap().id)
        .to_vec();
    let positions = lib.positions().unwrap().len();

    // The classic: `2023-05-19_Trip` renamed to `2023-05-19_Lisbon`.
    fs::rename(root.join("trip"), root.join("lisbon")).unwrap();
    let s = import(&mut lib, src);
    assert_eq!((s.moved, s.new, s.missing), (4, 0, 0), "{s:?}");
    let after: Vec<i64> = ["A", "B", "C", "D"]
        .map(|s| photo(&lib, s).unwrap().id)
        .to_vec();
    assert_eq!(before, after, "every photo keeps its record");
    assert_eq!(lib.counts().unwrap().photos, 4, "no duplicates");
    assert_eq!(lib.positions().unwrap().len(), positions);
}

#[test]
fn a_deleted_folder_makes_its_photos_missing() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    setup(root);
    let mut lib = Library::open_in_memory().unwrap();
    let src = lib
        .add_source(root, None, "test", &scope::Scope::all())
        .unwrap();
    import(&mut lib, src);
    fs::remove_dir_all(root.join("trip")).unwrap();
    let s = import(&mut lib, src);
    assert_eq!(s.missing, 4, "{s:?}");
    assert!(
        lib.positions().unwrap().is_empty(),
        "missing photos leave the map"
    );
    assert_eq!(lib.counts().unwrap().photos, 4, "but their records stay");
}

/// Opens a file with no sharing: until it's dropped, every other open fails (Windows).
fn lock(path: &Path) -> fs::File {
    use std::os::windows::fs::OpenOptionsExt;
    fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(path)
        .unwrap()
}

#[test]
fn a_locked_file_is_skipped_and_tried_again() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    setup(root);
    let mut lib = Library::open_in_memory().unwrap();
    let src = lib
        .add_source(root, None, "test", &scope::Scope::all())
        .unwrap();
    let guard = lock(&root.join("trip").join("A.jpg"));
    let s = import(&mut lib, src);
    assert_eq!((s.new, s.skipped), (3, 1), "{s:?}");
    assert!(photo(&lib, "A").is_none(), "not recorded while unreadable");
    drop(guard);
    let s = import(&mut lib, src);
    assert_eq!(s.new, 1, "picked up on the next rescan: {s:?}");
    assert!(photo(&lib, "A").is_some());
}

#[test]
fn a_rescan_hiccup_keeps_the_position() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    setup(root);
    let mut lib = Library::open_in_memory().unwrap();
    let src = lib
        .add_source(root, None, "test", &scope::Scope::all())
        .unwrap();
    import(&mut lib, src);
    let positions = lib.positions().unwrap().len();
    // A changes (so it must be read again), but can't be opened during the rescan.
    let a = root.join("trip").join("A.jpg");
    fs::write(&a, jpeg_with_gps(-33.8688, -70.6, "2023:05:19 21:06:15")).unwrap();
    let guard = lock(&a);
    let s = import(&mut lib, src);
    assert_eq!(s.skipped, 1, "{s:?}");
    assert_eq!(
        lib.positions().unwrap().len(),
        positions,
        "the old position stays"
    );
    drop(guard);
    let s = import(&mut lib, src);
    assert_eq!(s.changed, 1, "A; D is re-read but unchanged: {s:?}");
}

#[test]
fn an_offline_drive_letter_falls_back_to_the_unc_path() {
    let dir = tempfile::tempdir().unwrap();
    setup(dir.path());
    let mut lib = Library::open_in_memory().unwrap();
    // The stored root no longer resolves (the drive letter isn't mapped in this session), but
    // its stored UNC target does.
    let unc = dir.path().to_string_lossy().into_owned();
    let src = lib
        .add_source(
            &dir.path().join("unmapped"),
            Some(&unc),
            "test",
            &scope::Scope::all(),
        )
        .unwrap();
    let s = import(&mut lib, src);
    assert_eq!(s.new, 4, "{s:?}");
}

#[test]
fn the_ui_can_read_while_the_import_writes() {
    let dir = tempfile::tempdir().unwrap();
    setup(dir.path());
    let db = dir.path().join("library.sqlite");
    let mut writer = Library::open(&db).unwrap();
    let reader = Library::open(&db).unwrap();
    let src = writer
        .add_source(dir.path(), None, "test", &scope::Scope::all())
        .unwrap();
    import(&mut writer, src);
    // An open write transaction (as during an import batch) doesn't block the UI's reads.
    let tx = writer.transaction().unwrap();
    tx.execute("UPDATE photo SET lat = NULL", []).unwrap();
    let started = std::time::Instant::now();
    assert_eq!(
        reader.positions().unwrap().len(),
        2,
        "sees the last committed state"
    );
    assert!(started.elapsed() < std::time::Duration::from_secs(1));
    drop(tx);
}

#[test]
fn picked_folders_limit_the_import() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    setup(root);
    fs::create_dir_all(root.join("other")).unwrap();
    fs::write(
        root.join("other").join("F.jpg"),
        jpeg_with_gps(5.0, 5.0, "2021:01:01 00:00:00"),
    )
    .unwrap();
    let mut lib = Library::open_in_memory().unwrap();
    let mut only_other = scope::Scope::all();
    only_other.set("", false);
    only_other.set("other", true);
    let src = lib.add_source(root, None, "test", &only_other).unwrap();
    let s = import(&mut lib, src);
    assert_eq!(s.photos, 1);
}

#[test]
fn leaving_a_folder_out_later_removes_its_records_without_missing_noise() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    setup(root);
    fs::create_dir_all(root.join("other")).unwrap();
    fs::write(
        root.join("other").join("F.jpg"),
        jpeg_with_gps(5.0, 5.0, "2021:01:01 00:00:00"),
    )
    .unwrap();
    let mut lib = Library::open_in_memory().unwrap();
    let src = lib
        .add_source(root, None, "test", &scope::Scope::all())
        .unwrap();
    assert_eq!(import(&mut lib, src).photos, 5);

    // "Change folders…": leave `trip` out.
    let mut narrower = scope::Scope::all();
    narrower.set("trip", false);
    assert_eq!(
        lib.set_scope(src, &narrower).unwrap(),
        4,
        "A, B, C, D removed"
    );
    let s = import(&mut lib, src);
    assert_eq!((s.photos, s.missing, s.new), (1, 0, 0), "{s:?}");

    // A folder added later under an included folder is found by the next rescan.
    fs::create_dir_all(root.join("2027-01-01_new")).unwrap();
    fs::write(
        root.join("2027-01-01_new").join("G.jpg"),
        jpeg_with_gps(6.0, 6.0, "2027:01:01 00:00:00"),
    )
    .unwrap();
    assert_eq!(import(&mut lib, src).new, 1);
}

#[test]
fn cancel_and_offline_change_nothing() {
    let dir = tempfile::tempdir().unwrap();
    setup(dir.path());
    let mut lib = Library::open_in_memory().unwrap();
    let src = lib
        .add_source(dir.path(), None, "test", &scope::Scope::all())
        .unwrap();
    let s = run(&mut lib, src, &AtomicBool::new(true), 4, |_| {}).unwrap();
    assert!(s.cancelled);
    assert_eq!(lib.counts().unwrap().photos, 0);

    import(&mut lib, src);
    let gone = lib
        .add_source(
            &dir.path().join("unplugged"),
            None,
            "gone",
            &scope::Scope::all(),
        )
        .unwrap();
    assert!(matches!(
        run(&mut lib, gone, &AtomicBool::new(false), 4, |_| {}),
        Err(ImportError::SourceOffline(_))
    ));
    assert_eq!(lib.counts().unwrap().photos, 4);
}

#[test]
fn quick_hash_ignores_name_and_sees_content() {
    let dir = tempfile::tempdir().unwrap();
    let big: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
    fs::write(dir.path().join("a"), &big).unwrap();
    fs::write(dir.path().join("b"), &big).unwrap();
    let mut early = big.clone();
    early[1000] ^= 1; // differs in the first 64 KB
    fs::write(dir.path().join("c"), &early).unwrap();
    let mut late = big.clone();
    late[299_999] ^= 1; // differs in the last 64 KB
    fs::write(dir.path().join("d"), &late).unwrap();
    let mut middle = big.clone();
    middle[150_000] ^= 1; // differs only in the middle: by design not seen
    fs::write(dir.path().join("e"), &middle).unwrap();
    let h =
        |n: &str| quick_hash(&mut fs::File::open(dir.path().join(n)).unwrap(), 300_000).unwrap();
    assert_eq!(h("a"), h("b"));
    assert_ne!(h("a"), h("c"));
    assert_ne!(h("a"), h("d"));
    assert_eq!(h("a"), h("e"));
    let small = dir.path().join("small");
    fs::write(&small, b"tiny").unwrap();
    quick_hash(&mut fs::File::open(&small).unwrap(), 4).unwrap(); // under 64 KB: no tail read
}
