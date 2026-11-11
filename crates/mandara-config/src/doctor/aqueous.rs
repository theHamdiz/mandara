//! Aqueous validates its own TOML files when it hot-reloads them and has no
//! standalone configuration checker. Mandara writes nothing it reads, so
//! the doctor only reports whether it is installed (`aqueous -version`
//! prints its version and exits before touching a display).

use std::ffi::OsString;
use std::path::Path;

use super::{CompositorCheck, Diagnostic, Output, ParseContext, Severity};
use crate::compositor::Target;

pub struct Aqueous;

impl CompositorCheck for Aqueous {
    fn target(&self) -> Target { Target::Aqueous }
    fn program(&self) -> &'static str { "aqueous" }
    fn args(&self, _: &Path) -> Vec<OsString> { vec!["-version".into()] }
    fn needs_config(&self) -> bool { false }
    fn version_flag(&self) -> &'static str { "-version" }
    fn parse(&self, out: &Output, _: &ParseContext) -> Vec<Diagnostic> {
        if !out.success { return Vec::new(); }
        let mut info = Diagnostic::new(Severity::Info, mandara_i18n::t("Aqueous has no standalone configuration checker. It validates its own files in ~/.config/aqueous when it reloads them, and Mandara writes nothing there."));
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
        assert_eq!(Aqueous.args(Path::new("/untouched/wm.toml")), [OsString::from("-version")]);
        assert_eq!(Aqueous.version_flag(), "-version");
        assert_eq!(Aqueous.line_key("[decoration]"), None);
        let ctx = ParseContext::new(Target::Aqueous, Path::new("/untouched/wm.toml"));
        let info = Aqueous.parse(&Output { stdout: "aqueous 0.4.0 +xwayland".into(), stderr: String::new(), success: true }, &ctx);
        assert_eq!(info.len(), 1);
        assert_eq!(info[0].severity, Severity::Info);
        assert!(info[0].read_only);
        assert!(Aqueous.parse(&Output { stdout: String::new(), stderr: "failed".into(), success: false }, &ctx).is_empty());
        assert_eq!(crate::compositor::parse_version("aqueous 0.4.0 +xwayland"), Some((0, 4)));
    }

    #[test]
    fn availability_check_runs_without_any_aqueous_config() {
        use super::super::{CheckEnv, CheckerStatus, check_with, explain};
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("missing/wm.toml");
        let env = CheckEnv { argv: Some(vec!["printf".into(), "aqueous 0.4.0".into()]), version: Some("0.4.0".into()), ..Default::default() };
        let report = check_with(Target::Aqueous, &root, &env);
        assert_eq!(report.status, CheckerStatus::Ran);
        assert!(!root.exists());
        assert_eq!(report.diagnostics.len(), 1);
        assert_eq!(report.problems(), 0, "an availability note must not trigger config reminders");
        assert_eq!(explain(&report.diagnostics[0], Target::Aqueous, report.version.as_deref()), report.diagnostics[0].message);
    }

    #[test]
    fn mandara_never_writes_an_aqueous_drop_in() {
        assert!(Target::Aqueous.self_configured() && Target::KWin.self_configured());
        assert!(Target::ALL.iter().filter(|t| t.self_configured()).count() == 4);
        assert_eq!(Target::parse("Aqueous"), Some(Target::Aqueous));
        assert_eq!(Target::Aqueous.include_line(), "");
        assert!(Target::Aqueous.user_config().ends_with("aqueous/wm.toml"));
        let emitted = crate::compositor::Emitted { text: "# nothing\n".into(), skipped: Vec::new() };
        assert!(crate::compositor::write_generated(Target::Aqueous, &emitted).is_err());
        assert!(!Target::Aqueous.generated_file().exists());
        assert!(crate::compositor::include_installed(Target::Aqueous), "nothing to include");
        assert!(!crate::compositor::user_autostarts(Target::Aqueous));
        assert!(crate::compositor::RULE_OPTIONS.iter().all(|(_, s)| !s[Target::Aqueous.index()]));
        assert!(crate::input::INPUT_OPTIONS.iter().all(|(_, s)| !s[Target::Aqueous.index()]));
    }
}
