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

fn clashes(
    target: Target,
    row: &Binding,
    primary_mod_is_alt: bool,
    own: &[String],
    qt: &[i32],
) -> bool {
    if target == Target::Sway {
        return row
            .chord
            .as_ref()
            .is_some_and(|chord| own.binary_search(chord).is_ok());
    }
    if target == Target::KWin {
        return row
            .fields
            .iter()
            .filter(|field| field.label == "Shortcut")
            .filter_map(|field| keybinds::qt_sequence(&field.value).ok())
            .any(|keys| keys[0] != 0 && qt.binary_search(&keys[0]).is_ok());
    }
    if target == Target::Aqueous {
        return row
            .fields
            .iter()
            .filter(|field| field.label == "Shortcut")
            .filter_map(|field| keybinds::aqueous_chord(&field.value, primary_mod_is_alt))
            .any(|chord| own.binary_search(&chord).is_ok());
    }
    if let Some(chord) = &row.chord {
        return own.binary_search(chord).is_ok();
    }
    row.fields
        .iter()
        .filter(|field| field.label == "Shortcut")
        .filter_map(|field| {
            let value = &field.value;
            if value == "none" || value.contains([',', '\'', '"']) {
                return None;
            }
            keybinds::normalize(value)
        })
        .any(|chord| own.binary_search(&chord).is_ok())
}

pub(super) fn build(session: &Session, bindings: &[Keybind]) -> Vec<Row> {
    let mut own: Vec<_> = bindings
        .iter()
        .map(|bind| compositor::target_chord(session.target, bind))
        .collect();
    own.sort_unstable();
    own.dedup();
    let mut qt: Vec<_> = if session.target == Target::KWin {
        bindings.iter().filter_map(keybinds::qt_keybind).collect()
    } else {
        Vec::new()
    };
    qt.sort_unstable();
    qt.dedup();
    session
        .scan
        .sources
        .iter()
        .enumerate()
        .flat_map(|(source_index, source)| {
            let own = &own;
            let qt = &qt;
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
                    let mut action = String::new();
                    for field in row.fields.iter().filter(|field| field.label == "Action") {
                        if !action.is_empty() {
                            action.push_str("; ");
                        }
                        action.push_str(&field.value);
                    }
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
                        action: action.into(),
                        location,
                        clashes: clashes(session.target, row, session.primary_mod_is_alt, own, qt),
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
    fn qt_alternatives_and_sequence_prefixes_compare_with_large_shell_binding_lists() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("kglobalshortcutsrc");
        let mut body = String::from("[kwin]\n");
        for (index, value) in [
            "Ctrl+,",
            "Ctrl++",
            "Ctrl+K, Ctrl+C",
            "Meta+Page Up",
            "Volume Up",
            "Meta+Z",
        ]
        .iter()
        .enumerate()
        {
            body.push_str(&format!(
                "Action{index}={},none,Keep label\n",
                keybinds::Encoding::KConfig.encode(value).unwrap()
            ));
        }
        std::fs::write(&file, body).unwrap();
        let session = Session {
            environment: Default::default(),
            target: Target::KWin,
            root: file.clone(),
            scan: keybinds::scan_from(Target::KWin, &file),
            primary_mod_is_alt: false,
        };
        let mut bindings =
            vec![Keybind::new(&["alt"], "F12", BindAction::Spawn("true".into())); 2048];
        for (mods, key) in [
            (&["ctrl"][..], ","),
            (&["ctrl"][..], "+"),
            (&["ctrl"][..], "K"),
            (&["super"][..], "Prior"),
            (&[][..], "XF86AudioRaiseVolume"),
        ] {
            bindings.push(Keybind::new(mods, key, BindAction::Spawn("true".into())));
        }
        let rows = build(&session, &bindings);
        assert_eq!(rows.len(), 6);
        assert!(rows[..5].iter().all(|row| row.clashes));
        assert!(!rows[5].clashes);
    }

    #[test]
    fn multiple_actions_keep_their_order_and_remain_searchable() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("rc.xml");
        std::fs::write(&file, "<labwc_config><keyboard><keybind key=\"W-q\"><action name=\"Focus\"/><action name=\"Raise\"/></keybind></keyboard></labwc_config>").unwrap();
        let session = Session {
            environment: Default::default(),
            target: Target::Labwc,
            root: file.clone(),
            scan: keybinds::scan_from(Target::Labwc, &file),
            primary_mod_is_alt: false,
        };
        let rows = build(&session, &[]);
        assert_eq!(rows[0].action.as_ref(), "Focus; Raise");
        assert_eq!(filter(&rows, "raise focus"), [0]);
    }

    #[test]
    fn search_keeps_duplicate_rows_and_alternate_chords_report_shell_conflicts() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("wm.toml");
        std::fs::write(&file, "[keybinds]\ncycle_focus=['Super+Tab','Alt+Tab']\n[keybinds.custom]\n'Super+Q'='close'\n").unwrap();
        let session = Session {
            environment: Default::default(),
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
