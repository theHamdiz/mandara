//! Studio → Window switcher: the Alt-Tab switcher's options (registry
//! rows, with "Bind Alt+Tab" and a non-grabbing preview), then Screen time:
//! the plugin switch, its options and per-app daily reminders.

use mandara_ui::bidi::Bidi as _;
use std::rc::Rc;

use gpui::{AnyElement, ClickEvent, Context, InteractiveElement, IntoElement, ParentElement, SharedString, StatefulInteractiveElement, Styled, div, px};
use mandara_config::ShellConfig;
use mandara_core::ToGpui;
use mandara_i18n::t;
use mandara_theme::MandaraPalette;
use mandara_ui::icons::icon;
use mandara_ui::state::update_config;

use super::StudioView;
use super::compositor_page::{card, group_title, heading};
use super::dock_page::plugin_switch;
use crate::registry::{self, Section, workflow_settings};
use crate::tokens::*;

/// Default reminder for an app added to the limits list, minutes.
const DEFAULT_APP_LIMIT_MIN: u32 = 60;

impl StudioView {
    pub(super) fn switcher_page(&mut self, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let switcher = self.rows(registry::in_section(Section::WindowSwitcher).filter(|s| s.group != workflow_settings::SCREEN_TIME), cfg, p, cx);
        let screen_time_switch = plugin_switch("screen-time", t("Screen time"), t("A bar item and panel with today's time per app and the last seven days. Recorded only on this computer."), cfg, p, cx);
        let screen_time = self.rows(registry::in_section(Section::WindowSwitcher).filter(|s| s.group == workflow_settings::SCREEN_TIME), cfg, p, cx);

        // Per-app daily reminders.
        let mut limits = div().row().flex_col().gap(px(SPACE_8_PX));
        let entries: Vec<(String, u32)> = cfg.screen_time.app_limits.iter().map(|(k, v)| (k.clone(), *v)).collect();
        for (app, minutes) in entries {
            let (app_set, app_del) = (app.clone(), app.clone());
            let field = self.list_field(
                format!("limit-{app}"),
                &minutes.to_string(),
                t("minutes"),
                Rc::new(move |c, v| {
                    if let Ok(m) = v.trim().parse::<u32>() {
                        c.screen_time.app_limits.insert(app_set.clone(), m);
                    }
                }),
                cx,
            );
            limits = limits.child(
                div()
                    .p(px(SPACE_10_PX))
                    .rounded(px(RADIUS_14_PX))
                    .bg(p.surface_container_high.with_alpha(ALPHA_50).hsla())
                    .row()
                    .items_center()
                    .gap(px(SPACE_10_PX))
                    .child(mandara_ui::plugins::workspaces::app_icon(&app, ICON_22_PX, p.on_surface, false, p))
                    .child(div().flex_1().min_w(px(0.0)).truncate().child(mandara_ui::plugins::window::app_name(&app)))
                    .child(div().w(px(DIM_90_PX)).child(field))
                    .child(div().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(t("min a day")))
                    .child(
                        div()
                            .id(SharedString::from(format!("limit-del-{app}")))
                            .size(px(DIM_32_PX))
                            .rounded_full()
                            .row()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .hover(|s| s.bg(p.urgent_coral.with_alpha(ALPHA_14).hsla()))
                            .child(icon("trash", ICON_15_PX, p.on_surface_variant))
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                this.list_fields.retain(|k, _| !k.starts_with("limit-"));
                                let app = app_del.clone();
                                update_config(cx, move |c| {
                                    c.screen_time.app_limits.remove(&app);
                                });
                            })),
                    ),
            );
        }
        let add = self.add_field(
            "limit-add",
            t("Add an app id (e.g. firefox), then Enter"),
            Rc::new(|value, cx| {
                let app = value.trim().to_string();
                update_config(cx, move |c| {
                    c.screen_time.app_limits.entry(app).or_insert(DEFAULT_APP_LIMIT_MIN);
                });
            }),
            cx,
        );
        limits = limits.child(div().child(add));

        div()
            .row()
            .flex_col()
            .gap(px(SPACE_26_PX))
            .child(switcher)
            .child(div().row().flex_col().gap(px(SPACE_10_PX)).child(group_title(t("Screen time"), p)).child(screen_time_switch))
            .child(screen_time)
            .child(
                div()
                    .row()
                    .flex_col()
                    .gap(px(SPACE_10_PX))
                    .child(group_title(t("App reminders"), p))
                    .child(card(p).child(heading(t("Daily limits per app"), t("A gentle notification once an app has had this much of your day."), p)).child(limits)),
            )
            .into_any_element()
    }
}
