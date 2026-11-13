//! Search and conflict labels are prepared once per scan, never per frame.

use gpui::SharedString;
use mandara_config::{
    compositor::{self, Keybind, Target},
    keybinds::{self, Binding},
};
use mandara_services::shortcuts::Session;

pub(super) struct Row {
    pub source: usize,
    pub binding: usize,
    pub chord: SharedString,
    pub action: SharedString,
    pub location: SharedString,
    pub clashes: bool,
    search: String,
}

fn chords(target: Target, row: &Binding, primary_mod_is_alt: bool) -> Vec<String> {
    if target == Target::Aqueous {
        return row
            .fields
            .iter()
            .filter(|field| field.label == "Shortcut")
            .filter_map(|field| keybinds::aqueous_chord(&field.value, primary_mod_is_alt))
            .collect();
    }
    if let Some(chord) = &row.chord {
        return vec![chord.clone()];
    }
    row.fields
        .iter()
        .filter(|field| field.label == "Shortcut")
        .filter_map(|field| {
            let value = if target == Target::KWin {
                field.value.replace("Meta+", "Super+")
            } else {
                field.value.clone()
            };
            if value == "none" || value.contains([',', '\'', '"']) {
                return None;
            }
            keybinds::normalize(&value)
        })
        .collect()
}

pub(super) fn build(session: &Session, bindings: &[Keybind]) -> Vec<Row> {
    let own: Vec<_> = bindings
        .iter()
        .map(|bind| compositor::target_chord(session.target, bind))
        .collect();
    session
        .scan
        .sources
        .iter()
        .enumerate()
        .flat_map(|(source_index, source)| {
            let own = &own;
            source
                .bindings
                .iter()
                .enumerate()
                .map(move |(binding, row)| {
                    let chord: SharedString = row
                        .fields
                        .iter()
                        .find(|field| field.label == "Shortcut")
                        .map(|field| field.value.clone())
                        .or_else(|| row.chord.clone())
                        .unwrap_or_default()
                        .into();
                    let action: SharedString = row
                        .fields
                        .iter()
                        .filter(|field| field.label == "Action")
                        .map(|field| field.value.as_str())
                        .collect::<Vec<_>>()
                        .join("; ")
                        .into();
                    let location: SharedString = format!(
                        "{}:{} · {}",
                        source
                            .file
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy(),
                        row.line,
                        row.scope
                    )
                    .into();
                    let mut search =
                        format!("{} {}", source.file.display(), row.scope).to_lowercase();
                    for field in &row.fields {
                        search.push(' ');
                        search.push_str(&field.value.to_lowercase());
                    }
                    Row {
                        source: source_index,
                        binding,
                        chord,
                        action,
                        location,
                        clashes: chords(session.target, row, session.primary_mod_is_alt)
                            .iter()
                            .any(|chord| own.contains(chord)),
                        search,
                    }
                })
        })
        .collect()
}

pub(super) fn filter(rows: &[Row], query: &str) -> Vec<usize> {
    let terms: Vec<_> = query.split_whitespace().map(str::to_lowercase).collect();
    rows.iter()
        .enumerate()
        .filter(|(_, row)| terms.iter().all(|term| row.search.contains(term)))
        .map(|(index, _)| index)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use mandara_config::compositor::BindAction;

    #[test]
    fn search_keeps_duplicate_rows_and_alternate_chords_report_shell_conflicts() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("wm.toml");
        std::fs::write(&file, "[keybinds]\ncycle_focus=['Super+Tab','Alt+Tab']\n[keybinds.custom]\n'Super+Q'='close'\n").unwrap();
        let session = Session {
            primary_mod_is_alt: false,
            target: Target::Aqueous,
            root: file.clone(),
            scan: keybinds::scan_from(Target::Aqueous, &file),
        };
        let rows = build(
            &session,
            &[Keybind::new(
                &["alt"],
                "Tab",
                BindAction::Spawn("true".into()),
            )],
        );
        assert_eq!(rows.len(), 2);
        assert!(rows[0].clashes && !rows[1].clashes);
        assert_eq!(filter(&rows, "WM.toml CYCLE"), [0]);
        assert_eq!(filter(&rows, "close"), [1]);
        assert_eq!(filter(&rows, ""), [0, 1]);
        let mut session = session;
        session.primary_mod_is_alt = true;
        let rows = build(
            &session,
            &[Keybind::new(
                &["alt"],
                "Tab",
                BindAction::Spawn("true".into()),
            )],
        );
        assert!(rows[0].clashes);
    }
}
