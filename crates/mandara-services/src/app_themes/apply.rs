//! Ordered template application and the information needed to undo it later.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context as _, Result, ensure};
use mandara_config::app_themes::AppThemesConfig;
use mandara_theme::templates::custom::{
    Command, CustomColor, MAX_TEMPLATE_BYTES, Renderer, Template,
};
use mandara_theme::templates::{Ledger, Placement};
use mandara_theme::{Color, MandaraPalette};
use serde::{Deserialize, Serialize};

use super::{catalog, manifest::Manifest, paths::Locations, process};

/// Context owned by one settled theme job, never by a frame or a hidden view.
pub struct Theme {
    pub palette: MandaraPalette,
    pub other: MandaraPalette,
    pub font_family: String,
    pub image: PathBuf,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Undo {
    directory: PathBuf,
    command: Option<Command>,
    #[serde(default)]
    compositor: Option<super::compositor::Driver>,
    #[serde(default)]
    browser: bool,
    environment: Vec<(String, String)>,
}

#[derive(Default, Serialize, Deserialize)]
struct Applied {
    entries: BTreeMap<String, Undo>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Report {
    pub files: Vec<PathBuf>,
    pub errors: BTreeMap<String, String>,
    pub skipped: Vec<String>,
}

pub(crate) use super::file_io::read_text;

pub(super) fn protected_revisions(locations: &Locations) -> Result<Vec<PathBuf>> {
    if !super::file_io::present(&locations.applied())? {
        return Ok(Vec::new());
    }
    let applied: Applied = serde_json::from_str(&read_text(&locations.applied())?)?;
    Ok(applied
        .entries
        .into_values()
        .map(|entry| entry.directory)
        .collect())
}

fn store_applied(path: &Path,applied: &Applied) -> Result<()> {
    let bytes = serde_json::to_vec(applied)?;
    ensure!(bytes.len() <= MAX_TEMPLATE_BYTES,"applied app theme record is too large");
    mandara_config::store::write_atomic(path,&bytes)
}

fn renderer(
    theme: &Theme,
    colors: &BTreeMap<String, CustomColor>,
    base: &Path,
) -> Result<Renderer> {
    let mut render = Renderer::new(&theme.palette, &theme.other, colors)?;
    render.set("config_dir", base.to_string_lossy().as_ref());
    render.set(
        "config_file",
        base.join("template.toml").to_string_lossy().as_ref(),
    );
    render.set("image", theme.image.to_string_lossy().as_ref());
    render.set_font_family(&theme.font_family);
    Ok(render)
}

fn run(
    source: &str,
    render: &Renderer,
    base: &Path,
    environment: &[(String, String)],
    cancel: &AtomicBool,
) -> Result<()> {
    if !source.is_empty() {
        process::run(
            &render.render_command(source)?,
            base,
            environment,
            cancel,
            process::TIMEOUT,
        )?;
    }
    Ok(())
}

fn dynamic(
    source: &str,
    render: &Renderer,
    base: &Path,
    locations: &Locations,
    cancel: &AtomicBool,
) -> Result<Vec<PathBuf>> {
    let text = process::run(
        &render.render_command(source)?,
        base,
        &locations.environment(),
        cancel,
        process::TIMEOUT,
    )?;
    let paths = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| locations.resolve(base, Path::new(line)))
        .collect::<Result<Vec<_>>>()?;
    ensure!(
        paths.len() <= 32,
        "dynamic template returned too many paths"
    );
    Ok(paths)
}

fn closest(spec: &Template, render: &Renderer) -> Result<String> {
    if spec.compare_to.is_empty() || spec.colors_to_compare.is_empty() {
        return Ok(String::new());
    }
    let target = Color::from_hex(render.render(&spec.compare_to)?.trim())?;
    let mut nearest: Option<(&str, f32)> = None;
    for (name, hex) in &spec.colors_to_compare {
        let distance = mandara_theme::color::delta_e_ok(target, Color::from_hex(hex)?);
        if nearest.is_none_or(|(_, previous)| distance < previous) {
            nearest = Some((name, distance));
        }
    }
    Ok(nearest.map_or_else(String::new, |(name, _)| name.to_string()))
}

struct Package {
    root: PathBuf,
    manifest: Manifest,
    category: String,
    _lease: Option<std::sync::Arc<std::fs::File>>,
}

/// A caller serializes jobs. Superseding a job stops before its next entry;
/// a setup command already writing a config is allowed to finish.
pub fn sync(
    config: &AppThemesConfig,
    theme: &Theme,
    locations: &Locations,
    cancel: &AtomicBool,
    superseded: &dyn Fn() -> bool,
) -> Result<Report> {
    config.validate()?;
    // With every imported entry off, startup has nothing to load or write.
    if !config.any_enabled() && !super::file_io::present(&locations.applied())? {
        return Ok(Report::default());
    }
    let mut report = Report::default();
    let mut post_hooks = super::post_hooks::Hooks::new(cancel);
    let mut ledger = Ledger::read(&locations.ledger())?;
    let mut applied: Applied = match read_text(&locations.applied()) {
        Ok(text) => serde_json::from_str(&text).context("reading applied app themes")?,
        Err(error) if super::file_io::absent(&locations.applied(), &error) =>
        {
            Applied::default()
        }
        Err(error) => return Err(error),
    };
    let mut enabled: BTreeSet<&str> = config.enabled.iter().map(String::as_str).collect();
    if config.user.values().any(|entry| entry.enabled) {
        enabled.insert("user");
    }
    let retired = applied
        .entries
        .keys()
        .filter(|id| {
            id.split_once('/')
                .is_some_and(|(package, _)| !enabled.contains(package))
        })
        .cloned()
        .collect();
    ApplyContext {
        locations,
        hooks: config.run_hooks,
        cancel,
        ledger: &mut ledger,
        report: &mut report,
        applied: &mut applied,
        post_hooks: &mut post_hooks,
    }
    .retire(retired, superseded);
    // Load one package at a time. Bodies for the rest of the catalog stay on disk.
    for package_id in enabled {
        if cancel.load(Ordering::Acquire) || superseded() {
            break;
        }
        let package = if package_id == "user" {
            Package {
                root: locations.config.join("mandara"),
                category: String::new(),
                _lease: None,
                manifest: Manifest {
                    templates: config
                        .user
                        .iter()
                        .filter(|(_, entry)| entry.enabled)
                        .map(|(id, entry)| (id.clone(), entry.clone()))
                        .collect(),
                    ..Default::default()
                },
            }
        } else {
            match catalog::installed(&locations.catalogs(), package_id) {
                Ok(package) => {
                    if let Err(error) = super::approval::Review::new(package_id, &package, locations).and_then(|review| review.check(config)) {
                        report.errors.insert(package_id.into(), format!("{error:#}"));
                        continue;
                    }
                    Package { root: package.root, manifest: package.manifest, category: package.category, _lease: package.lease }
                },
                Err(error) => {
                    report.errors.insert(package_id.into(), error.to_string());
                    continue;
                }
            }
        };
        let retired = applied
            .entries
            .keys()
            .filter(|id| {
                id.split_once('/').is_some_and(|(parent, id)| {
                    parent == package_id && !package.manifest.templates.contains_key(id)
                })
            })
            .cloned()
            .collect();
        ApplyContext {
            locations,
            hooks: config.run_hooks,
            cancel,
            ledger: &mut ledger,
            report: &mut report,
            applied: &mut applied,
        post_hooks: &mut post_hooks,
        }
        .retire(retired, superseded);
        let mut colors = package.manifest.config.custom_colors.clone();
        colors.extend(config.custom_colors.clone());
        let mut render = match renderer(theme, &colors, &package.root) {
            Ok(render) => render,
            Err(error) => {
                report
                    .errors
                    .insert(package_id.into(), format!("{error:#}"));
                continue;
            }
        };
        let mut entries: Vec<_> = package.manifest.templates.iter().collect();
        entries.sort_by_key(|(_, entry)| entry.index);
        for (id, spec) in entries {
            if cancel.load(Ordering::Acquire) || superseded() {
                break;
            }
            let owner = format!("{package_id}/{id}");
            // Compositor setup has its own validated writer; imported scripts
            // must never replace an owner's compositor configuration.
            let result = multiple_clients(&package.manifest, spec, &package.root, locations)
                .and_then(|gated| {
                    ApplyContext {
                        locations,
                        hooks: config.run_hooks,
                        cancel,
                        ledger: &mut ledger,
                        report: &mut report,
                        applied: &mut applied,
                post_hooks: &mut post_hooks,
                    }
                    .entry(
                        &owner,
                        spec,
                        &mut render,
                        &package.root,
                        &package.category,
                        gated,
                        package._lease.clone(),
                        package_id != "user",
                    )
                });
            ledger.store(&locations.ledger())?;
            match result {
                Ok(true) => {}
                Ok(false) => report.skipped.push(owner),
                Err(error) => {
                    report.errors.insert(owner, format!("{error:#}"));
                }
            }
        }
    }
    post_hooks.barrier(&mut report);
    ledger.store(&locations.ledger())?;
    store_applied(&locations.applied(), &applied)?;
    report.errors.extend(ledger.take_errors());
    report.files.sort();
    report.files.dedup();
    Ok(report)
}

struct ApplyContext<'a, 'cancel> {
    locations: &'a Locations,
    hooks: bool,
    cancel: &'a AtomicBool,
    ledger: &'a mut Ledger,
    report: &'a mut Report,
    applied: &'a mut Applied,
    post_hooks: &'a mut super::post_hooks::Hooks<'cancel>,
}

fn multiple_clients(
    manifest: &Manifest,
    spec: &Template,
    base: &Path,
    locations: &Locations,
) -> Result<bool> {
    if spec.input_path.as_os_str().is_empty() {
        return Ok(false);
    }
    let mut roots = BTreeSet::new();
    for entry in manifest
        .templates
        .values()
        .filter(|entry| entry.input_path == spec.input_path)
    {
        for output in entry.output_path.iter() {
            if let Some(root) = locations.client_root(&locations.resolve(base, output)?) {
                roots.insert(root);
            }
        }
    }
    Ok(roots.len() > 1)
}

impl ApplyContext<'_, '_> {
    fn retire(&mut self, ids: Vec<String>, superseded: &dyn Fn() -> bool) {
        let Self {
            locations,
            hooks,
            cancel,
            ledger,
            report,
            applied,
            post_hooks,
        } = self;
        if !ids.is_empty() { post_hooks.barrier(report); }
        for id in ids {
            if cancel.load(Ordering::Acquire) || superseded() {
                break;
            }
            let undo = &applied.entries[&id];
            if let Some(target) = undo.compositor
                && let Err(error) = super::compositor::undo(&id,target,locations)
            {
                report
                    .errors
                    .insert(id, format!("undo compositor theme: {error:#}"));
                continue;
            }
            if undo.browser {
                match super::firefox::undo(&id, locations) {
                    Ok(files) => report.files.extend(files),
                    Err(error) => {
                        report
                            .errors
                            .insert(id, format!("undo browser CSS: {error:#}"));
                        continue;
                    }
                }
            }
            match super::post_action::undo(&id, locations) {
                Ok(files) => report.files.extend(files),
                Err(error) => {
                    report
                        .errors
                        .insert(id, format!("undo preferences: {error:#}"));
                    continue;
                }
            }
            if *hooks
                && let Some(command) = &undo.command
                && let Err(error) = process::run(
                    command,
                    &undo.directory,
                    &undo.environment,
                    cancel,
                    process::TIMEOUT,
                )
            {
                report.errors.insert(id, format!("undo: {error:#}"));
                continue;
            }
            let restored = ledger.take_back_checked(&id);
            let failed = !restored.errors.is_empty();
            report.files.extend(restored.files);
            report.errors.extend(restored.errors);
            if failed || ledger.has(&id) {
                report
                    .errors
                    .insert(id, "some app theme files could not be restored".into());
                continue;
            }
            if !*hooks && undo.command.is_some() {
                report.errors.insert(id, "undo command is pending while template commands are disabled".into());
                continue;
            }
            applied.entries.remove(&id);
        }
    }

    fn entry(
        &mut self,
        owner: &str,
        spec: &Template,
        render: &mut Renderer,
        base: &Path,
        category: &str,
        gated: bool,
        lease: Option<std::sync::Arc<std::fs::File>>,
        reviewed: bool,
    ) -> Result<bool> {
        let Self {
            locations,
            hooks,
            cancel,
            ledger,
            report,
            applied,
            post_hooks,
        } = self;
        let hooks = *hooks;
        let directory = spec
            .directory
            .as_ref()
            .map(|path| locations.resolve(base, path))
            .transpose()?;
        let base = directory.as_deref().unwrap_or(base);
        render.set("config_dir", base.to_string_lossy().as_ref());
        render.set(
            "config_file",
            base.join("template.toml").to_string_lossy().as_ref(),
        );
        let adapted = (category == "compositor")
            .then(|| super::compositor::prepare(owner, spec,locations))
            .transpose()?;
        let spec = adapted.as_ref().unwrap_or(spec);
        if let Some(required) = &spec.requires_path
            && !locations.resolve(base, required)?.exists()
        {
            return Ok(false);
        }
        render.set("closest_color", closest(spec, render)?);
        let mode = render.render("{{ mode }}")?;
        let mut input = spec
            .input_path_modes
            .get(&mode)
            .unwrap_or(&spec.input_path)
            .clone();
        if !spec.input_path_dynamic.is_empty() {
            ensure!(hooks, "dynamic input requires template commands");
            if let Some(path) = dynamic(&spec.input_path_dynamic, render, base, locations, cancel)?
                .into_iter()
                .next()
            {
                input = path;
            }
        }
        let mut outputs = spec
            .output_path
            .iter()
            .map(|path| locations.resolve(base, path))
            .collect::<Result<Vec<_>>>()?;
        if !spec.output_path_dynamic.is_empty() {
            ensure!(hooks, "dynamic output requires template commands");
            outputs.extend(dynamic(
                &spec.output_path_dynamic,
                render,
                base,
                locations,
                cancel,
            )?);
        }
        outputs.sort();
        outputs.dedup();
        if gated && spec.requires_path.is_none() {
            outputs.retain(|path| locations.client_root(path).is_none_or(|root| root.exists()));
        }
        let hook_only = outputs.is_empty() && spec.gtk_extension.is_none();
        if hook_only && spec.pre_hook.is_empty() && spec.post_hook.is_empty() {
            return Ok(false);
        }
        let body = if hook_only {
            String::new()
        } else {
            match &spec.content {
                Some(content) => render.render(content)?,
                None => render.render(&read_text(&locations.resolve(base, &input)?)?)?,
            }
        };
        let environment = locations.environment();
        let command = (hooks
            && (spec.post_action.is_empty() || owner.starts_with("user/"))
            && !spec.undo_hook.is_empty())
        .then(|| render.render_command(&spec.undo_hook))
        .transpose()?;
        if hooks && let Some(previous) = applied.entries.get(owner) {
            let changed = previous.directory != base
                || previous.command != command;
            if changed && let Some(command) = &previous.command {
                post_hooks.barrier(report);
                process::run(command,&previous.directory,&previous.environment,cancel,process::TIMEOUT)
                    .context("undoing the previous template setup before an update")?;
            }
        }
        if hooks || !applied.entries.contains_key(owner) {
            applied.entries.insert(
                owner.into(),
                Undo {
                    directory: base.to_path_buf(),
                    command,
                    environment: environment.clone(),
                    compositor: (category == "compositor")
                        .then(|| super::compositor::target(owner))
                        .transpose()?,
                    browser: spec.post_action == "firefox-theme",
                },
            );
        }
        store_applied(&locations.applied(), applied)?;
        ledger.store(&locations.ledger())?;
        if hooks {
            run(&spec.pre_hook, render, base, &environment, cancel)?;
        }
        let mut keep = outputs.clone();
        if category == "compositor" {
            let files = super::compositor::apply(owner, &body, locations,ledger)?;
            keep.extend(files.iter().cloned());
            report.files.extend(files);
        } else if let Some(extension) = &spec.gtk_extension {
            let version = match extension.as_str() {
                "gtk3" => "gtk-3.0",
                "gtk4" => "gtk-4.0",
                _ => anyhow::bail!("unknown GTK extension"),
            };
            let safe = owner.replace([':', '/'], "-");
            let path = locations
                .config
                .join(version)
                .join(format!("mandara-{safe}.css"));
            let import = locations.config.join(version).join("gtk.css");
            let line = format!(
                "@import url(\"{}\"); /* Mandara Shell: {safe} */",
                path.to_string_lossy()
                    .replace('\\', "\\\\")
                    .replace('"', "\\\"")
            );
            ledger.put_file(owner, &path, &body)?;
            ledger.put_block(owner, &import, &[&line], Placement::AfterImports)?;
            report.files.extend([path.clone(), import.clone()]);
            keep.extend([path, import]);
        } else {
            for path in &outputs {
                ledger.put_file(owner, path, &body)?;
                report.files.push(path.clone());
            }
        }
        ledger.store(&locations.ledger())?;
        if spec.post_action == "firefox-theme" {
            ensure!(
                outputs.len() == 1,
                "browser theme action needs one output file"
            );
            let files = super::firefox::apply(owner, &outputs[0], base, &mode, locations, ledger, reviewed)?;
            // The host manifest and settings remain managed alongside the
            // palette output until this entry is switched off.
            keep.extend(files.iter().cloned());
            report.files.extend(files);
            ledger.store(&locations.ledger())?;
        } else {
            super::post_action::run(&spec.post_action, owner, &outputs, locations)?;
        }
        if hooks && !spec.post_hook.is_empty() {
            if spec.hook_async {
                post_hooks.post(owner,render.render_command(&spec.post_hook)?,base,&environment,lease,report);
            } else {
                post_hooks.barrier(report);
                run(&spec.post_hook, render, base, &environment, cancel)?;
            }
        }
        let retired = ledger.take_back_except_checked(owner, &keep);
        report.files.extend(retired.files);
        ensure!(retired.errors.is_empty(), "old app theme destinations could not be restored: {:?}", retired.errors);
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mandara_theme::templates::custom::Paths;

    pub(super) fn locations(root: &Path) -> Locations {
        Locations {
            home: root.into(),
            config: root.join("config"),
            data: root.join("data"),
            cache: root.join("cache"),
            state: root.join("state"),
        }
    }
    fn theme() -> Theme {
        Theme {
            palette: mandara_theme::engine::default_palette(true),
            other: mandara_theme::engine::default_palette(false),
            font_family: "Test font".into(),
            image: "/images/a 'b' $(false).png".into(),
        }
    }

    #[test]
    fn dangling_applied_records_block_exports_and_cache_pruning() {
        let root = tempfile::tempdir().unwrap(); let locations = locations(root.path());
        std::fs::create_dir_all(locations.applied().parent().unwrap()).unwrap();
        let target = root.path().join("missing-ownership");
        std::os::unix::fs::symlink(&target, locations.applied()).unwrap();
        assert!(protected_revisions(&locations).is_err());
        assert!(sync(&AppThemesConfig::default(), &theme(), &locations, &AtomicBool::new(false), &||false).is_err());
        assert_eq!(std::fs::read_link(locations.applied()).unwrap(), target);
        assert!(!target.exists());
    }

    #[test]
    fn a_failed_retirement_is_reported_and_keeps_its_undo_record() {
        let root = tempfile::tempdir().unwrap();
        let locations = locations(root.path());
        let path = locations.home.join("old.css");
        std::fs::write(&path, "personal").unwrap();
        let mut config = AppThemesConfig { user: [("editor".into(), Template { enabled: true,
            content: Some("themed".into()), output_path: Paths::One(path.clone()), ..Default::default()
        })].into(), ..Default::default() };
        let cancel = AtomicBool::new(false);
        assert!(sync(&config, &theme(), &locations, &cancel, &||false).unwrap().errors.is_empty());
        let ledger = Ledger::read(&locations.ledger()).unwrap();
        let backup = ledger.files.values().next().unwrap().backup.as_ref().unwrap();
        std::fs::remove_file(backup).unwrap();
        config.user.get_mut("editor").unwrap().output_path = Paths::One(locations.home.join("new.css"));
        let report = sync(&config, &theme(), &locations, &cancel, &||false).unwrap();
        assert!(report.errors["user/editor"].contains("old app theme destinations"));
        assert!(Ledger::read(&locations.ledger()).unwrap().files.contains_key(path.to_str().unwrap()));
        assert_eq!(std::fs::read_to_string(path).unwrap(), "themed");
    }

    #[test]
    fn disabled_commands_keep_pending_hook_only_cleanup_for_a_later_retry() {
        let root = tempfile::tempdir().unwrap();
        let locations = locations(root.path());
        let mut config = AppThemesConfig { user: [("hook".into(), Template {
            enabled: true, post_hook: "printf applied > \"$HOME/hook\"".into(), undo_hook: "rm \"$HOME/hook\"".into(), ..Default::default()
        })].into(), ..Default::default() };
        let cancel = AtomicBool::new(false);
        assert!(sync(&config, &theme(), &locations, &cancel, &||false).unwrap().errors.is_empty());
        config.user.clear();
        config.run_hooks = false;
        let report = sync(&config, &theme(), &locations, &cancel, &||false).unwrap();
        assert!(!report.errors.is_empty());
        assert!(locations.home.join("hook").exists());
        let applied: Applied = serde_json::from_str(&read_text(&locations.applied()).unwrap()).unwrap();
        assert!(applied.entries.contains_key("user/hook"));
        config.run_hooks = true;
        assert!(sync(&config, &theme(), &locations, &cancel, &||false).unwrap().errors.is_empty());
        assert!(!locations.home.join("hook").exists());
    }

    #[test]
    fn changed_interpolated_undo_values_retire_the_previous_setup() {
        let dir = tempfile::tempdir().unwrap(); let locations = locations(dir.path());
        std::fs::create_dir_all(locations.config.join("mandara")).unwrap();
        let mut config = AppThemesConfig { user: BTreeMap::from([("setup".into(), Template { enabled: true, post_hook: "touch {{ 'old-setup' }}".into(), undo_hook: "rm -f {{ 'old-setup' }}".into(), ..Default::default() })]), ..Default::default() };
        let cancel = AtomicBool::new(false); let palette = theme();
        assert!(sync(&config, &palette, &locations, &cancel, &||false).unwrap().errors.is_empty());
        let base = locations.config.join("mandara"); assert!(base.join("old-setup").exists());
        let setup = config.user.get_mut("setup").unwrap(); setup.post_hook = "touch {{ 'new-setup' }}".into(); setup.undo_hook = "rm -f {{ 'new-setup' }}".into();
        assert!(sync(&config, &palette, &locations, &cancel, &||false).unwrap().errors.is_empty());
        assert!(!base.join("old-setup").exists()); assert!(base.join("new-setup").exists());
        config.user.clear(); assert!(sync(&config, &palette, &locations, &cancel, &||false).unwrap().errors.is_empty()); assert!(!base.join("new-setup").exists());
    }

    #[test]
    fn template_updates_undo_old_setup_and_sync_hooks_wait_for_async_predecessors() {
        let dir = tempfile::tempdir().unwrap();
        let locations = locations(dir.path());
        std::fs::create_dir_all(locations.config.join("mandara")).unwrap();
        let mut config = AppThemesConfig { user:BTreeMap::from([
            ("first".into(),Template { enabled:true,index:0,post_hook:"printf old > \"$HOME/old\"".into(),undo_hook:"rm -f \"$HOME/old\"".into(),..Default::default() }),
            ("second".into(),Template { enabled:true,index:1,hook_async:false,post_hook:"test -f \"$HOME/old\"".into(),..Default::default() }),
        ]),..Default::default() };
        let cancel = AtomicBool::new(false);
        assert!(sync(&config,&theme(),&locations,&cancel,&||false).unwrap().errors.is_empty());
        config.user.remove("second");
        let first = config.user.get_mut("first").unwrap();
        first.post_hook = "printf new > \"$HOME/new\"".into();
        first.undo_hook = "rm -f \"$HOME/new\"".into();
        assert!(sync(&config,&theme(),&locations,&cancel,&||false).unwrap().errors.is_empty());
        assert!(!locations.home.join("old").exists());
        assert!(locations.home.join("new").exists());
        config.user.clear();
        assert!(sync(&config,&theme(),&locations,&cancel,&||false).unwrap().errors.is_empty());
        assert!(!locations.home.join("new").exists());
    }

    #[test]
    fn hook_only_templates_apply_and_undo_without_a_dummy_output_file() {
        let dir = tempfile::tempdir().unwrap();
        let locations = locations(dir.path());
        std::fs::create_dir_all(locations.config.join("mandara")).unwrap();
        let spec = Template {
            enabled: true,
            post_hook: "printf 'applied' > \"$HOME/hook\"".into(),
            undo_hook: "rm \"$HOME/hook\"".into(),
            ..Default::default()
        };
        let mut config = AppThemesConfig {
            user: BTreeMap::from([("hook".into(), spec)]),
            ..Default::default()
        };
        let cancelled = AtomicBool::new(false);
        let report = sync(&config, &theme(), &locations, &cancelled, &|| false).unwrap();
        assert!(
            report.files.is_empty() && report.errors.is_empty(),
            "{:?}",
            report.errors
        );
        assert!(locations.home.join("hook").is_file());
        config.user.clear();
        sync(&config, &theme(), &locations, &cancelled, &|| false).unwrap();
        assert!(!locations.home.join("hook").exists());
    }

    #[test]
    fn edits_modes_multiple_destinations_and_disable_restore_the_original() {
        let dir = tempfile::tempdir().unwrap();
        let locations = locations(dir.path());
        let base = locations.config.join("mandara");
        std::fs::create_dir_all(&base).unwrap();
        std::fs::write(
            base.join("app.css"),
            "{{ colors.primary.default.hex }} {{ image }}",
        )
        .unwrap();
        let original = locations.config.join("editor/theme.css");
        std::fs::create_dir_all(original.parent().unwrap()).unwrap();
        std::fs::write(&original, "user's theme").unwrap();
        let spec = Template {
            enabled: true,
            input_path: "app.css".into(),
            output_path: Paths::Many(vec![original.clone(), "~/second.css".into()]),
            ..Default::default()
        };
        let mut config = AppThemesConfig {
            user: BTreeMap::from([("editor".into(), spec)]),
            ..Default::default()
        };
        let cancel = AtomicBool::new(false);
        let first = sync(&config, &theme(), &locations, &cancel, &|| false).unwrap();
        assert!(first.errors.is_empty(), "{:?}", first.errors);
        assert!(
            std::fs::read_to_string(&original)
                .unwrap()
                .contains("a 'b' $(false).png")
        );
        config.user.get_mut("editor").unwrap().output_path = Paths::One(original.clone());
        sync(&config, &theme(), &locations, &cancel, &|| false).unwrap();
        assert!(!dir.path().join("second.css").exists());
        config.user.clear();
        sync(&config, &theme(), &locations, &cancel, &|| false).unwrap();
        assert_eq!(std::fs::read_to_string(original).unwrap(), "user's theme");
    }

    #[test]
    fn dynamic_paths_hooks_and_gtk_extensions_are_applied_and_undone() {
        let dir = tempfile::tempdir().unwrap();
        let locations = locations(dir.path());
        let base = locations.config.join("mandara");
        std::fs::create_dir_all(&base).unwrap();
        std::fs::write(
            base.join("app.css"),
            "body { color: {{ colors.on_surface.default.hex }}; }",
        )
        .unwrap();
        let spec = Template {
            enabled: true,
            input_path: "app.css".into(),
            output_path_dynamic: "printf '%s/dynamic.css\\n' \"$HOME\"".into(),
            post_hook: "printf '%s' '{{ image }}' > \"$HOME/hook\"".into(),
            undo_hook: "rm \"$HOME/hook\"".into(),
            ..Default::default()
        };
        let gtk = Template {
            enabled: true,
            input_path: "app.css".into(),
            gtk_extension: Some("gtk3".into()),
            ..Default::default()
        };
        let mut config = AppThemesConfig {
            user: BTreeMap::from([("editor".into(), spec), ("gtk".into(), gtk)]),
            ..Default::default()
        };
        let cancel = AtomicBool::new(false);
        let report = sync(&config, &theme(), &locations, &cancel, &|| false).unwrap();
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        assert!(dir.path().join("dynamic.css").is_file());
        assert_eq!(
            std::fs::read_to_string(dir.path().join("hook")).unwrap(),
            theme().image.to_string_lossy()
        );
        assert!(
            std::fs::read_to_string(locations.config.join("gtk-3.0/gtk.css"))
                .unwrap()
                .contains("@import")
        );
        config.user.clear();
        assert!(
            sync(&config, &theme(), &locations, &cancel, &|| false)
                .unwrap()
                .errors
                .is_empty()
        );
        assert!(!dir.path().join("hook").exists() && !dir.path().join("dynamic.css").exists());
        assert!(!locations.config.join("gtk-3.0/gtk.css").exists());
    }
}

#[cfg(test)]
mod retirement_tests {
    use super::*;
    #[test]
    fn a_shared_recovery_failure_keeps_the_applied_undo_record() {
        let root = tempfile::tempdir().unwrap();
        let locations = super::tests::locations(root.path());
        let path = root.path().join("pending-theme");
        std::fs::write(&path, "newer personal edit").unwrap();
        let mut ledger: Ledger = serde_json::from_value(serde_json::json!({"pending":{
            "operation":"Apply", "path":path, "next":{"app":"unrelated","hash":"0000000000000000"},
            "previous":null,"original":{"device":0,"inode":0,"length":0,"seconds":0,"nanos":0},"moved":root.path().join("missing-backup")
        }})).unwrap();
        let owner = "user/example";
        let mut applied = Applied::default();
        applied.entries.insert(owner.into(), Undo { directory: root.path().into(), command:None, compositor:None,browser:false,environment:Vec::new() });
        let mut report = Report::default();
        let cancel = AtomicBool::new(false);
        let mut post_hooks = super::super::post_hooks::Hooks::new(&cancel);
        ApplyContext { locations:&locations,hooks:false,cancel:&cancel,ledger:&mut ledger,report:&mut report,applied:&mut applied,post_hooks:&mut post_hooks }.retire(vec![owner.into()], &|| false);
        assert!(!report.errors.is_empty());
        assert!(applied.entries.contains_key(owner));
        assert_eq!(std::fs::read_to_string(path).unwrap(), "newer personal edit");
    }
}
