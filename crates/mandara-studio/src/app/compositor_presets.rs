//! Studio → Compositor presets: one-click window looks (gaps, corner
//! radius, border width) with a live miniature, and common window rules
//! (picture-in-picture, volume control, file dialogs…) added with a click.
//! Both only write `[compositor]` — the drop-in, never your own file.

use mandara_ui::bidi::Bidi as _;
use gpui::{AnyElement, ClickEvent, Context, FontWeight, InteractiveElement, IntoElement, ParentElement, SharedString, StatefulInteractiveElement, Styled, div, prelude::FluentBuilder as _, px};
use mandara_config::ShellConfig;
use mandara_config::compositor::{CompositorConfig, WindowRule};
use mandara_core::ToGpui;
use mandara_i18n::{t, tk, tr};
use mandara_theme::MandaraPalette;
use mandara_ui::icons::icon;
use mandara_ui::state::update_config;

use super::StudioView;
use super::compositor_page::{card, chip, group_title, heading, tip};
use crate::tokens::*;

/// A window-look preset: `(gaps inner, gaps outer, rounding, border)`;
/// `None` everywhere = your compositor's own look.
struct LookPreset {
    id: &'static str,
    label: &'static str,
    look: Option<(u32, u32, u32, u32)>,
}

const LOOKS: &[LookPreset] = &[
    LookPreset { id: "yours", label: tk("Your config"), look: None },
    LookPreset { id: "snug", label: tk("Snug"), look: Some((4, 6, 8, 2)) },
    LookPreset { id: "balanced", label: tk("Balanced"), look: Some((8, 12, 12, 2)) },
    LookPreset { id: "airy", label: tk("Airy"), look: Some((14, 22, 18, 2)) },
    LookPreset { id: "flush", label: tk("Edge to edge"), look: Some((0, 0, 0, 1)) },
];

/// A common window rule: matcher regexes and what it does.
struct RulePreset {
    id: &'static str,
    label: &'static str,
    app_id: Option<&'static str>,
    title: Option<&'static str>,
    /// Visible on every workspace / tag too.
    global: bool,
}

const RULES: &[RulePreset] = &[
    RulePreset { id: "pip", label: tk("Picture-in-picture floats on every workspace"), app_id: None, title: Some("^Picture[- ]in[- ][Pp]icture$"), global: true },
    RulePreset { id: "volume", label: tk("Volume control floats"), app_id: Some("^(pavucontrol|org\\.pulseaudio\\.pavucontrol|com\\.saivert\\.pwvucontrol)$"), title: None, global: false },
    RulePreset { id: "dialogs", label: tk("File dialogs float"), app_id: None, title: Some("^(Open|Save|Save As|Open File|Save File|Select a File).*$"), global: false },
    RulePreset { id: "calc", label: tk("Calculators float"), app_id: Some("^(org\\.gnome\\.Calculator|gnome-calculator|qalculate-gtk|kcalc)$"), title: None, global: false },
    RulePreset { id: "applets", label: tk("Network & Bluetooth settings float"), app_id: Some("^(nm-connection-editor|blueman-manager|org\\.gnome\\.Settings)$"), title: None, global: false },
    RulePreset { id: "auth", label: tk("Password prompts float"), app_id: Some("^(org\\.kde\\.polkit-kde-authentication-agent-1|polkit-gnome-authentication-agent-1|gcr-prompter|pinentry-.*)$"), title: None, global: false },
];

/// Miniature: two windows in a frame, at `scale` of the real gaps / radius.
const MINI_W_PX: f32 = 84.0;
const MINI_H_PX: f32 = 50.0;
const MINI_SCALE: f32 = 0.5;
const MINI_FRAME_RADIUS_PX: f32 = 6.0;
const LOOK_GRID: CardGrid = CardGrid { min_w: 120.0, max_w: 200.0, gap: SPACE_8_PX };

fn current_look(c: &CompositorConfig) -> Option<(u32, u32, u32, u32)> {
    match (c.look.gaps_inner, c.look.gaps_outer, c.look.rounding, c.border_width) {
        (None, None, None, None) => None,
        (i, o, r, b) => Some((i.unwrap_or(u32::MAX), o.unwrap_or(u32::MAX), r.unwrap_or(u32::MAX), b.unwrap_or(u32::MAX))),
    }
}

fn has_rule(c: &CompositorConfig, r: &RulePreset) -> bool {
    c.window_rules.iter().any(|w| w.app_id.as_deref() == r.app_id && w.title.as_deref() == r.title)
}

fn mini(look: Option<(u32, u32, u32, u32)>, active: bool, p: &MandaraPalette) -> impl IntoElement {
    let (inner, outer, radius, border) = look.map(|(i, o, r, b)| (i as f32, o as f32, r as f32, b as f32)).unwrap_or((6.0, 8.0, 8.0, 1.0));
    let win = |focused: bool| {
        div()
            .flex_1()
            .h_full()
            .rounded(px(radius * MINI_SCALE))
            .border(px((border * MINI_SCALE).max(1.0)))
            .border_color(if focused { p.primary.hsla() } else { p.outline.with_alpha(ALPHA_40).hsla() })
            .bg(p.surface_container_high.hsla())
    };
    div()
        .w(px(MINI_W_PX))
        .h(px(MINI_H_PX))
        .flex_none()
        .rounded(px(MINI_FRAME_RADIUS_PX))
        .bg(if active { p.primary.with_alpha(ALPHA_12).hsla() } else { p.surface_dim.hsla() })
        .p(px(outer * MINI_SCALE))
        .row()
        .gap(px(inner * MINI_SCALE))
        .when(look.is_none(), |el| el.opacity(ALPHA_60))
        .child(win(true))
        .child(win(false))
}

impl StudioView {
    pub(super) fn comp_presets(&mut self, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let c = &cfg.compositor;
        let now = current_look(c);
        let looks: Vec<AnyElement> = LOOKS
            .iter()
            .map(|preset| {
                let active = now == preset.look && !c.look.gaps_follow_bar;
                let look = preset.look;
                div()
                    .id(SharedString::from(format!("look-preset-{}", preset.id)))
                    .flex_1()
                    .p(px(SPACE_10_PX))
                    .rounded(px(RADIUS_14_PX))
                    .row()
                    .flex_col()
                    .items_center()
                    .gap(px(SPACE_8_PX))
                    .cursor_pointer()
                    .bg(if active { p.primary_container.hsla() } else { p.surface_container_high.hsla() })
                    .hover(|s| s.opacity(ALPHA_88))
                    .child(mini(look, active, p))
                    .child(div().text_size(px(TEXT_12_5_PX)).font_weight(if active { FontWeight::SEMIBOLD } else { FontWeight::NORMAL }).child(mandara_ui::bidi::paragraph(tr(preset.label))))
                    .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                        update_config(cx, move |c| {
                            let c = &mut c.compositor;
                            c.look.gaps_follow_bar = false;
                            match look {
                                Some((i, o, r, b)) => {
                                    (c.look.gaps_inner, c.look.gaps_outer, c.look.rounding, c.border_width) = (Some(i), Some(o), Some(r), Some(b));
                                }
                                None => {
                                    (c.look.gaps_inner, c.look.gaps_outer, c.look.rounding, c.border_width) = (None, None, None, None);
                                }
                            }
                        });
                    }))
                    .into_any_element()
            })
            .collect();
        let rules = div().row().flex_wrap().gap(px(SPACE_6_PX)).children(RULES.iter().map(|r| {
            let added = has_rule(c, r);
            let (app_id, title, global) = (r.app_id, r.title, r.global);
            chip(SharedString::from(format!("rule-preset-{}", r.id)), tr(r.label), added, p)
                .child(icon(if added { "check" } else { "plus" }, ICON_11_PX, if added { p.on_primary } else { p.primary }))
                .tooltip(tip(if added { t("Already in your rules below") } else { t("Adds a window rule (edit or remove it below)") }, p))
                .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                    if added {
                        return;
                    }
                    update_config(cx, move |c| {
                        c.compositor.window_rules.push(WindowRule { app_id: app_id.map(String::from), title: title.map(String::from), floating: Some(true), global: global.then_some(true), ..Default::default() });
                    });
                }))
        }));
        card(p)
            .child(heading(t("Window look"), t("Gaps, corner radius and border width in one click; fine-tune them under Window look."), p))
            .child(super::grid::fit(self.page.column_w - 2.0 * SPACE_18_PX, LOOK_GRID).balanced(LOOKS.len()).render(looks))
            .child(div().h(px(SPACE_4_PX)))
            .child(group_title(t("Common window rules"), p).px(px(0.0)))
            .child(rules)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rule_presets_are_valid_regexes_and_matchers() {
        for r in RULES {
            assert!(r.app_id.is_some() || r.title.is_some(), "{}", r.id);
            for re in [r.app_id, r.title].into_iter().flatten() {
                assert!(super::super::compositor_page::regex_problem(re).is_none(), "{} → {re}", r.id);
            }
        }
    }

    #[test]
    fn look_presets_round_trip() {
        let mut c = CompositorConfig::default();
        assert_eq!(current_look(&c), None);
        (c.look.gaps_inner, c.look.gaps_outer, c.look.rounding, c.border_width) = (Some(8), Some(12), Some(12), Some(2));
        assert_eq!(current_look(&c), LOOKS.iter().find(|l| l.id == "balanced").unwrap().look);
        assert!(!has_rule(&c, &RULES[0]));
    }

    /// The rule a preset chip adds (as its click handler builds it).
    fn preset_rule(r: &RulePreset) -> WindowRule {
        WindowRule { app_id: r.app_id.map(String::from), title: r.title.map(String::from), floating: Some(true), global: r.global.then_some(true), ..Default::default() }
    }

    /// Every look and rule preset reaches each compositor's drop-in, Niri's
    /// as valid KDL and MangoWM's accepted by `mango -p` (when installed).
    #[test]
    fn presets_produce_valid_drop_ins() {
        use mandara_config::compositor::Target;
        let mut c = CompositorConfig { keybinds: Vec::new(), ..Default::default() };
        c.window_rules = RULES.iter().map(preset_rule).collect();
        assert!(RULES.iter().all(|r| has_rule(&c, r)));
        for preset in LOOKS.iter().filter_map(|l| l.look) {
            (c.look.gaps_inner, c.look.gaps_outer, c.look.rounding, c.border_width) = (Some(preset.0), Some(preset.1), Some(preset.2), Some(preset.3));
            let mango = super::super::compositor_page::preview_text(Target::Mango, &c, None);
            for needle in [format!("gap_inner_horizontal={}", preset.0), format!("border_radius={}", preset.2), format!("border_px={}", preset.3)] {
                assert!(mango.contains(&needle), "{needle}\n{mango}");
            }
            assert_eq!(mango.matches("window_rule=").count(), RULES.len());
            let niri = super::super::compositor_page::preview_text(Target::Niri, &c, None);
            kdl::KdlDocument::parse_v1(&niri).unwrap_or_else(|e| panic!("{e:?}\n{niri}"));
            assert!(niri.contains(&format!("width {}", preset.3)), "{niri}");
            let hypr = mandara_config::compositor::emit_hyprland_conf(&mandara_config::compositor::ManagedConfig {
                config: &c,
                borders: None,
                existing_chords: Default::default(),
                mandara_bin: "mandara".into(),
                version: None,
                user_autostarts: false,
            })
            .text;
            assert!(hypr.contains(&format!("border_size = {}", preset.3)) && hypr.contains("pin on"), "{hypr}");
            if std::process::Command::new("mango").arg("-v").output().is_ok() {
                let dir = tempfile::tempdir().unwrap();
                let path = dir.path().join("mango.conf");
                std::fs::write(&path, &mango).unwrap();
                let out = std::process::Command::new("mango").arg("-p").arg("-c").arg(&path).output().unwrap();
                let report = String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
                assert!(!report.contains("ERROR"), "mango -p rejected a preset:\n{report}\n{mango}");
            }
        }
    }
}
