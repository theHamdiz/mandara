//! Imported app templates and the user's own template document.

use std::rc::Rc;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use gpui::{
    AnyElement, AppContext as _, ClickEvent, Context, Entity, IntoElement, ParentElement,
    SharedString, StatefulInteractiveElement as _, Styled, Task, div, prelude::FluentBuilder, px,
};
use mandara_config::ShellConfig;
use mandara_core::ToGpui as _;
use mandara_i18n::{t, tf};
use mandara_services::app_themes::{catalog::Entry, jobs, manifest::Manifest, paths::Locations};
use mandara_theme::MandaraPalette;
use mandara_ui::state::update_config;

use super::{StudioView, compositor_page::button};
use crate::{text_field::TextField, tokens::*};

struct Job {
    cancel: Arc<AtomicBool>,
    _task: Task<()>,
}
impl Drop for Job {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
    }
}

struct Row {
    entry: Entry,
    search: String,
}

/// Bound card construction per frame while search still covers the whole catalog.
const CATALOG_ROWS_PER_PAGE: usize = 4;
/// Keep catalog cards aligned with Studio's compact app tiles.
const TEMPLATE_CARD_MIN_PX: f32 = 200.0;
const TEMPLATE_CARD_MAX_PX: f32 = 320.0;

#[derive(Default)]
pub(super) struct Templates {
    open: bool,
    rows: Arc<[Row]>,
    terms: Vec<String>,
    page: usize,
    search: Option<Entity<TextField>>,
    editor: Option<Entity<TextField>>,
    sources: Option<Entity<TextField>>,
    job: Option<Job>,
    busy: bool,
    error: Option<String>,
}

fn matches(row: &Row, terms: &[String]) -> bool {
    terms.iter().all(|term| row.search.contains(term))
}

fn editor_document(cfg: mandara_config::app_themes::AppThemesConfig) -> anyhow::Result<String> {
    let mut document = Manifest {
        templates: cfg.user,
        config: mandara_services::app_themes::manifest::Options {
            custom_colors: cfg.custom_colors,
        },
        ..Default::default()
    };
    if document.templates.is_empty() {
        document.templates = Manifest::parse_user("[templates.example]\nenabled = false\noutput_path = '~/.config/example/colors.css'\ncontent = '''\n:root { --accent: {{ colors.primary.default.hex }}; }\n'''\n")?.templates;
    }
    Ok(toml::to_string_pretty(&document)?)
}

impl StudioView {
    pub(super) fn app_templates(
        &mut self,
        cfg: &ShellConfig,
        p: &MandaraPalette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut content = div().flex().flex_col().gap(px(SPACE_12_PX));
        let open = self.app_templates.open;
        content = content.child(
            div()
                .flex()
                .flex_wrap()
                .gap(px(SPACE_8_PX))
                .child(
                    button(
                        "app-templates-browse",
                        "grid",
                        if open {
                            t("Hide app templates")
                        } else {
                            t("Browse app templates")
                        },
                        false,
                        p,
                    )
                    .on_click(cx.listener(
                        move |this, _: &ClickEvent, _, cx| {
                            if open {
                                this.app_templates = Templates::default();
                            } else {
                                this.app_templates.open = true;
                                this.load_app_templates(false, cx);
                            }
                            cx.notify();
                        },
                    )),
                )
                .child(
                    button(
                        "app-templates-sources",
                        "sliders",
                        t("Template sources"),
                        false,
                        p,
                    )
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        if this.app_templates.sources.is_some() {
                            this.app_templates.sources = None;
                        } else {
                            this.open_template_sources(cx);
                        }
                        cx.notify();
                    })),
                )
                .child(
                    button(
                        "app-templates-hooks",
                        "terminal",
                        t("Run template setup commands"),
                        cfg.integrations.app_themes.run_hooks,
                        p,
                    )
                    .on_click(cx.listener(|_, _: &ClickEvent, _, cx| {
                        update_config(cx, |config| {
                            config.integrations.app_themes.run_hooks =
                                !config.integrations.app_themes.run_hooks
                        });
                    })),
                )
                .child(
                    button(
                        "app-templates-edit",
                        "pencil",
                        t("Your templates"),
                        false,
                        p,
                    )
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        if this.app_templates.editor.is_some() {
                            this.app_templates.editor = None;
                        } else {
                            this.open_template_editor(cx);
                        }
                        cx.notify();
                    })),
                ),
        );
        if open {
            if self.app_templates.search.is_none() {
                let view = cx.entity().downgrade();
                self.app_templates.search = Some(cx.new(|cx| {
                    TextField::new(
                        String::new(),
                        t("Search app templates"),
                        true,
                        Rc::new(move |query, _, cx| {
                            let _ = view.update(cx, |this, cx| {
                                this.app_templates.terms =
                                    query.split_whitespace().map(str::to_lowercase).collect();
                                this.app_templates.page = 0;
                                cx.notify();
                            });
                        }),
                        cx,
                    )
                }));
            }
            content = content.child(
                div()
                    .flex()
                    .gap(px(SPACE_8_PX))
                    .child(
                        div()
                            .flex_1()
                            .child(self.app_templates.search.as_ref().unwrap().clone()),
                    )
                    .child(
                        button("app-templates-refresh", "refresh", t("Refresh"), false, p)
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                this.load_app_templates(true, cx)
                            })),
                    ),
            );
            content = content.child(div().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(t(
                "Imported templates can run app setup commands. Check their instructions before switching them on. Theme changes use installed files without downloading again.",
            )));
            if self.app_templates.busy {
                content = content.child(div().text_size(px(TEXT_12_PX)).child(t("Loading…")));
            }
            let rows = self.app_templates.rows.clone();
            let fit = super::grid::fit(
                self.page.column_w,
                CardGrid {
                    min_w: TEMPLATE_CARD_MIN_PX,
                    max_w: TEMPLATE_CARD_MAX_PX,
                    gap: SPACE_8_PX,
                },
            );
            let per_page = fit.cols * CATALOG_ROWS_PER_PAGE;
            let count = rows
                .iter()
                .filter(|row| matches(row, &self.app_templates.terms))
                .count();
            let pages = count.div_ceil(per_page).max(1);
            self.app_templates.page = self.app_templates.page.min(pages - 1);
            let page = self.app_templates.page;
            let cards: Vec<AnyElement> = rows
                .iter()
                .filter(|row| matches(row, &self.app_templates.terms))
                .skip(page * per_page)
                .take(per_page)
                .map(|row| {
                    let entry = row.entry.clone();
                    let id = entry.qualified_id();
                    let on = cfg.integrations.app_themes.enabled.contains(&id);
                    let busy = self.app_templates.busy;
                    let update_entry = entry.clone();
                    let update_id = SharedString::from(format!("app-template-update-{id}"));
                    let card = div()
                        .rounded(px(RADIUS_16_PX))
                        .p(px(SPACE_12_PX))
                        .bg(p.surface_container.hsla())
                        .flex()
                        .flex_col()
                        .gap(px(SPACE_8_PX))
                        .child(div().text_size(px(TEXT_13_5_PX)).child(entry.name.clone()))
                        .child(
                            div()
                                .text_size(px(TEXT_11_5_PX))
                                .text_color(p.on_surface_variant.hsla())
                                .child(format!("{} · {}", entry.source, entry.category)),
                        )
                        .child({
                            let url = if entry.documentation_url.is_empty() {
                                cfg.integrations
                                    .app_themes
                                    .sources
                                    .iter()
                                    .find(|source| source.id == entry.source)
                                    .map(|source| {
                                        source
                                            .files_url
                                            .replace("{app}", &entry.folder)
                                            .replace("{file}", "README.md")
                                    })
                                    .unwrap_or_default()
                            } else {
                                entry.documentation_url.clone()
                            };
                            button(
                                SharedString::from(format!("app-template-docs-{id}")),
                                "info",
                                t("Instructions"),
                                false,
                                p,
                            )
                            .on_click(move |_, _, cx| {
                                if url.starts_with("https://") {
                                    cx.open_url(&url);
                                }
                            })
                        })
                        .child(
                            button(
                                SharedString::from(format!("app-template-{id}")),
                                if on { "check" } else { "plus" },
                                if on { t("On") } else { t("Get & enable") },
                                on,
                                p,
                            )
                            .on_click(cx.listener(
                                move |this, _: &ClickEvent, _, cx| {
                                    if busy {
                                        return;
                                    }
                                    if on {
                                        update_config(cx, |config| {
                                            config
                                                .integrations
                                                .app_themes
                                                .enabled
                                                .retain(|enabled| enabled != &id)
                                        });
                                    } else {
                                        this.install_app_template(entry.clone(), cx);
                                    }
                                },
                            )),
                        )
                        .when(on, |card| {
                            card.child(
                                button(update_id, "refresh", t("Update template"), false, p)
                                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                        if !this.app_templates.busy {
                                            this.install_app_template(update_entry.clone(), cx);
                                        }
                                    })),
                            )
                        });
                    card.into_any_element()
                })
                .collect();
            content = content.child(fit.render(cards));
            if pages > 1 {
                content = content.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(SPACE_8_PX))
                        .child(
                            button(
                                "app-templates-previous",
                                "chevron-left",
                                t("Previous"),
                                false,
                                p,
                            )
                            .on_click(cx.listener(
                                move |this, _: &ClickEvent, _, cx| {
                                    this.app_templates.page = page.saturating_sub(1);
                                    cx.notify();
                                },
                            )),
                        )
                        .child(div().text_size(px(TEXT_12_PX)).child(tf(
                            "Page {page} of {pages}",
                            &[("page", &(page + 1)), ("pages", &pages)],
                        )))
                        .child(
                            button("app-templates-next", "chevron-right", t("Next"), false, p)
                                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                    this.app_templates.page = (page + 1).min(pages - 1);
                                    cx.notify();
                                })),
                        ),
                );
            }
        }
        if let Some(editor) = &self.app_templates.sources {
            content = content
                .child(
                    div()
                        .text_size(px(TEXT_12_PX))
                        .text_color(p.on_surface_variant.hsla())
                        .child(t("Catalog sources use HTTPS. Edit their names and URLs, then save with Ctrl+Enter.")),
                )
                .child(editor.clone());
        }
        let status = mandara_ui::state::app_theme_status(cx);
        let automatic_error = (!status.errors.is_empty()).then(|| {
            status
                .errors
                .iter()
                .map(|(id, error)| format!("{id}: {error}"))
                .collect::<Vec<_>>()
                .join("\n")
        });
        if let Some(error) = self
            .app_templates
            .error
            .as_ref()
            .or(automatic_error.as_ref())
        {
            content = content.child(
                div()
                    .text_size(px(TEXT_12_PX))
                    .text_color(p.urgent_coral.hsla())
                    .child(error.clone()),
            );
        }
        if let Some(editor) = &self.app_templates.editor {
            content = content
                .child(div().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(t(
                    "Write or paste a template document. Use content for an inline template, or input_path for a file. Set enabled = true to apply it. Save with Ctrl+Enter.",
                )))
                .child(editor.clone());
        }
        content = content.child(
            button(
                "app-templates-apply",
                "check",
                t("Apply app themes now"),
                false,
                p,
            )
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                if this.app_templates.busy {
                    return;
                }
                let palette = mandara_core::theme_entity(cx).read(cx).target().clone();
                let report = mandara_ui::state::export_app_themes_report(palette, cx);
                let cancel = Arc::new(AtomicBool::new(false));
                this.app_templates.busy = true;
                let task = cx.spawn(async move |view, cx| {
                    let report = report.await;
                    let _ = view.update(cx, |this, cx| {
                        this.app_templates.error = (!report.errors.is_empty()).then(|| {
                            report
                                .errors
                                .iter()
                                .map(|(id, error)| format!("{id}: {error}"))
                                .collect::<Vec<_>>()
                                .join("\n")
                        });
                        this.app_templates.busy = false;
                        cx.notify();
                    });
                });
                this.app_templates.job = Some(Job {
                    cancel,
                    _task: task,
                });
                cx.notify();
            })),
        );
        content.into_any_element()
    }

    fn load_app_templates(&mut self, refresh: bool, cx: &mut Context<Self>) {
        if self.app_templates.busy {
            return;
        }
        let locations = match Locations::current() {
            Ok(value) => value,
            Err(error) => {
                self.app_templates.error = Some(error.to_string());
                return;
            }
        };
        let sources = mandara_ui::state::config(cx)
            .read(cx)
            .get()
            .integrations
            .app_themes
            .sources
            .clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let token = cancel.clone();
        self.app_templates.busy = true;
        self.app_templates.error = None;
        let task = cx.spawn(async move |view, cx| {
            let result = jobs::discover(sources, locations.catalogs(), refresh, token).await;
            let _ = view.update(cx, |this, cx| {
                this.app_templates.rows = result
                    .entries
                    .into_iter()
                    .map(|entry| Row {
                        search: format!(
                            "{} {} {} {}",
                            entry.name, entry.id, entry.source, entry.category
                        )
                        .to_lowercase(),
                        entry,
                    })
                    .collect();
                this.app_templates.error =
                    (!result.errors.is_empty()).then(|| result.errors.join("\n"));
                this.app_templates.busy = false;
                cx.notify();
            });
        });
        self.app_templates.job = Some(Job {
            cancel,
            _task: task,
        });
        cx.notify();
    }

    fn install_app_template(&mut self, entry: Entry, cx: &mut Context<Self>) {
        let source = mandara_ui::state::config(cx)
            .read(cx)
            .get()
            .integrations
            .app_themes
            .sources
            .iter()
            .find(|source| source.id == entry.source)
            .cloned();
        let Some(source) = source else { return };
        let locations = match Locations::current() {
            Ok(value) => value,
            Err(error) => {
                self.app_templates.error = Some(error.to_string());
                return;
            }
        };
        let id = entry.qualified_id();
        let cancel = Arc::new(AtomicBool::new(false));
        let token = cancel.clone();
        self.app_templates.busy = true;
        let task = cx.spawn(async move |view, cx| {
            let result = jobs::install(source, entry, locations.catalogs(), token).await;
            let report = match result {
                Ok(_) => view
                    .update(cx, |_, cx| {
                        update_config(cx, |config| {
                            if !config.integrations.app_themes.enabled.contains(&id) {
                                config.integrations.app_themes.enabled.push(id);
                            }
                        });
                        let palette = mandara_core::theme_entity(cx).read(cx).target().clone();
                        mandara_ui::state::export_app_themes_report(palette, cx)
                    })
                    .ok(),
                Err(error) => {
                    let _ = view.update(cx, |this, cx| {
                        this.app_templates.error = Some(tf(
                            "Could not install app template: {error}",
                            &[("error", &format!("{error:#}"))],
                        ));
                        this.app_templates.busy = false;
                        cx.notify();
                    });
                    None
                }
            };
            if let Some(report) = report {
                let report = report.await;
                let _ = view.update(cx, |this, cx| {
                    this.app_templates.error = (!report.errors.is_empty()).then(|| {
                        report
                            .errors
                            .iter()
                            .map(|(id, error)| format!("{id}: {error}"))
                            .collect::<Vec<_>>()
                            .join("\n")
                    });
                    this.app_templates.busy = false;
                    cx.notify();
                });
            }
        });
        self.app_templates.job = Some(Job {
            cancel,
            _task: task,
        });
        cx.notify();
    }

    fn open_template_sources(&mut self, cx: &mut Context<Self>) {
        #[derive(serde::Serialize, serde::Deserialize)]
        struct Sources {
            sources: Vec<mandara_config::app_themes::CatalogSource>,
        }
        let sources = mandara_ui::state::config(cx)
            .read(cx)
            .get()
            .integrations
            .app_themes
            .sources
            .clone();
        let document = toml::to_string_pretty(&Sources { sources }).unwrap_or_default();
        let view = cx.entity().downgrade();
        self.app_templates.sources = Some(cx.new(|cx| {
            TextField::new(
                document,
                t("Catalog sources"),
                false,
                Rc::new(move |text, _, cx| {
                    let result = toml::from_str::<Sources>(&text)
                        .map_err(anyhow::Error::from)
                        .and_then(|sources| {
                            let mut config = mandara_ui::state::config(cx)
                                .read(cx)
                                .get()
                                .integrations
                                .app_themes
                                .clone();
                            config.sources = sources.sources;
                            config.validate()?;
                            update_config(cx, |current| {
                                current.integrations.app_themes.sources = config.sources
                            });
                            Ok(())
                        });
                    let _ = view.update(cx, |this, cx| {
                        let saved = result.is_ok();
                        this.app_templates.error = result.err().map(|error| {
                            tf(
                                "Could not save app templates: {error}",
                                &[("error", &error.to_string())],
                            )
                        });
                        if saved {
                            this.app_templates.job = None;
                            this.app_templates.rows = Arc::default();
                            this.app_templates.busy = false;
                            if this.app_templates.open {
                                this.load_app_templates(false, cx);
                            }
                        }
                        cx.notify();
                    });
                }),
                cx,
            )
            .multiline()
        }));
    }

    fn open_template_editor(&mut self, cx: &mut Context<Self>) {
        let cfg = mandara_ui::state::config(cx)
            .read(cx)
            .get()
            .integrations
            .app_themes
            .clone();
        let document = match editor_document(cfg) {
            Ok(document) => document,
            Err(error) => {
                self.app_templates.error = Some(error.to_string());
                return;
            }
        };
        let view = cx.entity().downgrade();
        self.app_templates.editor = Some(cx.new(|cx| {
            TextField::new(
                document,
                t("Template document"),
                false,
                Rc::new(move |text, _, cx| {
                    let _ = view.update(cx, |this, cx| {
                        match Manifest::parse_user(&text) {
                            Ok(document) => {
                                let mut next = mandara_ui::state::config(cx)
                                    .read(cx)
                                    .get()
                                    .integrations
                                    .app_themes
                                    .clone();
                                next.user = document.templates;
                                next.custom_colors = document.config.custom_colors;
                                match next.validate() {
                                    Ok(()) => {
                                        update_config(cx, |config| {
                                            config.integrations.app_themes = next
                                        });
                                        this.app_templates.error = None;
                                    }
                                    Err(error) => {
                                        this.app_templates.error = Some(tf(
                                            "Could not save app templates: {error}",
                                            &[("error", &error.to_string())],
                                        ))
                                    }
                                }
                            }
                            Err(error) => {
                                this.app_templates.error = Some(tf(
                                    "Could not save app templates: {error}",
                                    &[("error", &error.to_string())],
                                ))
                            }
                        }
                        cx.notify();
                    });
                }),
                cx,
            )
            .multiline()
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_example_document_keeps_existing_custom_colours_when_saved() {
        let mut config = mandara_config::app_themes::AppThemesConfig::default();
        config.custom_colors.insert(
            "warning".into(),
            mandara_theme::templates::custom::CustomColor::Hex("#ff8800".into()),
        );
        let expected = config.custom_colors.clone();
        let saved = Manifest::parse_user(&editor_document(config).unwrap()).unwrap();
        assert_eq!(saved.config.custom_colors, expected);
        assert!(!saved.templates["example"].enabled);
        assert!(saved.templates["example"].content.is_some());
    }

    #[test]
    fn catalog_search_matches_all_terms_without_changing_app_identity() {
        let row = Row {
            entry: Entry {
                id: "editor".into(),
                source: "community".into(),
                name: "Example Editor".into(),
                category: "editor".into(),
                documentation_url: String::new(),
                files: Vec::new(),
                manifest: None,
                folder: "editor".into(),
            },
            search: "example editor community".into(),
        };
        assert!(matches(&row, &["editor".into(), "community".into()]));
        assert!(!matches(&row, &["terminal".into()]));
        assert_eq!(row.entry.qualified_id(), "community:editor");
        assert!(!Templates::default().open);
        assert!(
            Manifest::parse_user("[templates]")
                .unwrap()
                .templates
                .is_empty()
        );
    }
}
