//! COSMIC keeps its settings in COSMIC Settings (RON files under
//! `~/.config/cosmic`) and has no standalone configuration checker.
//! Mandara writes nothing it reads, so the doctor only reports whether
//! cosmic-comp is installed (`cosmic-comp --version` prints its version and
//! exits before touching a display).

use std::ffi::OsString;
use std::path::Path;

use super::{CompositorCheck, Diagnostic, Output, ParseContext, Severity};
use crate::compositor::Target;

pub struct Cosmic;

/// The availability note shown instead of "All good".
pub const NOTE: &str = mandara_i18n::tk("COSMIC has no standalone configuration checker. Its settings are managed in COSMIC Settings, and Mandara writes nothing there.");

impl CompositorCheck for Cosmic {
    fn target(&self) -> Target { Target::Cosmic }
    fn program(&self) -> &'static str { "cosmic-comp" }
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
        assert_eq!(Cosmic.args(Path::new("/untouched/v1")), [OsString::from("--version")]);
        assert_eq!(Cosmic.line_key("workspaces: (workspace_mode: OutputBound)"), None);
        let ctx = ParseContext::new(Target::Cosmic, Path::new("/untouched/v1"));
        let info = Cosmic.parse(&Output { stdout: "cosmic-comp 1.0.0 (git commit 8cb1802)".into(), stderr: String::new(), success: true }, &ctx);
        assert_eq!(info.len(), 1);
        assert!(info[0].read_only && info[0].severity == Severity::Info);
        assert!(Cosmic.parse(&Output { stdout: String::new(), stderr: "failed".into(), success: false }, &ctx).is_empty());
        assert_eq!(crate::compositor::parse_version("cosmic-comp 1.0.0 (git commit 8cb1802)"), Some((1, 0)));
    }

    #[test]
    fn mandara_never_writes_a_labwc_or_cosmic_drop_in() {
        for target in [Target::Labwc, Target::Cosmic] {
            assert!(target.self_configured());
            assert!(target.settings_elsewhere().is_some());
            assert_eq!(Target::parse(target.id()), Some(target));
            assert_eq!(target.include_line(), "");
            let emitted = crate::compositor::Emitted { text: "# nothing\n".into(), skipped: Vec::new() };
            assert!(crate::compositor::write_generated(target, &emitted).is_err());
            assert!(!target.generated_file().exists());
            assert!(crate::compositor::include_installed(target), "nothing to include");
            assert!(!crate::compositor::user_autostarts(target));
            assert!(crate::compositor::RULE_OPTIONS.iter().all(|(_, s)| !s[target.index()]));
            assert!(crate::input::INPUT_OPTIONS.iter().all(|(_, s)| !s[target.index()]));
        }
        assert_eq!(Target::parse("cosmic-comp"), Some(Target::Cosmic));
        assert_eq!(Target::ALL.iter().filter(|t| t.self_configured()).count(), 4);
        assert_eq!(Target::ALL.iter().map(|t| t.index()).collect::<Vec<_>>(), (0..Target::ALL.len()).collect::<Vec<_>>());
    }
}
