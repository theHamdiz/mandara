//! Opens files, folders and the Trash with the user's own apps, the way
//! [`mandara_config::open`] decides: a file with the app set for its type
//! (Studio → Applications → Defaults first), a folder and the Trash in the
//! file manager. The app's own `Exec` line is launched with the launcher's
//! options (wrapper, terminal, systemd scope); nothing goes through
//! `xdg-open` or the desktop portal. The lookup and the launch run off the
//! UI thread, and a failure is posted as a notification.

use std::path::{Path, PathBuf};

use gpui::App;
use mandara_config::open::{self, OpenEnv, OpenError, OpenPlan, OpenTarget};
use mandara_config::workflow::LaunchOptions;
use mandara_i18n::{t, tf};

use crate::state::config;

/// What to open.
enum Job {
    Path(PathBuf),
    Trash,
}

/// Why opening failed: what (a file name, or "Trash") and why.
#[derive(Debug, PartialEq)]
struct Failure {
    what: String,
    why: String,
}

/// Opens a file with the app for its type, or a folder in the file
/// manager.
pub fn open_path(path: &Path, cx: &mut App) {
    start(Job::Path(path.to_path_buf()), cx);
}

/// Opens the Trash in the file manager.
pub fn open_trash(cx: &mut App) {
    start(Job::Trash, cx);
}

fn start(job: Job, cx: &mut App) {
    let (file_manager, options) = {
        let cfg = config(cx).read(cx).get();
        (cfg.apps.file_manager.clone(), cfg.launch_options())
    };
    let task = cx.background_executor().spawn(async move { run(&job, &file_manager, &options) });
    cx.spawn(async move |cx| {
        if let Err(failure) = task.await {
            tracing::warn!("opening {}: {}", failure.what, failure.why);
            cx.update(|cx| {
                crate::plugins::notifications::send(
                    cx,
                    mandara_services::NotificationCommand::Post { app: t("Files").into(), icon: "folder".into(), summary: tf("Can't open {file}", &[("file", &failure.what)]), body: failure.why },
                );
            });
        }
    })
    .detach();
}

/// Decides and launches (blocking: it reads the MIME and app lists).
fn run(job: &Job, file_manager: &str, options: &LaunchOptions) -> Result<(), Failure> {
    let env = OpenEnv::system(crate::apps::application_dirs());
    let (what, plan) = match job {
        Job::Path(path) => (path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned()), open::plan_path(&env, path, file_manager, crate::apps::on_path)),
        Job::Trash => (t("Trash").to_string(), open::plan_trash(&env, file_manager, crate::apps::on_path, &open::trash_files_dir())),
    };
    let fail = |why: String| Failure { what: what.clone(), why };
    let plan = plan.map_err(|e| fail(reason(&e)))?;
    if let (Job::Trash, OpenTarget::Path(files)) = (job, &plan.target) {
        open::ensure_trash_files(files).map_err(|e| fail(tf("{path}: {error}", &[("path", &files.display()), ("error", &e)])))?;
    }
    crate::apps::launch_with(&app_for(&plan), None, options).map_err(fail)
}

/// The app `plan` launches, its `Exec` line filled with the target.
fn app_for(plan: &OpenPlan) -> crate::apps::App {
    let h = &plan.handler;
    let exec = match &plan.target {
        OpenTarget::Path(path) => crate::plugins::dock::model::exec_with_files(&h.exec, std::slice::from_ref(path)),
        OpenTarget::Uri(uri) => crate::plugins::dock::model::exec_with_uri(&h.exec, uri),
    };
    let id = if h.id.is_empty() { h.program() } else { h.stem() };
    crate::apps::App { id: id.to_string(), name: h.name.clone(), exec, terminal: h.terminal, ..Default::default() }
}

/// What the notification says when nothing can open it.
fn reason(e: &OpenError) -> String {
    match e {
        OpenError::NoApp { mime } => tf("No app opens {type} files — choose one in Studio → Applications → Defaults", &[("type", mime)]),
        OpenError::NoFileManager => t("No file manager found — choose one in Studio → Applications").to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mandara_config::open::Handler;

    fn plan(id: &str, exec: &str, target: OpenTarget) -> OpenPlan {
        OpenPlan { handler: Handler { id: id.into(), name: "App".into(), exec: exec.into(), terminal: false }, target }
    }

    #[test]
    fn the_app_gets_its_own_exec_line_with_the_target() {
        let options = LaunchOptions::default();
        let argv = |p: &OpenPlan| {
            let app = app_for(p);
            crate::apps::launch_argv(&app.exec, app.terminal, &options)
        };
        let file = PathBuf::from("/home/u/Downloads/My notes.txt");
        let text = plan("org.gnome.TextEditor.desktop", "gnome-text-editor %U", OpenTarget::Path(file.clone()));
        assert_eq!(argv(&text), ["gnome-text-editor", "file:///home/u/Downloads/My%20notes.txt"]);
        assert_eq!(app_for(&text).id, "org.gnome.TextEditor");
        assert_eq!(argv(&plan("imv.desktop", "imv %F", OpenTarget::Path(file.clone()))), ["imv", "/home/u/Downloads/My notes.txt"]);
        assert_eq!(argv(&plan("dolphin.desktop", "dolphin %u", OpenTarget::Uri("trash:/".into()))), ["dolphin", "trash:/"]);
        assert_eq!(argv(&plan("nautilus.desktop", "nautilus --new-window %U", OpenTarget::Uri("trash:///".into()))), ["nautilus", "--new-window", "trash:///"]);
        let folder = PathBuf::from("/home/u/.local/share/Trash/files");
        assert_eq!(argv(&plan("com.system76.CosmicFiles.desktop", "cosmic-files %U", OpenTarget::Path(folder))), ["cosmic-files", "file:///home/u/.local/share/Trash/files"]);
        // A command from Studio → Applications → Preferred apps.
        let command = plan("", "yazi --cwd", OpenTarget::Path(file));
        assert_eq!(argv(&command), ["yazi", "--cwd", "/home/u/Downloads/My notes.txt"]);
        assert_eq!(app_for(&command).id, "yazi");
        // Terminal apps run in the terminal.
        let mut tui = plan("yazi.desktop", "yazi %u", OpenTarget::Path(PathBuf::from("/tmp")));
        tui.handler.terminal = true;
        let options = LaunchOptions { terminal: "foot".into(), ..LaunchOptions::default() };
        let app = app_for(&tui);
        assert_eq!(crate::apps::launch_argv(&app.exec, app.terminal, &options), ["foot", "-e", "yazi", "file:///tmp"]);
    }

    #[test]
    fn failures_say_what_to_do() {
        assert!(reason(&OpenError::NoApp { mime: "application/pdf".into() }).contains("application/pdf"));
        assert!(reason(&OpenError::NoFileManager).contains("Studio"));
    }
}
