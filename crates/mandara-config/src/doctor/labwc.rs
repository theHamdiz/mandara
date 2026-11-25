//! labwc reads `~/.config/labwc/rc.xml` itself and has no standalone
//! configuration checker. The doctor reports availability; imported theme
//! colors have their own private headless validation before a key edit.

use std::ffi::OsString;
use std::path::Path;

use super::{CompositorCheck, Diagnostic, Output, ParseContext, Severity};
use crate::compositor::Target;

pub struct Labwc;

/// The availability note shown instead of "All good".
pub const NOTE: &str = mandara_i18n::tk("labwc has no standalone configuration checker. Theme colours use a private headless check.");

impl CompositorCheck for Labwc {
    fn target(&self) -> Target { Target::Labwc }
    fn program(&self) -> &'static str { "labwc" }
    fn args(&self, _: &Path) -> Vec<OsString> { vec!["--version".into()] }
    fn needs_config(&self) -> bool { false }
    fn parse(&self, out: &Output, _: &ParseContext) -> Vec<Diagnostic> {
        if !out.success { return Vec::new(); }
        let mut info = Diagnostic::new(Severity::Info, mandara_i18n::t(NOTE));
        info.read_only = true;
        vec![info]
    }
    fn line_key(&self, _: &str) -> Option<String> { None }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn availability_check_never_starts_a_compositor_or_reads_its_files() {
        assert_eq!(Labwc.args(Path::new("/untouched/rc.xml")), [OsString::from("--version")]);
        assert_eq!(Labwc.line_key("<keybind key=\"W-Return\">"), None);
        let ctx = ParseContext::new(Target::Labwc, Path::new("/untouched/rc.xml"));
        let info = Labwc.parse(&Output { stdout: "labwc 0.20.2 (+xwayland +nls +rsvg +libsfdo) wlroots-0.20.2".into(), stderr: String::new(), success: true }, &ctx);
        assert_eq!(info.len(), 1);
        assert_eq!(info[0].severity, Severity::Info);
        assert!(info[0].read_only);
        assert!(Labwc.parse(&Output { stdout: String::new(), stderr: "failed".into(), success: false }, &ctx).is_empty());
        assert_eq!(crate::compositor::parse_version("labwc 0.20.2 (+xwayland +nls +rsvg +libsfdo) wlroots-0.20.2"), Some((0, 20)));
    }

    #[test]
    fn availability_check_runs_without_any_labwc_config() {
        use super::super::{CheckEnv, CheckerStatus, check_with, explain};
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("missing/rc.xml");
        let env = CheckEnv { argv: Some(vec!["printf".into(), "labwc 0.20.2".into()]), version: Some("0.20.2".into()), ..Default::default() };
        let report = check_with(Target::Labwc, &root, &env);
        assert_eq!(report.status, CheckerStatus::Ran);
        assert!(!root.exists());
        assert_eq!(report.diagnostics.len(), 1);
        assert_eq!(report.problems(), 0, "an availability note must not trigger config reminders");
        assert_eq!(explain(&report.diagnostics[0], Target::Labwc, report.version.as_deref()), report.diagnostics[0].message);
    }
}
