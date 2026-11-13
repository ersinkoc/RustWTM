//! What changed in the running version, from the changelog built into the
//! program. Shown offline; the entries themselves are in English.

use eframe::egui::{self, RichText, Ui};

const CHANGELOG: &str = include_str!("../../../CHANGELOG.md");

/// One `### Added` / `### Fixed` … group of a release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    pub title: String,
    /// One entry per bullet, its wrapped lines joined.
    pub entries: Vec<String>,
}

/// The release headed `## [version]`, or `None` when the changelog has no
/// such section.
pub fn section(changelog: &str, version: &str) -> Option<(String, Vec<Group>)> {
    let mut lines = changelog.lines();
    let heading = lines.find(|l| {
        l.strip_prefix("## [")
            .and_then(|rest| rest.split(']').next())
            .is_some_and(|v| v == version)
    })?;
    let date = heading
        .split_once("] - ")
        .map(|(_, d)| d.trim().to_owned())
        .unwrap_or_default();
    let mut groups: Vec<Group> = Vec::new();
    for line in lines.take_while(|l| !l.starts_with("## ")) {
        if let Some(title) = line.strip_prefix("### ") {
            groups.push(Group {
                title: title.trim().to_owned(),
                entries: Vec::new(),
            });
        } else if let Some(group) = groups.last_mut() {
            let text = line.trim();
            if let Some(entry) = line.strip_prefix("- ") {
                group.entries.push(entry.trim().to_owned());
            } else if !text.is_empty()
                && let Some(last) = group.entries.last_mut()
            {
                last.push(' ');
                last.push_str(text);
            }
        }
    }
    groups.retain(|g| !g.entries.is_empty());
    Some((date, groups))
}

/// The running version's notes, or the unreleased ones in a development build.
fn current() -> Option<(String, String, Vec<Group>)> {
    let version = env!("CARGO_PKG_VERSION");
    section(CHANGELOG, version)
        .map(|(date, groups)| (version.to_owned(), date, groups))
        .or_else(|| {
            section(CHANGELOG, "Unreleased")
                .map(|(_, groups)| (version.to_owned(), String::new(), groups))
        })
}

/// Group titles of Keep a Changelog, in the interface language.
fn group_title(title: &str) -> &str {
    match title {
        "Added" => crate::i18n::tr("Added"),
        "Changed" => crate::i18n::tr("Changed"),
        "Fixed" => crate::i18n::tr("Fixed"),
        "Removed" => crate::i18n::tr("Removed"),
        "Security" => crate::i18n::tr("Security"),
        "Deprecated" => crate::i18n::tr("Deprecated"),
        other => other,
    }
}

/// `**bold**`, `*emphasis*` and `` `code` `` spans of an entry as plain text.
fn plain(entry: &str) -> String {
    entry.replace("**", "").replace(['*', '`'], "")
}

/// A card with the notes of this version, its groups expandable.
pub fn show(ui: &mut Ui) {
    let Some((version, date, groups)) = current() else {
        return;
    };
    crate::theme::panel(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new(crate::i18n::format("What's new in {0}", &[version]))
                    .strong()
                    .size(16.0),
            );
            if !date.is_empty() {
                ui.weak(date);
            }
        });
        let id = ui.make_persistent_id("release-notes-open");
        let mut open = ui.data(|d| d.get_temp::<Option<bool>>(id)).flatten();
        ui.horizontal(|ui| {
            if ui.small_button(crate::i18n::tr("Expand all")).clicked() {
                open = Some(true);
            }
            if ui.small_button(crate::i18n::tr("Collapse all")).clicked() {
                open = Some(false);
            }
        });
        for (i, group) in groups.iter().enumerate() {
            let header = egui::CollapsingHeader::new(format!(
                "{} ({})",
                group_title(&group.title),
                group.entries.len()
            ))
            .id_salt(("release-notes", i))
            .default_open(i == 0)
            .open(open);
            header.show(ui, |ui| {
                for entry in &group.entries {
                    ui.horizontal_top(|ui| {
                        ui.label("•");
                        ui.add(egui::Label::new(plain(entry)).wrap());
                    });
                }
            });
        }
        // Expand/collapse all applies once; afterwards each group is free.
        ui.data_mut(|d| d.insert_temp::<Option<bool>>(id, None));
        ui.hyperlink_to(
            crate::i18n::tr("Full changelog"),
            format!("{}/blob/main/CHANGELOG.md", crate::about::REPO),
        );
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "# Changelog\n\n## [Unreleased]\n\n### Added\n\n- Next thing.\n\n\
## [1.2.0] - 2026-01-02\n\n### Added\n\n- **Bold** first entry that\n  wraps onto two lines.\n\
- `code` second.\n\n### Fixed\n\n- A fix.\n\n### Removed\n\n## [1.1.0] - 2025-12-01\n\n### Added\n\n- Old.\n";

    #[test]
    fn a_release_section_ends_at_the_next_release_and_joins_wrapped_bullets() {
        let (date, groups) = section(SAMPLE, "1.2.0").unwrap();
        assert_eq!(date, "2026-01-02");
        assert_eq!(groups.len(), 2, "empty groups are dropped");
        assert_eq!(groups[0].title, "Added");
        assert_eq!(
            groups[0].entries,
            [
                "**Bold** first entry that wraps onto two lines.",
                "`code` second."
            ]
        );
        assert_eq!(groups[1].entries, ["A fix."]);
        assert_eq!(plain(&groups[0].entries[1]), "code second.");
        assert_eq!(plain("*Settings* → **AI**"), "Settings → AI");
        assert_eq!(
            section(SAMPLE, "Unreleased").unwrap().1[0].entries,
            ["Next thing."]
        );
        assert!(section(SAMPLE, "9.9.9").is_none());
        // A version must match exactly, not as a prefix.
        assert!(section(SAMPLE, "1.2").is_none());
    }

    #[test]
    fn the_built_in_changelog_has_notes_for_this_build() {
        let (_, _, groups) = current().expect("CHANGELOG.md documents this version");
        assert!(groups.iter().any(|g| !g.entries.is_empty()));
    }
}
