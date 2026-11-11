//! The bar, the side edges (side bars and the sidebar), panels and edge
//! actions in focused tabs.

use gpui::{AnyElement, Context, IntoElement, ParentElement, Styled, Window, div, px};
use mandara_config::ShellConfig;
use mandara_i18n::tr;
use mandara_theme::MandaraPalette;
use mandara_ui::bidi::Bidi as _;

use super::{StudioView, page_tabs};
use crate::registry::{self, Section, bars::BarsTab};
use crate::tokens::*;

mod sidebars;

impl page_tabs::PageTab for BarsTab {
    fn slug(self) -> &'static str { BarsTab::slug(self) }
    fn label(self) -> String { tr(self.title()) }
    fn icon(self) -> &'static str { BarsTab::icon(self) }
}

impl StudioView {
    /// Uses normal page navigation to open Bars, then selects the destination
    /// and starts its content at the top.
    pub fn show_bars_tab(&mut self, tab: BarsTab, cx: &mut Context<Self>) {
        self.show(Section::Bars, cx);
        self.bars_tab = tab;
        sidebars::forget_scroll(cx);
        self.content_scroll.set_offset(gpui::point(px(0.0), px(0.0)));
        cx.notify();
    }

    /// Renders the selected tab's registry settings and, where applicable,
    /// its layout editor. All controls retain their existing config paths.
    pub(super) fn bars_page(&mut self, cfg: &ShellConfig, p: &MandaraPalette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let tab = self.bars_tab;
        let tabs = page_tabs::bar("bars-tabs", &BarsTab::ALL, tab, p, |view, tab, _, cx| view.show_bars_tab(tab, cx), window, cx);
        let body = match tab {
            BarsTab::Bar => {
                let rows = self.rows(registry::in_section(Section::Bars).filter(|setting| BarsTab::for_path(setting.path) == Some(tab)), cfg, p, cx);
                let designer = self.bar_designer(cfg, p, window, cx);
                let displays = self.displays_link(mandara_config::outputs::SurfaceKind::Bar, cfg, p, cx);
                div().row().flex_col().gap(px(SPACE_26_PX)).child(designer).child(rows).child(displays).into_any_element()
            }
            BarsTab::Sidebars => self.sidebars_tab(cfg, p, window, cx),
            BarsTab::Panels | BarsTab::Edges => self.rows(registry::in_section(Section::Bars).filter(|setting| BarsTab::for_path(setting.path) == Some(tab)), cfg, p, cx),
        };
        div().row().flex_col().gap(px(SPACE_22_PX)).child(tabs).child(body).into_any_element()
    }
}
