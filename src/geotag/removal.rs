//! One removal session (design.md §2.9(n), canvas `MapCraft — Remove position`): the photos in
//! scope, which of them are ticked, and what writing will do. Pure state, so it's tested without
//! the UI; `ui::remove_panel` draws it and the app wires it to the map.
//!
//! Only photos whose position comes from a sidecar can be ticked. Those with a position inside
//! the file are in scope too, shown faded without a tick: MapCraft never changes originals.

use std::collections::HashSet;

use crate::library::{PhotoPoint, PlacedPhoto};

/// Which photos the session shows (design.md §2.9(n)).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// The folder of the photo it started from (the default).
    Folder,
    /// Every photo the filters let through.
    Map,
}

pub struct Removal {
    pub scope: Scope,
    /// The photo it started from (its folder is the scope's).
    pub around: i64,
    /// That photo's folder, as shown in the panel.
    pub folder: String,
    pub photos: Vec<PlacedPhoto>,
    /// The ticked photos.
    pub on: HashSet<i64>,
    /// The photo clicked on the map.
    pub selected: Option<i64>,
    /// Something was ticked or unticked since it began (leaving asks first).
    pub changed: bool,
}

impl Removal {
    /// A session on `photos` that starts with `around` ticked.
    pub fn new(around: i64, folder: String, photos: Vec<PlacedPhoto>) -> Self {
        let mut r = Removal {
            scope: Scope::Folder,
            around,
            folder,
            photos: Vec::new(),
            on: HashSet::new(),
            selected: Some(around),
            changed: false,
        };
        r.set_photos(photos);
        if r.removable_ids().contains(&around) {
            r.on.insert(around);
        }
        r
    }

    /// New photos (the scope changed, a reload): ticks stay on those still removable.
    pub fn set_photos(&mut self, photos: Vec<PlacedPhoto>) {
        self.photos = photos;
        let ok = self.removable_ids();
        self.on.retain(|id| ok.contains(id));
        if self
            .selected
            .is_some_and(|s| !self.photos.iter().any(|p| p.id == s))
        {
            self.selected = None;
        }
    }

    fn removable_ids(&self) -> HashSet<i64> {
        self.photos
            .iter()
            .filter(|p| p.removable())
            .map(|p| p.id)
            .collect()
    }

    pub fn removable(&self) -> usize {
        self.photos.iter().filter(|p| p.removable()).count()
    }

    /// Photos in scope whose position is inside the file.
    pub fn in_file(&self) -> usize {
        self.photos.len() - self.removable()
    }

    pub fn ticked(&self) -> usize {
        self.on.len()
    }

    /// Every photo in scope, for the map.
    pub fn points(&self) -> Vec<PhotoPoint> {
        self.photos.iter().map(PlacedPhoto::point).collect()
    }

    /// The unticked removable photos (the map's ticks show them off).
    pub fn off(&self) -> HashSet<i64> {
        self.photos
            .iter()
            .filter(|p| p.removable() && !self.on.contains(&p.id))
            .map(|p| p.id)
            .collect()
    }

    /// Photos that can't be ticked: their position is inside the file.
    pub fn locked(&self) -> HashSet<i64> {
        self.photos
            .iter()
            .filter(|p| !p.removable())
            .map(|p| p.id)
            .collect()
    }

    /// Flips these photos' ticks (the map passes the photos whose tick changes); photos that
    /// can't be ticked are left out.
    pub fn toggle(&mut self, ids: &[i64]) {
        let ok = self.removable_ids();
        for id in ids.iter().filter(|id| ok.contains(id)) {
            if !self.on.remove(id) {
                self.on.insert(*id);
            }
        }
        self.changed = true;
    }

    pub fn tick_all(&mut self) {
        self.on = self.removable_ids();
        self.changed = true;
    }

    pub fn untick_all(&mut self) {
        self.on.clear();
        self.changed = true;
    }

    /// The ticked photos, in id order.
    pub fn removals(&self) -> Vec<PlacedPhoto> {
        self.photos
            .iter()
            .filter(|p| self.on.contains(&p.id))
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::library::Thumb;

    fn photo(id: i64, source: &str) -> PlacedPhoto {
        PlacedPhoto {
            id,
            lat: 10.0 + id as f64,
            lon: 20.0,
            alt: None,
            gps_source: Some(source.to_owned()),
            thumb: Thumb::Pending,
            taken: 0,
            root: PathBuf::from("C:\\P"),
            unc: None,
            rel_dir: "trip".into(),
            original_name: format!("IMG_{id}.CR2"),
            preview_name: None,
            sidecar_name: (source == "xmp").then(|| format!("IMG_{id}.xmp")),
        }
    }

    #[test]
    fn only_sidecar_positions_tick() {
        let photos = vec![photo(1, "xmp"), photo(2, "xmp"), photo(3, "exif")];
        let mut r = Removal::new(2, "C:\\P\\trip".into(), photos);
        assert_eq!((r.removable(), r.in_file(), r.ticked()), (2, 1, 1));
        assert_eq!(r.off(), HashSet::from([1]));
        assert_eq!(r.locked(), HashSet::from([3]));
        assert_eq!(r.points().len(), 3);
        // A group's tick passes every member; the one inside its file stays unticked.
        r.toggle(&[1, 3]);
        assert_eq!(r.ticked(), 2);
        assert!(r.changed);
        r.untick_all();
        assert_eq!(r.ticked(), 0);
        r.tick_all();
        assert_eq!(
            r.removals().iter().map(|p| p.id).collect::<Vec<_>>(),
            [1, 2]
        );
    }

    #[test]
    fn starting_from_a_photo_inside_its_file_ticks_nothing() {
        let r = Removal::new(3, String::new(), vec![photo(1, "xmp"), photo(3, "exif")]);
        assert_eq!(r.ticked(), 0);
        assert!(!r.changed);
    }

    #[test]
    fn new_photos_keep_the_ticks_that_still_apply() {
        let mut r = Removal::new(1, String::new(), vec![photo(1, "xmp"), photo(2, "xmp")]);
        r.toggle(&[2]);
        // Photo 1's position was removed meanwhile (a reload): it's gone, 2 stays ticked.
        r.set_photos(vec![photo(2, "xmp"), photo(4, "xmp")]);
        assert_eq!(r.on, HashSet::from([2]));
        assert_eq!(r.selected, None);
    }
}
