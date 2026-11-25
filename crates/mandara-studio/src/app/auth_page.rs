//! Login and session-lock settings share one page, with independent tabs.
use super::page_tabs::{self, PageTab};
use super::*;

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum AuthTab {
    #[default]
    Login,
    Lock,
}
impl AuthTab {
    pub const ALL: [Self; 2] = [Self::Login, Self::Lock];
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "login" => Some(Self::Login),
            "lock" => Some(Self::Lock),
            _ => None,
        }
    }
}
impl PageTab for AuthTab {
    fn slug(self) -> &'static str {
        match self {
            Self::Login => "login",
            Self::Lock => "lock",
        }
    }
    fn label(self) -> String {
        t(match self {
            Self::Login => "Login",
            Self::Lock => "Lock",
        })
        .into()
    }
    fn icon(self) -> &'static str {
        match self {
            Self::Login => "key",
            Self::Lock => "lock",
        }
    }
}

/// Keep old section links usable while presenting one navigation entry.
pub(super) fn navigation(section: Section, current: AuthTab) -> (Section, AuthTab) {
    match section {
        Section::LockScreen => (Section::LoginScreen, AuthTab::Lock),
        Section::LoginScreen => (Section::LoginScreen, AuthTab::Login),
        _ => (section, current),
    }
}

/// The lock design setting, shown as the gallery instead of a list row.
const LOCK_DESIGN_PATH: &str = "/lock/design";

impl StudioView {
    /// The lock screen's designs as the Login looks show theirs: each one
    /// in miniature, the chosen one ringed. A click picks it for the lock
    /// screen only.
    fn lock_design_gallery(&mut self, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        use mandara_config::auth_design::{AuthDesign, LOOKS};
        let designs = self.lock_designs.get_or_insert_with(|| {
            LOOKS
                .iter()
                .map(|(id, _, _)| {
                    let mut probe = mandara_greeter::config::GreeterConfig::default();
                    probe.apply_look(id);
                    let pal = mandara_greeter::view::palette_for(&probe);
                    (probe, pal)
                })
                .collect()
        }).clone();
        let (fit, thumb_w, thumb_h) = greeter_page::look_grid(self.page.column_w - 2.0 * SPACE_10_PX);
        let current = cfg.lock.design;
        let cards = LOOKS.iter().zip(designs.iter()).enumerate().map(|(i, ((id, name, summary), (probe, pal)))| {
            let design = AuthDesign::parse(id).unwrap_or_default();
            greeter_page::look_card(("lock-design", i), greeter_page::look_thumb::thumb(probe, pal, thumb_w, thumb_h), design == current, tr(name), t("In use").into(), tr(summary), p)
                .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| update_config(cx, move |c| c.lock.design = design)))
                .into_any_element()
        });
        let help = registry::by_path(LOCK_DESIGN_PATH).map(|s| SharedString::from(tr(s.help)));
        grouped::section(grouped::header(t("Design"), help, Vec::new(), p), grouped::group(vec![div().p(px(SPACE_10_PX)).child(fit.render(cards)).into_any_element()], false, p)).into_any_element()
    }

    pub(crate) fn show_auth_tab(&mut self, tab: AuthTab, cx: &mut Context<Self>) {
        self.show(
            match tab {
                AuthTab::Login => Section::LoginScreen,
                AuthTab::Lock => Section::LockScreen,
            },
            cx,
        );
    }

    pub(super) fn auth_page(
        &mut self,
        cfg: &ShellConfig,
        p: &MandaraPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let tabs = page_tabs::bar(
            "auth-tabs",
            &AuthTab::ALL,
            self.auth_tab,
            p,
            |view, tab, _, cx| view.show_auth_tab(tab, cx),
            window,
            cx,
        );
        let body = match self.auth_tab {
            AuthTab::Login => greeter_page::element(cx),
            AuthTab::Lock => {
                let designs = self.lock_design_gallery(cfg, p, cx);
                let rows = self.rows(registry::in_section(Section::LockScreen).filter(|s| s.path != LOCK_DESIGN_PATH), cfg, p, cx);
                let widgets = if registry::running_compositor()
                    == Some(mandara_config::compositor::Target::KWin)
                {
                    compositor_page::card(p).p(px(SPACE_16_PX)).child(compositor_page::heading(t("Widgets"), t("Plasma's secure locker controls this screen. Custom lock widgets are available with Mandara's native locker on supported compositors."), p)).into_any_element()
                } else {
                    auth_widgets::panel(
                        "lock",
                        &cfg.lock.widgets,
                        Rc::new(|edit, cx| {
                            update_config(cx, |cfg| edit.apply(&mut cfg.lock.widgets))
                        }),
                        p,
                        cx,
                    )
                };
                div()
                    .row()
                    .flex_col()
                    .gap(px(GROUP_SECTION_GAP_PX))
                    .child(designs)
                    .child(rows)
                    .child(widgets)
                    .into_any_element()
            }
        };
        div()
            .w_full()
            .min_w(px(0.0))
            .row()
            .flex_col()
            .gap(px(SPACE_22_PX))
            .child(tabs)
            .child(body)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn old_links_select_the_right_tab_without_creating_another_page() {
        assert_eq!(
            navigation(Section::LockScreen, AuthTab::Login),
            (Section::LoginScreen, AuthTab::Lock)
        );
        assert_eq!(
            navigation(Section::LoginScreen, AuthTab::Lock),
            (Section::LoginScreen, AuthTab::Login)
        );
        assert_eq!(
            navigation(Section::Themes, AuthTab::Lock),
            (Section::Themes, AuthTab::Lock)
        );
    }
}
