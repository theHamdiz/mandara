//! Typed Studio-to-shell requests. The shell rechecks its own live grants.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::extension::{Access, Capability, MAX_PAYLOAD_BYTES, valid_name};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub plugin: String,
    pub generation: u64,
    pub action: Action,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Action {
    Status {},
    Render { root: PathBuf, project: PathBuf, output: Option<String> },
    Stop { output: Option<String> },
}

impl Request {
    pub fn parse(json: &str) -> Result<Self, String> {
        if json.len() > MAX_PAYLOAD_BYTES { return Err("the live wallpaper request is too large".into()); }
        let request: Self = serde_json::from_str(json).map_err(|e| e.to_string())?;
        if !valid_name(&request.plugin) { return Err("invalid plugin id".into()); }
        Ok(request)
    }

    /// Path containment and the renderer are checked separately on a blocking worker.
    pub fn authorize(&self, access: &Access, enabled: bool) -> Result<(), String> {
        if !enabled { return Err("the plugin is switched off".into()); }
        if self.generation != access.generation { return Err("the plugin's permissions changed; try again".into()); }
        let operation = match self.action { Action::Status { .. } => "status", Action::Render { .. } => "render", Action::Stop { .. } => "stop" };
        if let Some(reason) = access.refuse(&Capability::WallpaperLive, operation) { return Err(reason); }
        if let Action::Render { root, project, .. } = &self.action {
            if !access.allows_folder(root) { return Err("that folder is no longer granted".into()); }
            if !root.is_absolute() || !project.is_absolute() || project == root {
                return Err("the project is outside the granted folder".into());
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extension::Grant;

    fn access(capabilities: &[&str], folders: &[&str]) -> Access {
        Access::from_grant(&["wallpaper.live".into(), "files.folders".into()], Some(&Grant { generation: 0, capabilities: capabilities.iter().map(|c| (*c).into()).collect(), folders: folders.iter().map(PathBuf::from).collect() }))
    }

    #[test]
    fn studio_requests_need_current_grants_and_absolute_project_paths() {
        let request = Request { generation: 0, plugin: "workshop".into(), action: Action::Render { root: "/approved".into(), project: "/approved/123".into(), output: Some("DP-1".into()) } };
        let granted = access(&["wallpaper.live", "files.folders"], &["/approved"]);
        assert!(request.authorize(&granted, true).is_ok());
        let mut restored = granted.clone();
        restored.generation = 2;
        assert!(request.authorize(&restored, true).is_err());
        assert!(Request { generation: 2, ..request.clone() }.authorize(&restored, true).is_ok());
        restored.generation = Grant::EXHAUSTED_GENERATION;
        assert!(Request { generation: restored.generation, ..request.clone() }.authorize(&restored, true).is_err());
        assert!(request.authorize(&granted, false).is_err());
        assert!(request.authorize(&access(&["files.folders"], &["/approved"]), true).is_err());
        assert!(request.authorize(&access(&["wallpaper.live"], &["/approved"]), true).is_err());
        assert!(request.authorize(&access(&["wallpaper.live", "files.folders"], &["/other"]), true).is_err());
        let outside = Request { action: Action::Render { root: "/approved".into(), project: "relative/project".into(), output: None }, ..request };
        assert!(outside.authorize(&granted, true).is_err());
    }

    #[test]
    fn wire_requests_cannot_supply_programs_or_unbounded_payloads() {
        let status = r#"{"plugin":"workshop","generation":0,"action":{"operation":"status"}}"#;
        assert!(matches!(Request::parse(status).unwrap().action, Action::Status { .. }));
        assert!(Request::parse(r#"{"plugin":"workshop","generation":0,"action":{"operation":"status","program":"anything"}}"#).is_err());
        assert!(Request::parse(r#"{"plugin":"../other","generation":0,"action":{"operation":"status"}}"#).is_err());
        assert!(Request::parse(&" ".repeat(MAX_PAYLOAD_BYTES + 1)).is_err());
        assert!(!crate::wasm::PLUGIN_COMMANDS.iter().any(|command| command.starts_with("plugins")), "the guest command API must not become a relay tunnel");
    }
}
