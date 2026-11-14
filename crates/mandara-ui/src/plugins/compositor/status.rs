//! Inspect filesystem-backed startup state away from the UI thread.

use mandara_compositor::CompositorKind;
use mandara_plugins::Output;
use serde_json::json;

pub(super) fn pending(kind: CompositorKind) -> Output {
    super::pending(move || {
        let target = kind.target();
        Ok(json!({
            "compositor": kind,
            "user_config": target.user_config(),
            "generated": target.generated_file(),
            "generated_exists": target.generated_file().exists(),
            "included": mandara_config::compositor::include_installed(target),
            "include_line": target.include_line(),
        }))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_defers_inspection_and_preserves_the_command_reply() {
        let Output::Pending(reply) = pending(CompositorKind::Cosmic) else {
            panic!("filesystem inspection must not run in the command handler");
        };
        let body = reply.recv_blocking().unwrap().unwrap();
        let report: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(report["compositor"], "Cosmic");
        assert_eq!(report["included"], true, "COSMIC needs no config include");
        assert!(report["user_config"].is_string());
        assert!(report["generated"].is_string());
        assert!(report["generated_exists"].is_boolean());
        assert!(report["include_line"].is_string());
    }
}
