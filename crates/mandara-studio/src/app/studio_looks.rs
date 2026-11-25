//! Lazy Studio Look previews; selecting one writes only the Studio snapshot.

use std::rc::Rc;

use gpui::{
    AnyElement, ClickEvent, Context, FontWeight, InteractiveElement, IntoElement, ParentElement,
    SharedString, StatefulInteractiveElement, Styled, canvas, div, prelude::FluentBuilder as _, px,
};
use mandara_config::{
    ShellConfig, ThemeMode,
    studio::{FrameDesign, StudioAppearance, StudioLook},
};
use mandara_core::ToGpui as _;
use mandara_i18n::{t, tr};
use mandara_theme::MandaraPalette;
use mandara_ui::{bidi::Bidi as _, components::toggles, state::update_config};

use super::{
    StudioView,
    compositor_page::{button, heading},
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
    appearance: StudioAppearance,
    display_font: SharedString,
    ui_font: SharedString,
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

fn resolve_previews(installed: &[String]) -> anyhow::Result<Vec<Preview>> {
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
                appearance,
                display_font: mandara_ui::fonts::display_family(
                    installed,
                    look.definition().display_font,
                ),
                ui_font: mandara_ui::fonts::display_family(installed, look.definition().ui_font),
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
        let installed = cx.text_system().all_font_names();
        let job = cx
            .background_executor()
            .spawn(async move { resolve_previews(&installed) });
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
                    let customized = customized(config.studio.as_deref(), preview);
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .row()
                        .flex_col()
                        .gap(px(SPACE_12_PX))
                        .child(miniature(preview))
                        .child(
                            div()
                                .font_family(preview.display_font.clone())
                                .text_size(px(TEXT_18_PX))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(tr(def.name)),
                        )
                        .when(customized, |el| {
                            el.child(
                                div()
                                    .text_size(px(TEXT_12_PX))
                                    .text_color(p.warning_amber.hsla())
                                    .child(t("Customized")),
                            )
                        })
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
                                if customized {
                                    "refresh"
                                } else if active {
                                    "check"
                                } else {
                                    "palette"
                                },
                                if customized {
                                    t("Reapply preset")
                                } else if active {
                                    t("Selected")
                                } else {
                                    t("Apply")
                                },
                                active,
                                p,
                            )
                            .tab_index(0)
                            .focus(|s| s.border_2().border_color(p.primary.hsla()))
                            .on_click(move |_: &ClickEvent, _, cx| {
                                if active && !customized {
                                    return;
                                }
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

fn customized(current: Option<&StudioAppearance>, preview: &Preview) -> bool {
    current.is_some_and(|selected| selected.look == preview.look && selected != &preview.appearance)
}

fn miniature(preview: &Preview) -> AnyElement {
    let p = &preview.palette;
    let mut frame = preview.appearance.frame.clone();
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
                .font_family(preview.display_font.clone())
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
                .child(toggles::switch_still(
                    true,
                    preview.appearance.style.toggle_design,
                    p,
                )),
        )
        .child(button_preview(preview));
    let lights = frame.design == FrameDesign::Glass;
    let control_icons = if frame.controls_at_start {
        [
            ("close", p.urgent_coral),
            ("minus", p.warning_amber),
            ("maximize", p.success_emerald),
        ]
    } else {
        [
            ("minus", p.warning_amber),
            ("maximize", p.success_emerald),
            ("close", p.urgent_coral),
        ]
    };
    let controls =
        div()
            .ltr_row()
            .items_center()
            .gap(px(SPACE_4_PX))
            .children(control_icons.into_iter().map(|(glyph, color)| {
                if lights {
                    div()
                        .size(px(SPACE_6_PX))
                        .rounded_full()
                        .bg(color.hsla())
                        .into_any_element()
                } else {
                    mandara_ui::icons::icon(glyph, ICON_10_PX, p.on_surface_variant)
                        .into_any_element()
                }
            }));
    let caption = div()
        .flex_1()
        .row()
        .when(lights, |el| el.justify_center())
        .child(
            div()
                .w(px(DIM_48_PX))
                .h(px(SPACE_4_PX))
                .rounded_full()
                .bg(p.on_surface.with_alpha(ALPHA_12).hsla()),
        );
    let title = studio_frame::title_surface(div(), &frame, p)
        .h(px(PREVIEW_TITLE_HEIGHT))
        .flex_none()
        .px(px(SPACE_8_PX))
        .row()
        .items_center()
        .gap(px(SPACE_8_PX));
    let title = if frame.controls_at_start {
        title.child(controls).child(caption)
    } else {
        title.child(caption).child(controls)
    };
    div()
        .h(px(PREVIEW_HEIGHT))
        .font_family(preview.ui_font.clone())
        .text_color(p.on_surface.hsla())
        .bg(p.surface_container_low.hsla())
        .rounded(px(RADIUS_12_PX))
        .child(studio_frame::preview(
            &frame,
            nav.into_any_element(),
            content.into_any_element(),
            title.into_any_element(),
            p,
            preview.appearance.style.ink,
        ))
        .into_any_element()
}

/// A still control uses the component geometry without tracking presses or
/// inheriting the active window's radius, motion and shadow settings.
fn button_preview(preview: &Preview) -> gpui::Div {
    use mandara_ui::components::{ButtonDesign, buttons, cut};
    let p = &preview.palette;
    let style = &preview.appearance.style;
    let button = div()
        .h(px(DIM_34_PX))
        .px(px(SPACE_14_PX))
        .row()
        .items_center()
        .gap(px(SPACE_7_PX))
        .text_color(p.on_primary.hsla())
        .text_size(px(TEXT_12_5_PX))
        .font_weight(FontWeight::MEDIUM);
    let button = if style.button_design == ButtonDesign::Cut {
        let color = p.primary;
        button.relative().child(
            canvas(
                |_, _, _| {},
                move |bounds, _, window, _| {
                    cut::paint(
                        window,
                        bounds,
                        buttons::cut(DIM_34_PX),
                        cut::Ink::solid(Some(color), None),
                    );
                },
            )
            .absolute()
            .inset_0(),
        )
    } else {
        button
            .rounded(px(buttons::radius_at_scale(
                style.button_design,
                DIM_34_PX,
                0.0,
                style.radius_scale,
            )))
            .bg(p.primary.hsla())
    };
    button
        .child(mandara_ui::icons::icon("check", ICON_14_PX, p.on_primary))
        .child(t("Apply"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_studio_look_resolves_and_release_discards_preview_storage() {
        let previews: Rc<[Preview]> = resolve_previews(&["Inter".into()]).unwrap().into();
        assert_eq!(previews.len(), StudioLook::ALL.len());
        for (preview, look) in previews.iter().zip(StudioLook::ALL) {
            assert_eq!(preview.look, look);
            assert_eq!(preview.palette.is_dark, look.definition().dark);
            assert_eq!(preview.display_font.as_ref(), "Inter");
            assert_eq!(preview.ui_font.as_ref(), "Inter");
        }
        let weak = Rc::downgrade(&previews);
        let mut gallery = Gallery {
            previews: Some(previews),
            ..Default::default()
        };
        gallery.release();
        assert!(weak.upgrade().is_none());
    }

    #[test]
    fn fine_tuning_a_preset_is_visible_before_reapplying_its_defaults() {
        let previews = resolve_previews(&["Inter".into()]).unwrap();
        let preview = &previews[0];
        let mut selected = preview.appearance.clone();
        assert!(!customized(Some(&selected), preview));
        selected.frame.sidebar_width += 1.0;
        assert!(customized(Some(&selected), preview));
        assert!(!customized(Some(&selected), &previews[1]));
        assert!(!customized(None, preview));
    }
}
