//! Personal notes and pins on rows of the Services, Startup and Network
//! lists: "keep running, the backup needs it", or a port worth keeping an
//! eye on at the top. Kept with the window's other remembered settings, per
//! list, by a key that survives restarts (a service name, not a PID).

use std::collections::{BTreeMap, BTreeSet};

use eframe::egui::{self, Id, RichText, Ui};

/// Longest note kept, in characters.
const NOTE_LIMIT: usize = 200;

#[derive(Debug, Default, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Marks {
    #[serde(default)]
    pinned: BTreeSet<String>,
    #[serde(default)]
    notes: BTreeMap<String, String>,
}

/// A note being written: the row's key and the text so far.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Draft {
    key: String,
    title: String,
    text: String,
}

fn id(list: &str) -> Id {
    Id::unique(("row-marks", list))
}

fn draft_id(list: &str) -> Id {
    Id::unique(("row-mark-draft", list))
}

impl Marks {
    /// The marks of `list` ("services", "startup" or "connections").
    pub fn load(ui: &Ui, list: &str) -> Self {
        ui.data_mut(|d| d.get_persisted(id(list)))
            .unwrap_or_default()
    }

    pub fn store(&self, ui: &Ui, list: &str) {
        ui.data_mut(|d| d.insert_persisted(id(list), self.clone()));
    }

    pub fn is_pinned(&self, key: &str) -> bool {
        self.pinned.contains(key)
    }

    pub fn note(&self, key: &str) -> Option<&str> {
        self.notes.get(key).map(String::as_str)
    }

    /// Whether the note of `key` contains `query` (already lower-case).
    pub fn note_matches(&self, key: &str, query: &str) -> bool {
        !query.is_empty()
            && self
                .note(key)
                .is_some_and(|n| n.to_lowercase().contains(query))
    }

    pub fn toggle_pin(&mut self, key: &str) {
        if !self.pinned.remove(key) {
            self.pinned.insert(key.to_owned());
        }
    }

    /// Sets or, with an empty text, removes the note of `key`.
    pub fn set_note(&mut self, key: &str, text: &str) {
        let text: String = text.trim().chars().take(NOTE_LIMIT).collect();
        if text.is_empty() {
            self.notes.remove(key);
        } else {
            self.notes.insert(key.to_owned(), text);
        }
    }

    /// Moves pinned rows to the top, keeping the order within each part.
    pub fn pinned_first<T>(&self, rows: &mut [T], key: impl Fn(&T) -> String) {
        rows.sort_by_key(|row| !self.is_pinned(&key(row)));
    }

    /// The pin and note items of a row's context menu. `title` names the row
    /// in the note editor.
    pub fn menu_items(&mut self, ui: &mut Ui, list: &str, key: &str, title: &str) -> bool {
        let mut changed = false;
        let pin = if self.is_pinned(key) {
            "Unpin"
        } else {
            "Pin to top"
        };
        if crate::context_actions::action(ui, crate::i18n::tr(pin)) {
            self.toggle_pin(key);
            changed = true;
        }
        let edit = if self.note(key).is_some() {
            "Edit note…"
        } else {
            "Add note…"
        };
        if crate::context_actions::action(ui, crate::i18n::tr(edit)) {
            let draft = Draft {
                key: key.to_owned(),
                title: title.to_owned(),
                text: self.note(key).unwrap_or_default().to_owned(),
            };
            ui.data_mut(|d| d.insert_temp(draft_id(list), draft));
        }
        if self.note(key).is_some()
            && crate::context_actions::action(ui, crate::i18n::tr("Remove note"))
        {
            self.set_note(key, "");
            changed = true;
        }
        changed
    }

    /// The row's name with its pin before it and its note after it.
    pub fn name(&self, ui: &mut Ui, key: &str, name: &str) {
        if self.is_pinned(key) {
            ui.label("📌")
                .on_hover_text(crate::i18n::tr("Pinned to the top"));
        }
        ui.label(name);
        if let Some(note) = self.note(key) {
            ui.add(egui::Label::new(RichText::new(format!("✎ {note}")).weak().small()).truncate())
                .on_hover_text(note);
        }
    }

    /// The note editor of `list`, while one is open. Returns whether a note
    /// was saved.
    pub fn editor(&mut self, ui: &Ui, list: &str) -> bool {
        let Some(mut draft) = ui.data(|d| d.get_temp::<Draft>(draft_id(list))) else {
            return false;
        };
        let mut saved = false;
        let mut open = true;
        let modal = egui::Modal::new(Id::unique(("row-mark-editor", list))).show(ui.ctx(), |ui| {
            ui.heading(crate::i18n::format(
                "Note for {0}",
                std::slice::from_ref(&draft.title),
            ));
            let label = crate::i18n::tr("Note");
            let field = ui.add(
                egui::TextEdit::multiline(&mut draft.text)
                    .char_limit(NOTE_LIMIT)
                    .desired_rows(3)
                    .hint_text(crate::i18n::tr("Only on this PC, for your own reference")),
            );
            field.widget_info(|| egui::WidgetInfo::labeled(egui::Role::TextInput, true, label));
            if !field.has_focus() && draft.text.is_empty() {
                field.request_focus();
            }
            ui.horizontal(|ui| {
                if ui.button(crate::i18n::tr("Save")).clicked() {
                    self.set_note(&draft.key, &draft.text);
                    saved = true;
                    open = false;
                }
                if ui.button(crate::i18n::tr("Cancel")).clicked() {
                    open = false;
                }
            });
        });
        if modal.should_close() {
            open = false;
        }
        ui.data_mut(|d| {
            if open {
                d.insert_temp(draft_id(list), draft);
            } else {
                d.remove::<Draft>(draft_id(list));
            }
        });
        saved
    }
}

/// The key of a network endpoint's marks: the program and the port it
/// serves, or the remote end it talks to. It outlives the process's PID.
/// `serves` says the program listens on the connection's local port, so an
/// accepted connection belongs with that port.
pub fn connection_key(program: &str, c: &wtm_core::Connection, serves: bool) -> String {
    let program = program.to_lowercase();
    match c
        .remote
        .filter(|r| r.port() != 0 && !r.ip().is_unspecified())
    {
        Some(remote) if !serves => format!("{program}|to {remote}"),
        _ => format!("{program}|port {}", c.local.port()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wtm_core::{Connection, Protocol};
    use wtm_sys::net::TcpState;

    #[test]
    fn pins_and_notes_toggle_trim_and_bound() {
        let mut m = Marks::default();
        m.toggle_pin("Spooler");
        assert!(m.is_pinned("Spooler"));
        m.toggle_pin("Spooler");
        assert!(!m.is_pinned("Spooler"));
        m.set_note("Spooler", "  the label printer needs it  ");
        assert_eq!(m.note("Spooler"), Some("the label printer needs it"));
        assert!(m.note_matches("Spooler", "label"));
        assert!(!m.note_matches("Spooler", ""));
        m.set_note("Spooler", &"x".repeat(500));
        assert_eq!(m.note("Spooler").unwrap().chars().count(), NOTE_LIMIT);
        m.set_note("Spooler", "   ");
        assert_eq!(m.note("Spooler"), None, "an empty note is removed");
    }

    #[test]
    fn pinned_rows_lead_and_keep_their_sorted_order() {
        let mut m = Marks::default();
        m.toggle_pin("c");
        m.toggle_pin("a");
        let mut rows = vec!["a", "b", "c", "d"];
        m.pinned_first(&mut rows, |r| (*r).to_owned());
        assert_eq!(rows, ["a", "c", "b", "d"]);
    }

    #[test]
    fn connection_keys_follow_the_port_not_the_pid() {
        let conn = |local: &str, remote: Option<&str>, state, pid| Connection {
            protocol: Protocol::Tcp,
            local: local.parse().unwrap(),
            remote: remote.map(|r| r.parse().unwrap()),
            state: Some(state),
            pid,
        };
        let listen = conn("0.0.0.0:5432", None, TcpState::Listen, 10);
        let accepted = conn(
            "127.0.0.1:5432",
            Some("127.0.0.1:60123"),
            TcpState::Established,
            11,
        );
        let outbound = conn(
            "10.0.0.2:60500",
            Some("1.2.3.4:443"),
            TcpState::Established,
            12,
        );
        assert_eq!(
            connection_key("Postgres.exe", &listen, true),
            "postgres.exe|port 5432"
        );
        assert_eq!(
            connection_key("postgres.exe", &accepted, true),
            "postgres.exe|port 5432"
        );
        assert_eq!(
            connection_key("app.exe", &outbound, false),
            "app.exe|to 1.2.3.4:443"
        );
        let udp = Connection {
            protocol: Protocol::Udp,
            local: "0.0.0.0:53".parse().unwrap(),
            remote: None,
            state: None,
            pid: 1,
        };
        assert_eq!(connection_key("dns.exe", &udp, false), "dns.exe|port 53");
    }

    #[test]
    fn marks_survive_a_round_trip_through_saved_settings() {
        let mut m = Marks::default();
        m.toggle_pin("a");
        m.set_note("b", "note");
        let text = serde_json::to_string(&m).unwrap();
        assert_eq!(serde_json::from_str::<Marks>(&text).unwrap(), m);
        assert_eq!(
            serde_json::from_str::<Marks>("{}").unwrap(),
            Marks::default()
        );
    }
}
