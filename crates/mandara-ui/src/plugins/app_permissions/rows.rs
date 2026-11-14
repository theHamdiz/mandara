//! App labels and the search index are built once per installed-app snapshot.

use gpui::SharedString;
use super::InstalledApp;

pub(super) struct Row {
    pub app: InstalledApp,
    pub id: SharedString,
    pub name: SharedString,
    pub subtitle: SharedString,
    pub initials: SharedString,
    search: String,
}
impl Row {
    pub fn new(app: InstalledApp) -> Self {
        let id = format!("permission-app-{}-{}", app.id, app.installation).into();
        let name = app.name.clone().into();
        let subtitle = format!("{} · {}", app.id, app.installation).into();
        let initials: String = app.name.split_whitespace().take(2).filter_map(|part| part.chars().next()).flat_map(char::to_uppercase).collect();
        let search = format!("{} {} {}", app.name, app.id, app.installation).to_lowercase();
        Self { app, id, name, subtitle, initials: initials.into(), search }
    }
    pub fn matches(&self, query: &str) -> bool { self.search.contains(query) }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn searches_reach_every_app_and_keep_its_installation_identity() {
        let row = Row::new(InstalledApp { id: "io.github.example.Music".into(), name: "Music Box".into(), installation: "user".into() });
        assert!(row.matches("music"));
        assert!(row.matches("io.github.example"));
        assert!(row.matches("user"));
        assert!(!row.matches("system"));
        assert_eq!(row.initials.as_ref(), "MB");
    }
}
