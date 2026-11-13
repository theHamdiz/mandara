//! Lazy Studio Look previews; selecting one writes only the Studio snapshot.

use std::rc::Rc;

use gpui::{
    AnyElement, ClickEvent, Context, FontWeight, InteractiveElement, IntoElement, ParentElement,
    SharedString, StatefulInteractiveElement, Styled, div, px,
};
use mandara_config::{
    ShellConfig, ThemeMode,
    desktop::StyleConfig,
    studio::{StudioFrame, StudioLook},
};
use mandara_core::ToGpui as _;
use mandara_i18n::{t, tr};
use mandara_theme::MandaraPalette;
use mandara_ui::{bidi::Bidi as _, components::toggles, state::update_config};

use super::{
    StudioView,
    compositor_page::{button, button_as, heading},
    grid, studio_frame,
};
use crate::{latest::Latest, tokens::*};

/// Two or three legible miniature windows fit the page without shrinking controls.
const PRESETS: CardGrid = CardGrid {
    min_w: 260.0,
    max_w: 510.0,
    gap: SPACE_18_PX,
};
/// A miniature window has a fixed viewport; its content demonstrates the recipe.
const PREVIEW_HEIGHT: f32 = 206.0;
const PREVIEW_NAV_WIDTH: f32 = 68.0;
const PREVIEW_NAV_ROW: f32 = 8.0;
const PREVIEW_TITLE_HEIGHT: f32 = 28.0;
const PREVIEW_CONTENT_PADDING: f32 = 12.0;

struct Preview {
    look: StudioLook,
    frame: StudioFrame,
    style: StyleConfig,
    palette: MandaraPalette,
}

#[derive(Default)]
pub(super) struct Gallery {
    requests: Latest<()>,
    previews: Option<Rc<[Preview]>>,
    error: Option<String>,
}

impl Gallery {
    pub fn release(&mut self) {
        self.requests.set(None);
        self.previews = None;
        self.error = None;
    }
}

fn resolve_previews() -> anyhow::Result<Vec<Preview>> {
    StudioLook::ALL
        .into_iter()
        .map(|look| {
            let appearance = look.appearance();
            let palette = appearance
                .theme
                .source()
                .resolve(appearance.theme.mode.is_dark())?
                .palette;
            Ok(Preview {
                look,
                frame: appearance.frame.bounded(),
                style: appearance.style,
                palette,
            })
        })
        .collect()
}

impl StudioView {
    fn studio_previews(&mut self, cx: &mut Context<Self>) {
        self.studio_looks.requests.set(Some(()));
        let Some(request) = self.studio_looks.requests.begin() else {
            return;
        };
        // Resolve one small fixed catalog in sequence, outside GPUI's thread.
        let job = cx.background_executor().spawn(async { resolve_previews() });
        cx.spawn(async move |this, cx| {
            let result = job.await;
            let _ = this.update(cx, |this, cx| {
                if this.studio_looks.requests.finish(request) {
                    match result {
                        Ok(previews) => this.studio_looks.previews = Some(previews.into()),
                        Err(error) => this.studio_looks.error = Some(format!("{error:#}")),
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn studio_looks_page(
        &mut self,
        config: &ShellConfig,
        p: &MandaraPalette,
        visible: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if visible {
            self.studio_previews(cx);
        }
        let current = config.studio.as_deref().map(|s| s.look);
        let mut body = div().row().flex_col().gap(px(SPACE_18_PX))
            .child(heading(t("A look for Studio"), t("Change this window, its sidebar and every control. Your desktop Look stays separate."), p))
            .child(button("studio-follow-shell", "link", t("Follow the shell"), current.is_none(), p).on_click(|_, _, cx| update_config(cx, |c| c.studio = None)));
        if let Some(selected) = config.studio.as_deref() {
            let dark = selected.theme.mode.is_dark();
            body = body.child(
                div().row().gap(px(SPACE_8_PX)).children(
                    [
                        (false, "studio-light", "Light"),
                        (true, "studio-dark", "Dark"),
                    ]
                    .into_iter()
                    .map(|(mode, id, label)| {
                        button(
                            id,
                            if mode { "moon" } else { "sun" },
                            tr(label),
                            mode == dark,
                            p,
                        )
                        .on_click(move |_, _, cx| {
                            update_config(cx, |c| {
                                if let Some(studio) = c.studio.as_mut() {
                                    studio.theme.mode = if mode {
                                        ThemeMode::Dark
                                    } else {
                                        ThemeMode::Light
                                    };
                                }
                            })
                        })
                    }),
                ),
            );
        }
        if let Some(error) = crate::appearance::error(cx) {
            body = body
                .child(super::theme_galleries::error_row(error.to_owned(), p))
                .child(
                    button("studio-palette-retry", "refresh", t("Retry"), false, p)
                        .on_click(|_, _, cx| crate::appearance::retry(cx)),
                );
        }
        if let Some(error) = self.studio_looks.error.as_deref() {
            body = body
                .child(super::theme_galleries::error_row(error.to_owned(), p))
                .child(
                    button("studio-previews-retry", "refresh", t("Retry"), false, p).on_click(
                        cx.listener(|this, _, _, cx| {
                            this.studio_looks.error = None;
                            this.studio_looks.requests.refresh();
                            cx.notify();
                        }),
                    ),
                );
        }
        if let Some(previews) = self.studio_looks.previews.clone() {
            let fit = grid::fit(self.page.column_w, PRESETS).balanced(previews.len());
            body = body.child(
                fit.render(previews.iter().enumerate().map(|(index, preview)| {
                    let def = preview.look.definition();
                    let look = preview.look;
                    let active = current == Some(look);
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .row()
                        .flex_col()
                        .gap(px(SPACE_12_PX))
                        .child(miniature(preview, index))
                        .child(
                            div()
                                .font_family(SharedString::new_static(def.display_font))
                                .text_size(px(TEXT_18_PX))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(tr(def.name)),
                        )
                        .child(
                            div()
                                .flex_1()
                                .text_size(px(TEXT_12_PX))
                                .text_color(p.on_surface_variant.hsla())
                                .child(mandara_ui::bidi::paragraph(tr(def.description))),
                        )
                        .child(
                            button(
                                SharedString::from(format!("studio-look-{index}")),
                                if active { "check" } else { "palette" },
                                if active { t("Selected") } else { t("Apply") },
                                active,
                                p,
                            )
                            .tab_index(0)
                            .focus(|s| s.bg(p.primary_container.hsla()))
                            .on_click(move |_: &ClickEvent, _, cx| {
                                update_config(cx, |c| c.studio = Some(Box::new(look.appearance())))
                            }),
                        )
                        .into_any_element()
                })),
            );
        } else if self.studio_looks.error.is_none() {
            body = body.child(
                div()
                    .text_color(p.on_surface_variant.hsla())
                    .child(t("Loading previews…")),
            );
        }
        body.into_any_element()
    }
}

fn miniature(preview: &Preview, index: usize) -> AnyElement {
    let p = &preview.palette;
    let mut frame = preview.frame.clone();
    frame.title_height = PREVIEW_TITLE_HEIGHT;
    frame.inset = SPACE_6_PX;
    frame.sidebar_inset = SPACE_6_PX;
    let nav = studio_frame::sidebar(div(), Some(&frame), p)
        .w(px(PREVIEW_NAV_WIDTH))
        .flex_none()
        .p(px(SPACE_8_PX))
        .row()
        .flex_col()
        .gap(px(SPACE_8_PX))
        .children((0..5).map(|at| {
            div()
                .h(px(PREVIEW_NAV_ROW))
                .rounded(px(frame.nav_radius))
                .bg(if at == 1 {
                    p.primary_container
                } else {
                    p.on_surface.with_alpha(ALPHA_12)
                }
                .hsla())
        }));
    let content = div()
        .flex_1()
        .min_w(px(0.0))
        .p(px(PREVIEW_CONTENT_PADDING))
        .row()
        .flex_col()
        .gap(px(SPACE_10_PX))
        .child(
            div()
                .text_size(px(TEXT_12_PX))
                .font_weight(FontWeight::SEMIBOLD)
                .child(t("Appearance")),
        )
        .child(
            div()
                .h(px(DIM_28_PX))
                .rounded(px(frame.card_radius))
                .bg(p.surface_container.hsla())
                .px(px(SPACE_8_PX))
                .row()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .w(px(DIM_36_PX))
                        .h(px(PREVIEW_NAV_ROW))
                        .rounded_full()
                        .bg(p.on_surface.with_alpha(ALPHA_12).hsla()),
                )
                .child(toggles::switch(true, preview.style.toggle_design, p)),
        )
        .child(button_as(
            preview.style.button_design,
            SharedString::from(format!("studio-preview-button-{index}")),
            "check",
            t("Apply"),
            true,
            p,
        ));
    let title = div()
        .h(px(PREVIEW_TITLE_HEIGHT))
        .px(px(SPACE_8_PX))
        .row()
        .items_center()
        .gap(px(SPACE_4_PX))
        .child(
            div()
                .size(px(SPACE_6_PX))
                .rounded_full()
                .bg(p.urgent_coral.hsla()),
        )
        .child(
            div()
                .size(px(SPACE_6_PX))
                .rounded_full()
                .bg(p.warning_amber.hsla()),
        )
        .child(
            div()
                .size(px(SPACE_6_PX))
                .rounded_full()
                .bg(p.success_emerald.hsla()),
        );
    div()
        .h(px(PREVIEW_HEIGHT))
        .text_color(p.on_surface.hsla())
        .bg(p.surface_container_low.hsla())
        .rounded(px(RADIUS_12_PX))
        .child(studio_frame::window(
            Some(&frame),
            nav.into_any_element(),
            content.into_any_element(),
            Some(title.into_any_element()),
            p,
        ))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_studio_look_resolves_and_release_discards_preview_storage() {
        let previews: Rc<[Preview]> = resolve_previews().unwrap().into();
        assert_eq!(previews.len(), StudioLook::ALL.len());
        for (preview, look) in previews.iter().zip(StudioLook::ALL) {
            assert_eq!(preview.look, look);
            assert_eq!(preview.palette.is_dark, look.definition().dark);
        }
        let weak = Rc::downgrade(&previews);
        let mut gallery = Gallery {
            previews: Some(previews),
            ..Default::default()
        };
        gallery.release();
        assert!(weak.upgrade().is_none());
    }
}
