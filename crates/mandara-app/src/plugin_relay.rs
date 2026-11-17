//! Studio's bounded broker transport to the shell's existing IPC server.

use std::path::PathBuf;
use std::sync::Arc;

use mandara_plugins::live_wallpaper::Request;
use serde_json::Value;

fn arguments(request: &Request) -> Result<Vec<String>, String> {
    let json = serde_json::to_string(request).map_err(|e| e.to_string())?;
    Request::parse(&json)?;
    Ok(vec!["plugins".into(), "live".into(), json])
}

pub fn connect(socket: PathBuf, cx: &mut gpui::App) {
    cx.set_global(mandara_ui::plugins::external::ShellRelay(Arc::new(move |request| {
        match crate::ipc::send(&socket, &arguments(request)?) {
            Ok(reply) if reply.ok => Ok(reply.data.unwrap_or(Value::Null)),
            Ok(reply) => Err(reply.error.unwrap_or_default()),
            Err(error) => Err(format!("{error:#}")),
        }
    })));
}

#[cfg(test)]
mod tests {
    use super::*;
    use mandara_plugins::live_wallpaper::Action;

    #[test]
    fn paths_stay_inside_one_json_argument_to_a_fixed_command() {
        let request = Request { generation: 7, plugin: "workshop".into(), action: Action::Render { root: "/approved folder".into(), project: "/approved folder/\"project\"\n123".into(), output: Some("DP-1".into()) } };
        let arguments = arguments(&request).unwrap();
        assert_eq!(&arguments[..2], ["plugins", "live"]);
        assert_eq!(arguments.len(), 3);
        assert_eq!(Request::parse(&arguments[2]).unwrap(), request);
    }
}
