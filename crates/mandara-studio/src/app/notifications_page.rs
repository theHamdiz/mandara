//! Studio → Notifications: popups, timeouts, do not disturb and history
//! (registry rows), plus the per-app rules editor: match an app (and
//! optionally its title, text or category), then block it, keep it out of
//! popups / the history / sounds, let it through do not disturb, change
//! its urgency or popup time. Apps that sent notifications recently are
//! offered as one-click rules.
//!
//! Studio → Privacy: the indicator switches plus the per-device ignore lists.
//! Studio → On-screen displays is registry rows only (default arm).

use mandara_ui::bidi::Bidi as _;
use std::rc::Rc;

use gpui::{AnyElement, ClickEvent, Context, FontWeight, InteractiveElement, IntoElement, ParentElement, SharedString, StatefulInteractiveElement, Styled, div, prelude::FluentBuilder as _, px};
use mandara_config::ShellConfig;
use mandara_config::alerts::{NotificationRule, UrgencyLevel};
use mandara_core::ToGpui;
use mandara_i18n::{t, tf};
use mandara_theme::MandaraPalette;
use mandara_ui::icons::icon;
use mandara_ui::update_config;

use super::StudioView;
use super::compositor_page::{badge, button, card, chip, group_title, heading, regex_problem};
use crate::registry::{self, Section};
use crate::tokens::*;

/// Recent apps offered as one-click rules.
const RECENT_APPS: usize = 8;
/// Urgency override choices: `(value, label)`.
const URGENCY_CHOICES: [(Option<UrgencyLevel>, &str); 4] = [(None, mandara_i18n::tk("Keep")), (Some(UrgencyLevel::Low), mandara_i18n::tk("Low")), (Some(UrgencyLevel::Normal), mandara_i18n::tk("Normal")), (Some(UrgencyLevel::Critical), mandara_i18n::tk("Critical"))];

/// App names from the saved notification history, newest first, unique.
fn recent_apps() -> Vec<String> {
    #[derive(serde::Deserialize)]
    struct Entry {
        app_name: String,
    }
    let path = mandara_config::paths::cache_dir().join("notifications.json");
    let entries: Vec<Entry> = std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    let mut out: Vec<String> = Vec::new();
    for e in entries {
        if !e.app_name.trim().is_empty() && !out.iter().any(|a| a.eq_ignore_ascii_case(&e.app_name)) {
            out.push(e.app_name);
        }
    }
    out
}

/// A rule pattern matching exactly `app` (as the server's regex sees it).
fn exact(app: &str) -> String {
    super::compositor_page::exact_regex(app)
}

impl StudioView {
    fn notification_rule_card(&mut self, i: usize, rule: &NotificationRule, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let stack = self.stack_rows;
        let field = |this: &mut Self, key: &str, value: &str, placeholder: &'static str, apply: fn(&mut NotificationRule, String), cx: &mut Context<Self>| {
            this.list_field(
                format!("nrule-{i}-{key}"),
                value,
                placeholder,
                Rc::new(move |cfg: &mut ShellConfig, v: String| {
                    if let Some(r) = cfg.notifications.rules.get_mut(i) {
                        apply(r, v);
                    }
                }),
                cx,
            )
        };
        let app = field(self, "app", &rule.app, t("App, e.g. Spotify or ^discord$"), |r, v| r.app = v.trim().to_string(), cx);
        let summary = field(self, "summary", &rule.summary, t("Title contains (optional)"), |r, v| r.summary = v.trim().to_string(), cx);
        let body = field(self, "body", &rule.body, t("Text contains (optional)"), |r, v| r.body = v.trim().to_string(), cx);
        let timeout = field(
            self,
            "timeout",
            &rule.timeout.map(|s| format!("{s}")).unwrap_or_default(),
            t("Seconds (empty: default)"),
            |r, v| r.timeout = v.trim().parse::<f32>().ok().filter(|s| *s >= 0.0),
            cx,
        );
        let problem = [rule.app.as_str(), rule.summary.as_str(), rule.body.as_str()].into_iter().find_map(regex_problem);
        let matches_all = rule.app.trim().is_empty() && rule.summary.trim().is_empty() && rule.body.trim().is_empty() && rule.category.trim().is_empty();
        let toggle = |id: &str, label: &'static str, on: bool, flip: fn(&mut NotificationRule), p: &MandaraPalette| {
            chip(SharedString::from(format!("nrule-{i}-{id}")), t(label), on, p).on_click(move |_: &ClickEvent, _, cx| {
                update_config(cx, |c| {
                    if let Some(r) = c.notifications.rules.get_mut(i) {
                        flip(r);
                    }
                })
            })
        };
        let mut effects = div().row().flex_wrap().gap(px(SPACE_6_PX));
        effects = effects
            .child(toggle("block", mandara_i18n::tk("Block"), rule.block, |r| r.block = !r.block, p))
            .when(!rule.block, |el| {
                el.child(toggle("popup", mandara_i18n::tk("Popup"), rule.popup, |r| r.popup = !r.popup, p))
                    .child(toggle("history", mandara_i18n::tk("History"), rule.history, |r| r.history = !r.history, p))
                    .child(toggle("sound", mandara_i18n::tk("Sound"), rule.sound, |r| r.sound = !r.sound, p))
                    .child(toggle("dnd", mandara_i18n::tk("Through do not disturb"), rule.bypass_dnd, |r| r.bypass_dnd = !r.bypass_dnd, p))
            });
        let mut urgency = div().row().flex_wrap().items_center().gap(px(SPACE_6_PX)).child(div().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(t("Urgency")));
        for (n, (value, label)) in URGENCY_CHOICES.into_iter().enumerate() {
            urgency = urgency.child(chip(SharedString::from(format!("nrule-{i}-u{n}")), t(label), rule.set_urgency == value, p).on_click(move |_: &ClickEvent, _, cx| {
                update_config(cx, |c| {
                    if let Some(r) = c.notifications.rules.get_mut(i) {
                        r.set_urgency = value;
                    }
                })
            }));
        }
        let enabled = rule.enabled;
        let delete = div()
            .id(SharedString::from(format!("nrule-{i}-del")))
            .size(px(DIM_32_PX))
            .flex_none()
            .rounded_full()
            .row()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .hover(|s| s.bg(p.urgent_coral.with_alpha(ALPHA_14).hsla()))
            .child(icon("trash", ICON_15_PX, p.on_surface_variant))
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.list_fields.clear();
                update_config(cx, |c| {
                    if i < c.notifications.rules.len() {
                        c.notifications.rules.remove(i);
                    }
                });
            }));
        let switch = div()
            .id(SharedString::from(format!("nrule-{i}-on")))
            .flex_none()
            .cursor_pointer()
            .child(mandara_ui::plugins::network::switch(enabled, p))
            .on_click(move |_: &ClickEvent, _, cx| {
                update_config(cx, |c| {
                    if let Some(r) = c.notifications.rules.get_mut(i) {
                        r.enabled = !r.enabled;
                    }
                })
            });
        div()
            .p(px(SPACE_12_PX))
            .rounded(px(RADIUS_14_PX))
            .bg(p.surface_container_high.with_alpha(ALPHA_50).hsla())
            .when(!enabled, |el| el.opacity(ALPHA_60))
            .row()
            .flex_col()
            .gap(px(SPACE_10_PX))
            .child(
                div()
                    .row()
                    .items_center()
                    .gap(px(SPACE_8_PX))
                    .child(div().w(px(DIM_22_PX)).flex_none().text_size(px(TEXT_12_PX)).font_weight(FontWeight::SEMIBOLD).text_color(p.primary.hsla()).font_features(mandara_ui::fonts::tabular_nums()).child(format!("{}", i + 1)))
                    .child(div().flex_1().min_w(px(0.0)).child(app))
                    .child(switch)
                    .child(delete),
            )
            .child(div().row().when(stack, |el| el.flex_col()).gap(px(SPACE_8_PX)).child(div().flex_1().min_w(px(0.0)).child(summary)).child(div().flex_1().min_w(px(0.0)).child(body)))
            .child(effects)
            .child(
                div()
                    .row()
                    .when(stack, |el| el.flex_col().cross_start())
                    .when(!stack, |el| el.items_center())
                    .gap(px(SPACE_10_PX))
                    .child(urgency)
                    .when(!rule.block, |el| el.child(div().row().items_center().gap(px(SPACE_8_PX)).child(div().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(t("Popup time"))).child(div().w(px(DIM_180_PX)).child(timeout)))),
            )
            .when_some(problem, |el, msg| el.child(badge(t(msg), p.urgent_coral)))
            .when(matches_all, |el| el.child(badge(t("Matches every notification"), p.warning_amber)))
            .into_any_element()
    }

    /// The per-app rules editor.
    fn notification_rules(&mut self, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let rules = cfg.notifications.rules.clone();
        let mut list = div().row().flex_col().gap(px(SPACE_8_PX));
        for (i, rule) in rules.iter().enumerate() {
            list = list.child(self.notification_rule_card(i, rule, p, cx));
        }
        if rules.is_empty() {
            list = list.child(div().px(px(SPACE_4_PX)).text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(t("No rules yet: every app follows the settings above.")));
        }
        // One-click rules for apps that notified recently.
        let recent: Vec<String> = recent_apps().into_iter().filter(|a| !rules.iter().any(|r| r.app == exact(a) || r.app.eq_ignore_ascii_case(a))).take(RECENT_APPS).collect();
        let add = button("nrule-add", "plus", t("Add rule"), true, p).on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
            this.list_fields.clear();
            update_config(cx, |c| c.notifications.rules.push(NotificationRule::default()));
        }));
        let mut quick = div().row().flex_wrap().items_center().gap(px(SPACE_6_PX));
        if !recent.is_empty() {
            quick = quick.child(div().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(t("Recent apps:")));
            for (n, app) in recent.into_iter().enumerate() {
                let pattern = exact(&app);
                quick = quick.child(chip(SharedString::from(format!("nrule-recent-{n}")), app, false, p).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.list_fields.clear();
                    let rule = NotificationRule::for_app(&pattern);
                    update_config(cx, |c| c.notifications.rules.push(rule));
                })));
            }
        }
        card(p)
            .child(heading(t("Per-app rules"), t("The first matching rule decides. Patterns are case-insensitive regular expressions; plain words match anywhere. Also from the command line: `mandara msg notifications rules add silent Spotify`."), p))
            .child(list)
            .child(div().row().flex_wrap().items_center().gap(px(SPACE_10_PX)).child(add).child(quick))
            .into_any_element()
    }

    pub(super) fn notifications_page(&mut self, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        use registry::alert_settings::{POPUP_CONTENT, POPUPS, TIMEOUTS};
        let popup = |s: &&registry::Setting| s.group == POPUPS || s.group == POPUP_CONTENT || s.group == TIMEOUTS;
        let first = self.rows(registry::in_section(Section::Notifications).filter(popup), cfg, p, cx);
        let rules = self.notification_rules(cfg, p, cx);
        let rest = self.rows(registry::in_section(Section::Notifications).filter(|s| !popup(s)), cfg, p, cx);
        div()
            .row()
            .flex_col()
            .gap(px(SPACE_26_PX))
            .child(first)
            .child(div().row().flex_col().gap(px(SPACE_10_PX)).child(group_title(t("Rules"), p)).child(rules))
            .child(rest)
            .into_any_element()
    }

    /// One ignore list (`[privacy] ignore_*`) as removable chips plus a field.
    pub(super) fn ignore_list(&mut self, key: &'static str, title: &'static str, list: &[String], get: fn(&mut ShellConfig) -> &mut Vec<String>, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let field = self.list_field(
            format!("ignore-{key}"),
            "",
            t("App name or pattern, then Enter"),
            Rc::new(move |cfg: &mut ShellConfig, v: String| {
                let v = v.trim().to_string();
                let list = get(cfg);
                if !v.is_empty() && !list.contains(&v) {
                    list.push(v);
                }
            }),
            cx,
        );
        let mut chips = div().row().flex_wrap().gap(px(SPACE_6_PX));
        for (n, item) in list.iter().enumerate() {
            let item2 = item.clone();
            chips = chips.child(
                chip(SharedString::from(format!("ignore-{key}-{n}")), item.clone(), false, p).child(icon("close", ICON_12_PX, p.on_surface_variant)).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.list_fields.clear();
                    let item = item2.clone();
                    update_config(cx, move |c| get(c).retain(|x| *x != item));
                })),
            );
        }
        div()
            .row()
            .flex_col()
            .gap(px(SPACE_8_PX))
            .child(div().text_size(px(TEXT_13_PX)).font_weight(FontWeight::MEDIUM).child(t(title)))
            .when(!list.is_empty(), |el| el.child(chips))
            .child(field)
            .into_any_element()
    }

    pub(super) fn privacy_page(&mut self, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let rows = self.rows(registry::in_section(Section::Privacy), cfg, p, cx);
        let mic = self.ignore_list("mic", mandara_i18n::tk("Microphone"), &cfg.privacy.ignore_microphone, |c| &mut c.privacy.ignore_microphone, p, cx);
        let cam = self.ignore_list("camera", mandara_i18n::tk("Camera"), &cfg.privacy.ignore_camera, |c| &mut c.privacy.ignore_camera, p, cx);
        let screen = self.ignore_list("screen", mandara_i18n::tk("Screen sharing"), &cfg.privacy.ignore_screen_share, |c| &mut c.privacy.ignore_screen_share, p, cx);
        let placed = [&cfg.bar.left, &cfg.bar.center, &cfg.bar.right].iter().any(|z| z.iter().any(|id| id == "privacy"));
        let ignore = card(p)
            .child(heading(t("Never report"), t("Apps you trust, e.g. a voice assistant that always listens. Case-insensitive regular expressions."), p))
            .child(super::grid::fit(self.page.column_w, PANEL_GRID).at_most(3).render([div().flex_1().child(mic), div().flex_1().child(cam), div().flex_1().child(screen)]));
        div()
            .row()
            .flex_col()
            .gap(px(SPACE_26_PX))
            .child(rows)
            .when(!placed && cfg.privacy.indicators, |el| {
                el.child(
                    card(p).child(heading(t("The bar chip isn't placed"), tf("Add the “{name}” plugin to the bar (Studio → Bar) to see the indicator there; on-screen displays work either way.", &[("name", &t("Privacy"))]), p)),
                )
            })
            .child(div().row().flex_col().gap(px(SPACE_10_PX)).child(group_title(t("Ignore"), p)).child(ignore))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_patterns_escape_names() {
        assert_eq!(exact("Mozilla Firefox"), "^Mozilla Firefox$");
        assert_eq!(exact("a.b"), "^a\\.b$");
    }
}
