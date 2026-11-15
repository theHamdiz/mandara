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

impl StudioView {
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
                let rows = self.rows(registry::in_section(Section::LockScreen), cfg, p, cx);
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
                    .gap(px(SPACE_26_PX))
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
