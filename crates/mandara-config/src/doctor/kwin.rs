//! KWin has no include fragments or command-line configuration validator.
//! Its settings stay in Plasma; the doctor reports availability only.

use std::ffi::OsString;
use std::path::Path;

use super::{CompositorCheck, Diagnostic, Output, ParseContext, Severity};
use crate::compositor::Target;

pub struct KWin;

impl CompositorCheck for KWin {
    fn target(&self) -> Target { Target::KWin }
    fn program(&self) -> &'static str { "kwin_wayland" }
    fn args(&self, _: &Path) -> Vec<OsString> { vec!["--version".into()] }
    fn needs_config(&self) -> bool { false }
    fn parse(&self, out: &Output, _: &ParseContext) -> Vec<Diagnostic> {
        if !out.success { return Vec::new(); }
        let mut info = Diagnostic::new(Severity::Info, mandara_i18n::t("KWin has no standalone configuration checker. Its settings are managed in Plasma System Settings."));
        info.read_only = true;
        vec![info]
    }
    fn line_key(&self, _: &str) -> Option<String> { None }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn availability_check_never_starts_a_compositor_or_reads_kwinrc() {
        assert_eq!(KWin.args(Path::new("/untouched/kwinrc")), [OsString::from("--version")]);
        assert_eq!(KWin.line_key("[Desktops]"), None);
        let ctx = ParseContext::new(Target::KWin, Path::new("/untouched/kwinrc"));
        let info = KWin.parse(&Output { stdout: "kwin 6.8.2".into(), stderr: String::new(), success: true }, &ctx);
        assert_eq!(info.len(), 1);
        assert_eq!(info[0].severity, Severity::Info);
        assert!(info[0].read_only);
        assert!(KWin.parse(&Output { stdout: String::new(), stderr: "failed".into(), success: false }, &ctx).is_empty());
    }

    #[test]
    fn availability_check_runs_without_a_kwinrc_or_config_directory() {
        use super::super::{CheckEnv, CheckerStatus, check_with, explain};
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("missing/kwinrc");
        let env = CheckEnv { argv: Some(vec!["printf".into(), "kwin 6.8.2".into()]), version: Some("6.8.2".into()), ..Default::default() };
        let report = check_with(Target::KWin, &root, &env);
        assert_eq!(report.status, CheckerStatus::Ran);
        assert!(!root.exists());
        assert_eq!(report.diagnostics.len(), 1);
        assert_eq!(report.problems(), 0, "an availability note must not trigger config reminders");
        assert_eq!(explain(&report.diagnostics[0], Target::KWin, report.version.as_deref()), report.diagnostics[0].message);
    }
}
