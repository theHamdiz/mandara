//! Cleanup commands retain one concrete manifest and return its paths in bounded pages.

use std::path::PathBuf;
use gpui::App;
use mandara_i18n::{t, tk};
use mandara_plugins::{ArgKind, ArgSpec, CommandSpec, Output, PluginRegistry};
use super::*;

#[derive(serde::Serialize)]
struct Prepared {
    #[serde(skip)]
    preview: Arc<Preview>,
    path: PathBuf,
    files: usize,
    logical_bytes: u64,
    left_untouched: usize,
    age_days: u64,
}

fn prepare(model: &Shared, path: PathBuf, cx: &mut App) -> Result<async_channel::Receiver<Result<String, String>>, String> {
    if model.borrow().active.is_some() { return Err(t("An action is still running").into()); }
    let age_days = super::super::preferences(cx).1.cleanup_days;
    model.borrow_mut().cleanup.clear();
    let generation = model.borrow().cleanup.review_generation;
    jobs::perform(model, tk("Preparing the cleanup preview…"), move |cancel| {
        let candidate = api::candidates().map_err(|error| format!("{error:#}"))?.into_iter()
            .find(|candidate| candidate.path == path)
            .ok_or_else(|| t("Choose a current path from backups cleanup locations.").to_owned())?;
        let preview = Arc::new(api::preview(&candidate, age_days, &cancel).map_err(|error| format!("{error:#}"))?);
        Ok(Prepared { path, files: preview.file_count(), logical_bytes: preview.logical_bytes, left_untouched: preview.left_untouched, preview, age_days })
    }, move |prepared, model, _| {
        let mut state = model.borrow_mut();
        if state.cleanup.review_generation != generation { return; }
        state.select_page(Page::Cleanup);
        state.cleanup.selected = Some(prepared.preview.candidate.clone());
        state.cleanup.command_preview = Some(prepared.preview);
    }, cx)
}

fn reviewed(model: &Shared) -> Result<Arc<Preview>, String> {
    let state = model.borrow();
    state.cleanup.command_preview.clone().or_else(|| state.cleanup.preview.borrow().value.clone())
        .ok_or_else(|| t("Prepare and review a cleanup preview first.").into())
}

fn file_page<'a>(base: &std::path::Path, files: impl Iterator<Item = (&'a std::path::Path, u64)>, offset: usize) -> Vec<serde_json::Value> {
    files.skip(offset).take(PATHS_PER_PAGE).map(|(path, bytes)| serde_json::json!({"path":base.join(path),"logical_bytes":bytes})).collect()
}

fn page(preview: &Preview, offset: usize) -> serde_json::Value {
    let files = file_page(preview.base_directory(), preview.files(), offset);
    let end = offset.saturating_add(files.len());
    serde_json::json!({"path":preview.candidate.path,"total_files":preview.file_count(),"logical_bytes":preview.logical_bytes,"left_untouched":preview.left_untouched,"offset":offset,"next_offset":(end < preview.file_count()).then_some(end),"files":files,"permanent":true})
}

pub(in crate::plugins::backups) fn register(registry: &mut PluginRegistry) {
    registry.command(super::super::ID, CommandSpec::new_output("backups cleanup locations", tk("Current cache and Trash items eligible for a reviewed cleanup"), |_, cx| {
        jobs::perform(&super::super::model(), tk("Reading cleanup locations…"), |_| api::candidates().map(|items| serde_json::json!(items.into_iter().map(|item| serde_json::json!({"name":item.name,"path":item.path,"kind":match item.kind { Kind::Cache => "cache", Kind::Trash => "trash" }})).collect::<Vec<_>>())).map_err(|error| format!("{error:#}")), |_, _, _| {}, cx).map(Output::Pending)
    }));
    registry.command(super::super::ID, CommandSpec::new_output("backups cleanup preview", tk("Prepare an exact cleanup manifest using the configured minimum age"), |args, cx| {
        let path = args.str(0)?;
        if path.len() > mandara_services::backups::MAX_PATH_BYTES { return Err(t("The cleanup path is too long.").into()); }
        prepare(&super::super::model(), PathBuf::from(path), cx).map(Output::Pending)
    }).arg(ArgSpec::required("path", ArgKind::String, tk("Exact path returned by backups cleanup locations"))));
    registry.command(super::super::ID, CommandSpec::new("backups cleanup review", tk("Read the exact reviewed paths in bounded pages"), |args, _| {
        let offset = if args.0.is_empty() { 0 } else { usize::try_from(args.int(0)?).map_err(|_| t("Choose a non-negative file offset.").to_owned())? };
        let preview = reviewed(&super::super::model())?;
        Ok(page(&preview, offset))
    }).arg(ArgSpec::optional("offset", ArgKind::Integer, tk("File offset; use next_offset for the next page"))));
    registry.command(super::super::ID, CommandSpec::new_output("backups cleanup confirm", tk("Permanently remove only files in the last reviewed cleanup manifest"), |_, cx| {
        let model = super::super::model();
        let preview = reviewed(&model)?;
        remove_output(&model, preview, cx).map(Output::Pending)
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancelled_closed_or_abandoned_previews_cannot_restore_a_review() {
        let shared = Rc::new(RefCell::new(super::super::super::Model::default()));
        let generation = shared.borrow().cleanup.review_generation;
        cancel_review(&shared);
        assert_ne!(shared.borrow().cleanup.review_generation, generation);
        let generation = shared.borrow().cleanup.review_generation;
        shared.borrow_mut().select_page(Page::Snapshots);
        assert_ne!(shared.borrow().cleanup.review_generation, generation, "navigation invalidates a CLI preview even when it started outside Cleanup");
        shared.borrow_mut().select_page(Page::Cleanup);
        let generation = shared.borrow().cleanup.review_generation;
        shared.borrow_mut().select_page(Page::Cleanup);
        assert_eq!(shared.borrow().cleanup.review_generation, generation, "rendering the same page keeps a current review");
        shared.borrow_mut().cleanup.clear();
        assert_ne!(shared.borrow().cleanup.review_generation, generation, "closing releases the review and rejects its late completion");
        assert!(reviewed(&shared).is_err());
    }
    #[test]
    fn a_cleanup_confirmation_requires_a_retained_manifest() {
        let model = Rc::new(RefCell::new(super::super::super::Model::default()));
        assert!(reviewed(&model).is_err());
        cancel_review(&model);
        assert!(reviewed(&model).is_err());
    }

    #[test]
    fn cleanup_review_pages_preserve_every_path_without_an_unbounded_reply() {
        let paths = (0..30).map(|index| (PathBuf::from(format!("العربية/{index}")), index)).collect::<Vec<_>>();
        let base = std::path::Path::new("/cache");
        let read = |offset| file_page(base, paths.iter().map(|(path, bytes)| (path.as_path(), *bytes)), offset);
        assert_eq!(read(0).len(), PATHS_PER_PAGE);
        assert_eq!(read(PATHS_PER_PAGE)[0]["logical_bytes"], 12);
        assert_eq!(read(24).len(), 6);
        assert_eq!(read(24)[5]["path"], "/cache/العربية/29");
        assert!(read(usize::MAX).is_empty());
        assert!(file_page(base, std::iter::empty(), 0).is_empty());
    }
}
