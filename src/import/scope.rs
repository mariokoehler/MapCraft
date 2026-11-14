//! Which folders of a source are imported (design.md §2.8(f)).
//!
//! A scope stores *decisions*: (folder, included or not). A folder without a decision inherits
//! from its nearest parent that has one; with no decisions at all, everything is included. So a
//! folder added later under an included folder is imported by the next rescan, and photos lying
//! directly in a partly picked folder can be included or not, like any other folder.

use std::collections::BTreeMap;

use crate::library::key;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Scope {
    /// Folder key (relative path, lower case, `\` separated; `` is the root) → included.
    rules: BTreeMap<String, bool>,
}

/// The parent of a folder key: `a\b` → `a`, `a` → `` (the root), the root → none.
pub fn parent(dir: &str) -> Option<&str> {
    if dir.is_empty() {
        None
    } else {
        Some(dir.rsplit_once('\\').map_or("", |(p, _)| p))
    }
}

impl Scope {
    /// Everything included (no decisions).
    pub fn all() -> Self {
        Self::default()
    }

    pub fn from_rules(rules: impl IntoIterator<Item = (String, bool)>) -> Self {
        let mut s = Self::default();
        for (dir, included) in rules {
            s.rules.insert(key(&dir), included);
        }
        s.normalise();
        s
    }

    /// The stored decisions, for the library.
    pub fn rules(&self) -> impl Iterator<Item = (&str, bool)> {
        self.rules.iter().map(|(d, i)| (d.as_str(), *i))
    }

    /// Is the folder itself included (its own files imported)?
    pub fn includes(&self, dir: &str) -> bool {
        let mut d = key(dir);
        loop {
            if let Some(i) = self.rules.get(&d) {
                return *i;
            }
            match parent(&d) {
                Some(p) => d = p.to_owned(),
                None => return true,
            }
        }
    }

    /// Must the import enter the folder: is it, or anything below it, included?
    pub fn reaches(&self, dir: &str) -> bool {
        let d = key(dir);
        if self.includes(&d) {
            return true;
        }
        let prefix = if d.is_empty() {
            String::new()
        } else {
            format!("{d}\\")
        };
        self.rules
            .iter()
            .any(|(r, i)| *i && (d.is_empty() || r.starts_with(&prefix)))
    }

    /// Sets a folder and everything below it (the tree's checkbox: "checking a folder checks
    /// all descendants").
    pub fn set(&mut self, dir: &str, included: bool) {
        let d = key(dir);
        let prefix = format!("{d}\\");
        self.rules
            .retain(|r, _| !(d.is_empty() || *r == d || r.starts_with(&prefix)));
        self.rules.insert(d, included);
        self.normalise();
    }

    /// Drops decisions that repeat what the folder would inherit anyway.
    fn normalise(&mut self) {
        let dirs: Vec<String> = self.rules.keys().cloned().collect();
        for d in dirs {
            let own = self.rules[&d];
            let inherited = {
                let mut p = parent(&d).map(str::to_owned);
                loop {
                    match p {
                        Some(ref pd) => {
                            if let Some(i) = self.rules.get(pd) {
                                break *i;
                            }
                            p = parent(pd).map(str::to_owned);
                        }
                        None => break true,
                    }
                }
            };
            if own == inherited {
                self.rules.remove(&d);
            }
        }
    }

    /// The tree's tri-state for a folder whose subfolders are `children` (keys of direct
    /// subfolders as listed): on, off, or mixed when anything below differs from the folder.
    pub fn state(&self, dir: &str) -> Check {
        let d = key(dir);
        let own = self.includes(&d);
        let prefix = if d.is_empty() {
            String::new()
        } else {
            format!("{d}\\")
        };
        let differs = self
            .rules
            .iter()
            .any(|(r, i)| *i != own && r != &d && (d.is_empty() || r.starts_with(&prefix)));
        match (own, differs) {
            (_, true) => Check::Mixed,
            (true, false) => Check::On,
            (false, false) => Check::Off,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Check {
    On,
    Off,
    Mixed,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn everything_is_included_by_default_and_new_folders_inherit() {
        let s = Scope::all();
        assert!(s.includes("") && s.includes(r"2026-05-01_New"));
        let mut s = Scope::all();
        s.set("Phone", false);
        assert!(s.includes("2019-12-07_Market"), "unpicked siblings stay in");
        assert!(
            s.includes("2027-01-01_Added later"),
            "a folder added later is imported"
        );
        assert!(!s.includes(r"Phone\2021"), "below an excluded folder: out");
    }

    #[test]
    fn a_mixed_folder_keeps_its_own_files_decision() {
        let mut s = Scope::all();
        s.set(r"Phone\Screenshots", false);
        assert!(
            s.includes("Phone"),
            "the files directly in Phone still count"
        );
        assert_eq!(s.state("Phone"), Check::Mixed);
        assert_eq!(s.state(r"Phone\Screenshots"), Check::Off);
        assert_eq!(s.state(""), Check::Mixed);
        // Picking just one folder of an excluded root.
        let mut s = Scope::all();
        s.set("", false);
        s.set(r"Phone\2021", true);
        assert!(!s.includes("Phone") && s.includes(r"Phone\2021"));
        assert!(
            s.reaches("Phone") && s.reaches(""),
            "pass through to reach 2021"
        );
        assert!(!s.reaches("Other"));
    }

    #[test]
    fn unticking_everything_imports_nothing_and_checking_resets_below() {
        let mut s = Scope::all();
        s.set("a", false);
        s.set("", false);
        assert!(!s.includes("a") && !s.includes("b") && !s.reaches(""));
        assert_eq!(s.rules().count(), 1, "the rule for `a` is redundant now");
        s.set("", true);
        assert_eq!(
            s,
            Scope::all(),
            "checking the root clears every decision below"
        );
    }

    #[test]
    fn keys_are_case_insensitive() {
        let s = Scope::from_rules([("PHONE".to_string(), false)]);
        assert!(!s.includes(r"phone\x"));
    }
}
