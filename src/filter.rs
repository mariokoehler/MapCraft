//! Filters (design.md §2.3): a date range, sources and cameras. They narrow what the map shows;
//! hidden photos also leave the groups, the counts and search.
//!
//! One rule decides what a filter hides: the SQL condition from [`Filter::sql`], used by every
//! query that lists photos on the map (positions, place counts, search). There's no second
//! in-memory version that could drift from it.
//!
//! Sources and cameras are stored as what's *hidden*, so a camera or source that appears later
//! (an import during the session) shows rather than being left out silently. Filters live for
//! the session only and are never logged (design.md §3.8, §4).

use std::collections::{BTreeSet, HashMap};

use rusqlite::types::Value;

/// A month as `yyyymm` (201905 is May 2019).
pub type Month = u32;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Filter {
    /// The months shown, both included; `None` is any time. A range hides undated photos.
    pub dates: Option<(Month, Month)>,
    pub hidden_sources: BTreeSet<i64>,
    /// Raw camera keys ([`camera_key`]) left out.
    pub hidden_cameras: BTreeSet<String>,
}

/// A SQL condition (`" AND …"`, or empty) with its named parameters.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Where {
    pub sql: String,
    pub params: Vec<(String, Value)>,
}

impl Filter {
    pub fn is_active(&self) -> bool {
        self.active_count() > 0
    }

    /// How many filters are set (the badge on the Filters button): date, sources, cameras.
    pub fn active_count(&self) -> usize {
        self.dates.is_some() as usize
            + !self.hidden_sources.is_empty() as usize
            + !self.hidden_cameras.is_empty() as usize
    }

    /// The condition on table `photo` (columns prefixed with `prefix`, such as `"p."`). Values
    /// are bound as parameters, never written into the SQL: camera names come from the files.
    pub fn sql(&self, prefix: &str) -> Where {
        let mut w = Where::default();
        if let Some((from, to)) = self.dates {
            // `taken_at` is ISO 8601 text, so months compare as strings: from ≤ t < next month.
            w.sql.push_str(&format!(
                " AND {prefix}taken_at >= :f_from AND {prefix}taken_at < :f_to"
            ));
            w.params
                .push((":f_from".into(), Value::Text(month_start(from))));
            w.params
                .push((":f_to".into(), Value::Text(month_start(next_month(to)))));
        }
        if !self.hidden_sources.is_empty() {
            let names: Vec<String> = (0..self.hidden_sources.len())
                .map(|i| format!(":f_s{i}"))
                .collect();
            w.sql.push_str(&format!(
                " AND {prefix}source_id NOT IN ({})",
                names.join(", ")
            ));
            for (n, id) in names.into_iter().zip(&self.hidden_sources) {
                w.params.push((n, Value::Integer(*id)));
            }
        }
        if !self.hidden_cameras.is_empty() {
            let names: Vec<String> = (0..self.hidden_cameras.len())
                .map(|i| format!(":f_c{i}"))
                .collect();
            w.sql.push_str(&format!(
                " AND {} NOT IN ({})",
                camera_sql(prefix),
                names.join(", ")
            ));
            for (n, key) in names.into_iter().zip(&self.hidden_cameras) {
                w.params.push((n, Value::Text(key.clone())));
            }
        }
        w
    }

    /// Shows or hides a camera (all its raw keys).
    pub fn set_camera(&mut self, camera: &Camera, shown: bool) {
        for k in &camera.keys {
            if shown {
                self.hidden_cameras.remove(k);
            } else {
                self.hidden_cameras.insert(k.clone());
            }
        }
    }

    pub fn camera_shown(&self, camera: &Camera) -> bool {
        !camera.keys.iter().all(|k| self.hidden_cameras.contains(k))
    }

    /// Forgets hidden sources that no longer exist (removed during the session).
    pub fn prune_sources(&mut self, alive: impl Fn(i64) -> bool) {
        self.hidden_sources.retain(|&id| alive(id));
    }
}

/// The SQL expression of a photo's raw camera key: make, unit separator, model.
pub fn camera_sql(prefix: &str) -> String {
    format!("coalesce({prefix}camera_make, '') || char(31) || coalesce({prefix}camera_model, '')")
}

/// A raw camera key as [`camera_sql`] computes it.
pub fn camera_key(make: Option<&str>, model: Option<&str>) -> String {
    format!("{}\u{1f}{}", make.unwrap_or(""), model.unwrap_or(""))
}

/// One camera as the filter lists it: raw (make, model) pairs that read the same are one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Camera {
    pub label: String,
    pub keys: Vec<String>,
    pub photos: usize,
}

/// What the filter panel offers: the library's photos on the map by month, source and camera.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Facets {
    /// Photos per month, in order (months without photos left out).
    pub months: Vec<(Month, usize)>,
    pub undated: usize,
    pub sources: HashMap<i64, usize>,
    /// The most photos first.
    pub cameras: Vec<Camera>,
}

impl Facets {
    /// The first and last month with photos.
    pub fn span(&self) -> Option<(Month, Month)> {
        Some((self.months.first()?.0, self.months.last()?.0))
    }

    /// The years with photos, newest first.
    pub fn years(&self) -> Vec<u32> {
        let mut y: Vec<u32> = self.months.iter().map(|(m, _)| m / 100).collect();
        y.dedup();
        y.reverse();
        y
    }

    /// Groups raw (make, model, photos) rows into cameras by their label.
    pub fn cameras_from(rows: Vec<(Option<String>, Option<String>, usize)>) -> Vec<Camera> {
        let mut by: Vec<Camera> = Vec::new();
        for (make, model, n) in rows {
            let label = crate::ui::details::camera_text(
                make.as_deref().filter(|s| !s.trim().is_empty()),
                model.as_deref().filter(|s| !s.trim().is_empty()),
            )
            .unwrap_or_else(|| "Unknown camera".to_owned());
            let key = camera_key(make.as_deref(), model.as_deref());
            match by.iter_mut().find(|c| c.label == label) {
                Some(c) => {
                    c.keys.push(key);
                    c.photos += n;
                }
                None => by.push(Camera {
                    label,
                    keys: vec![key],
                    photos: n,
                }),
            }
        }
        by.sort_by(|a, b| b.photos.cmp(&a.photos).then_with(|| a.label.cmp(&b.label)));
        by
    }
}

/// `"2019-05"`, the form `taken_at` starts with.
pub fn month_start(m: Month) -> String {
    format!("{:04}-{:02}", m / 100, m % 100)
}

/// Parses the start of a `taken_at` (`"2019-05…"`) into a month.
pub fn parse_month(s: &str) -> Option<Month> {
    let y: u32 = s.get(..4)?.parse().ok()?;
    if s.get(4..5)? != "-" {
        return None;
    }
    let m: u32 = s.get(5..7)?.parse().ok()?;
    (1..=12).contains(&m).then_some(y * 100 + m)
}

pub fn next_month(m: Month) -> Month {
    if m % 100 >= 12 {
        (m / 100 + 1) * 100 + 1
    } else {
        m + 1
    }
}

/// Months from the first of the span: the index of `m` on the slider.
pub fn month_index(first: Month, m: Month) -> i64 {
    (m / 100) as i64 * 12 + (m % 100) as i64 - ((first / 100) as i64 * 12 + (first % 100) as i64)
}

/// The month `i` months after `first`.
pub fn month_at(first: Month, i: i64) -> Month {
    let n = (first / 100) as i64 * 12 + (first % 100) as i64 - 1 + i;
    ((n / 12) * 100 + n % 12 + 1) as Month
}

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// "May 2019".
pub fn month_text(m: Month) -> String {
    let name = MONTHS
        .get((m % 100).saturating_sub(1) as usize)
        .unwrap_or(&"");
    format!("{name} {}", m / 100)
}

/// "May 2019 – Aug 2023", or "May 2019" for one month.
pub fn range_text(from: Month, to: Month) -> String {
    if from == to {
        month_text(from)
    } else {
        format!("{} – {}", month_text(from), month_text(to))
    }
}

/// A checklist's chip: "DJI FC3582" (one shown), "All cameras but iPhone 13 mini" (one
/// hidden), "3 of 9 cameras", or "No cameras". `names` are the shown and hidden labels.
pub fn list_text(kind: &str, shown: &[&str], hidden: &[&str]) -> String {
    let total = shown.len() + hidden.len();
    match (shown, hidden) {
        ([], _) => format!("No {kind}"),
        ([one], _) => (*one).to_owned(),
        (_, [one]) => format!("All {kind} but {one}"),
        _ => format!("{} of {total} {kind}", shown.len()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn months_count_and_step_over_years() {
        assert_eq!(next_month(201912), 202001);
        assert_eq!(next_month(201905), 201906);
        assert_eq!(month_index(201911, 202002), 3);
        assert_eq!(month_at(201911, 3), 202002);
        assert_eq!(month_at(201911, 0), 201911);
        for i in 0..40 {
            assert_eq!(month_index(201805, month_at(201805, i)), i);
        }
        assert_eq!(parse_month("2019-05-14T10:00:00"), Some(201905));
        assert_eq!(parse_month("2019-13"), None);
        assert_eq!(parse_month("19"), None);
        assert_eq!(range_text(201905, 202308), "May 2019 – Aug 2023");
        assert_eq!(range_text(201701, 201701), "Jan 2017");
    }

    #[test]
    fn the_condition_binds_every_value() {
        let mut f = Filter::default();
        assert_eq!(f.sql("p.").sql, "");
        assert!(!f.is_active());
        f.dates = Some((201905, 202312));
        f.hidden_sources.insert(7);
        f.hidden_cameras.insert(camera_key(Some("Bob's"), None));
        let w = f.sql("p.");
        assert_eq!(f.active_count(), 3);
        assert!(!w.sql.contains("Bob"), "camera names are parameters");
        assert!(w.sql.contains("p.taken_at >= :f_from"));
        assert!(
            w.params
                .contains(&(":f_to".into(), Value::Text("2024-01".into())))
        );
        assert!(w.params.contains(&(":f_s0".into(), Value::Integer(7))));
    }

    #[test]
    fn cameras_that_read_the_same_are_one() {
        let rows = vec![
            (Some("Canon".into()), Some("Canon EOS R5".into()), 3),
            (Some("CANON".into()), Some("Canon EOS R5".into()), 2),
            (None, None, 4),
            (Some(String::new()), None, 1),
            (Some("Apple".into()), Some("iPhone 13 mini".into()), 9),
        ];
        let c = Facets::cameras_from(rows);
        let labels: Vec<_> = c.iter().map(|c| (c.label.as_str(), c.photos)).collect();
        assert_eq!(
            labels,
            [
                ("Apple iPhone 13 mini", 9),
                ("Canon EOS R5", 5),
                ("Unknown camera", 5),
            ]
        );
        let mut f = Filter::default();
        f.set_camera(&c[1], false);
        assert_eq!(f.hidden_cameras.len(), 2);
        assert!(!f.camera_shown(&c[1]));
        f.set_camera(&c[1], true);
        assert!(f.hidden_cameras.is_empty());
    }

    #[test]
    fn chips_name_what_is_shown() {
        assert_eq!(list_text("cameras", &["DJI"], &["A", "B"]), "DJI");
        assert_eq!(
            list_text("cameras", &["A", "B"], &["iPhone"]),
            "All cameras but iPhone"
        );
        assert_eq!(
            list_text("cameras", &["A", "B"], &["C", "D"]),
            "2 of 4 cameras"
        );
        assert_eq!(list_text("sources", &[], &["C"]), "No sources");
    }
}
