//! Date, time & location → Clock: the system time zone and network time,
//! through systemd-timedated ([`mandara_services::timedate`]; changes ask
//! for your password through polkit). When the clock's time zone doesn't
//! match the shared location's, a warning offers the fix in one click: a
//! wrong time zone shifts the bar, the lock screen and prayer times alike.
//!
//! The location's time zone comes from the shell
//! ([`mandara_ui::timezone::location_timezone`]); Studio on its own looks
//! it up once from the coordinates (Open-Meteo, only with weather on).

use gpui::{AnyElement, AppContext as _, ClickEvent, Context, Entity, FontWeight, Global, InteractiveElement, IntoElement, ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder as _, px};
use mandara_core::ToGpui;
use mandara_i18n::{t, tf};
use mandara_services::timedate::{self, TimeStatus};
use mandara_theme::MandaraPalette;
use mandara_ui::bidi::Bidi as _;
use mandara_ui::icons::icon;
use mandara_ui::text_field::TextField;

use super::StudioView;
use crate::tokens::*;

/// `mandara studio date --demo`: a fictional clock (Riyadh's time zone in
/// Alexandria, the mismatch case); nothing on the system changes.
static DEMO: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// The demo's location time zone.
const DEMO_LOCATION_ZONE: &str = "Africa/Cairo";

/// Switches the card to its demo clock (call before Studio opens).
pub fn set_demo() {
    DEMO.store(true, std::sync::atomic::Ordering::Relaxed);
}

fn demo() -> bool {
    DEMO.load(std::sync::atomic::Ordering::Relaxed)
}

/// Time zones listed under the search field.
const MATCHES_SHOWN: usize = 8;
/// Coordinates are compared at ~1 km (as the shared location is stored).
const COORD_SCALE: f64 = 100.0;

/// The location's coordinates as a lookup key.
fn coord_key(latitude: f64, longitude: f64) -> (i64, i64) {
    ((latitude * COORD_SCALE).round() as i64, (longitude * COORD_SCALE).round() as i64)
}

/// The first zones containing `query` (any case), preferring a match at
/// the start of the city part (`cai` → `Africa/Cairo`).
pub(super) fn matching<'a>(zones: &'a [String], query: &str) -> Vec<&'a str> {
    let q = query.trim().to_lowercase().replace(' ', "_");
    if q.is_empty() {
        return Vec::new();
    }
    let city = |z: &str| z.rsplit('/').next().unwrap_or(z).to_lowercase();
    let mut hits: Vec<&str> = zones.iter().map(String::as_str).filter(|z| z.to_lowercase().contains(&q)).collect();
    hits.sort_by_key(|z| !city(z).starts_with(&q));
    hits.truncate(MATCHES_SHOWN);
    hits
}

pub(super) struct TimeCard {
    status: Option<Result<TimeStatus, String>>,
    zones: Vec<String>,
    picking: bool,
    search: Option<Entity<TextField>>,
    query: String,
    busy: bool,
    message: Option<(bool, String)>,
    /// The shared location's time zone looked up here (by coordinates).
    looked_up: Option<((i64, i64), String)>,
    looking: bool,
}

struct CardHandle(Entity<TimeCard>);

impl Global for CardHandle {}

/// The card for the Date, time & location page (one per Studio process).
pub(super) fn element(cx: &mut Context<StudioView>) -> AnyElement {
    let card = match cx.try_global::<CardHandle>() {
        Some(h) => h.0.clone(),
        None => {
            let card = cx.new(TimeCard::new);
            cx.set_global(CardHandle(card.clone()));
            card
        }
    };
    card.into_any_element()
}

impl TimeCard {
    fn new(cx: &mut Context<Self>) -> Self {
        let mut card = Self { status: None, zones: Vec::new(), picking: false, search: None, query: String::new(), busy: false, message: None, looked_up: None, looking: false };
        card.refresh(cx);
        card
    }

    /// Reads the clock (and, once, the zone list) off the UI thread.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        if demo() {
            self.status = Some(Ok(self.status.clone().and_then(Result::ok).unwrap_or(TimeStatus { timezone: "Asia/Riyadh".into(), ntp: true, can_ntp: true, synced: true })));
            if self.zones.is_empty() {
                self.zones = chrono_tz::TZ_VARIANTS.iter().map(|z| z.name().to_string()).collect();
            }
            return cx.notify();
        }
        let need_zones = self.zones.is_empty();
        let task = cx.background_executor().spawn(async move { mandara_services::runtime::block_on(async { (timedate::status().await, if need_zones { timedate::zones().await.ok() } else { None }) }) });
        cx.spawn(async move |this, cx| {
            let (status, zones) = task.await;
            let _ = this.update(cx, |this, cx| {
                this.status = Some(status);
                if let Some(zones) = zones {
                    this.zones = zones;
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Runs a change (it asks for your password), then reads the clock again.
    fn change(&mut self, done: String, job: impl std::future::Future<Output = Result<(), String>> + Send + 'static, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        if demo() {
            self.message = Some((true, t("Dry run: nothing was changed.").to_string()));
            return cx.notify();
        }
        self.busy = true;
        self.message = Some((true, t("Waiting for authorization…").to_string()));
        let task = cx.background_executor().spawn(async move { mandara_services::runtime::block_on(job) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                this.message = Some(match result {
                    Ok(()) => (true, done),
                    Err(e) => (false, e),
                });
                this.refresh(cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn set_zone(&mut self, zone: String, cx: &mut Context<Self>) {
        self.picking = false;
        let done = tf("Time zone set to {zone}", &[("zone", &zone)]);
        self.change(done, async move { timedate::set_timezone(&zone).await }, cx);
    }

    fn set_ntp(&mut self, on: bool, cx: &mut Context<Self>) {
        let done = if on { t("The clock now sets itself from internet time servers") } else { t("The clock keeps its own time now") }.to_string();
        self.change(done, async move { timedate::set_ntp(on).await }, cx);
    }

    /// The shared location's time zone: the shell's answer, else one
    /// lookup here by coordinates (remembered, and saved for the shell).
    fn location_zone(&mut self, cx: &mut Context<Self>) -> String {
        if demo() {
            return DEMO_LOCATION_ZONE.into();
        }
        let known = mandara_ui::timezone::location_timezone(cx);
        if !known.is_empty() {
            return known;
        }
        let cfg = mandara_ui::state::config(cx).read(cx).snapshot();
        let key = coord_key(cfg.location.latitude, cfg.location.longitude);
        if let Some((k, zone)) = &self.looked_up
            && *k == key
        {
            return zone.clone();
        }
        if !self.looking && cfg.weather.enabled && !mandara_services::offline::is_offline() {
            self.looking = true;
            let (lat, lon) = (cfg.location.latitude, cfg.location.longitude);
            let task = cx.background_executor().spawn(async move { mandara_services::weather::timezone_at(lat, lon) });
            cx.spawn(async move |this, cx| {
                let result = task.await;
                let _ = this.update(cx, |this, cx| {
                    this.looking = false;
                    match result {
                        Ok(zone) if timedate::is_zone(&zone) => {
                            this.looked_up = Some((key, zone.clone()));
                            mandara_ui::state::update_config(cx, move |c| {
                                if c.location.timezone.is_empty() && coord_key(c.location.latitude, c.location.longitude) == key {
                                    c.location.timezone = zone;
                                }
                            });
                        }
                        Ok(_) => {}
                        Err(e) => tracing::info!("time zone lookup: {e}"),
                    }
                    cx.notify();
                });
            })
            .detach();
        }
        String::new()
    }

    fn search_field(&mut self, cx: &mut Context<Self>) -> Entity<TextField> {
        if let Some(f) = &self.search {
            return f.clone();
        }
        let card = cx.weak_entity();
        let field = cx.new(|cx| {
            TextField::new(
                String::new(),
                t("Search, e.g. Cairo or Europe"),
                true,
                std::rc::Rc::new(move |value, _, cx| {
                    let _ = card.update(cx, |this, cx| {
                        this.query = value;
                        cx.notify();
                    });
                }),
                cx,
            )
        });
        self.search = Some(field.clone());
        field
    }

    fn button(p: &MandaraPalette, id: impl Into<SharedString>, label: String, ic: &'static str, primary: bool) -> gpui::Stateful<gpui::Div> {
        div()
            .id(gpui::ElementId::Name(id.into()))
            .h(px(DIM_32_PX))
            .px(px(SPACE_12_PX))
            .flex_none()
            .row()
            .items_center()
            .gap(px(SPACE_7_PX))
            .rounded(px(RADIUS_11_PX))
            .cursor_pointer()
            .text_size(px(TEXT_12_5_PX))
            .font_weight(FontWeight::MEDIUM)
            .when(primary, |el| el.bg(p.primary.hsla()).text_color(p.on_primary.hsla()).hover(|s| s.opacity(ALPHA_90)))
            .when(!primary, |el| el.bg(p.surface_container_high.hsla()).hover(|s| s.bg(p.surface_container.hsla())))
            .child(icon(ic, ICON_15_PX, if primary { p.on_primary } else { p.on_surface }))
            .child(label)
    }

    fn entry(p: &MandaraPalette, ic: &'static str, name: String, sub: String, ok: bool, control: Option<AnyElement>) -> AnyElement {
        div()
            .min_h(px(DIM_52_PX))
            .px(px(SPACE_14_PX))
            .py(px(SPACE_8_PX))
            .row()
            .items_center()
            .gap(px(SPACE_12_PX))
            .child(div().size(px(DIM_32_PX)).flex_none().rounded(px(RADIUS_10_PX)).bg(p.surface_container_high.hsla()).row().items_center().justify_center().child(icon(ic, ICON_15_PX, if ok { p.success_emerald } else { p.primary })))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .row()
                    .flex_col()
                    .gap(px(SPACE_3_PX))
                    .child(div().font_weight(FontWeight::MEDIUM).text_color(p.on_surface.hsla()).child(name))
                    .child(div().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(sub)),
            )
            .children(control)
            .into_any_element()
    }

    /// The mismatch warning with its one-click fix.
    fn mismatch_banner(&self, p: &MandaraPalette, system: &str, zone: &str, place: &str, cx: &mut Context<Self>) -> AnyElement {
        let now = chrono::Utc::now();
        let label = |z: &str| timedate::offset_seconds(z, now).map(timedate::offset_label).unwrap_or_default();
        let place = if place.is_empty() { t("Your location").to_string() } else { place.to_string() };
        // Zone names don't wrap at their slash ("Asia/" + "Riyadh").
        let whole = |z: &str| z.replace('/', "/\u{2060}");
        let (zone_shown, system_shown) = (whole(zone), whole(system));
        let target = zone.to_string();
        let fix = Self::button(p, "time-fix", tf("Use {zone}", &[("zone", &zone)]), "check", true).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.set_zone(target.clone(), cx)));
        div()
            .row()
            .flex_wrap()
            .items_center()
            .gap(px(SPACE_12_PX))
            .p(px(SPACE_14_PX))
            .rounded(px(RADIUS_18_PX))
            .bg(p.warning_amber.with_alpha(ALPHA_12).hsla())
            .child(icon("alert", ICON_16_PX, p.warning_amber))
            .child(
                div()
                    .flex_1()
                    .min_w(px(DIM_220_PX))
                    .row()
                    .flex_col()
                    .gap(px(SPACE_3_PX))
                    .child(div().font_weight(FontWeight::SEMIBOLD).text_color(p.on_surface.hsla()).child(tf(
                        "{place} is in {zone} ({offset}), but your clock uses {system} ({system_offset})",
                        &[("place", &place), ("zone", &zone_shown), ("offset", &label(zone)), ("system", &system_shown), ("system_offset", &label(system))],
                    )))
                    .child(div().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(mandara_ui::bidi::paragraph(t("The bar, the lock screen and prayer times all follow the clock, so they show the wrong time there.")))),
            )
            .child(fix)
            .into_any_element()
    }
}

impl Render for TimeCard {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = mandara_core::frame_palette(window, cx);
        let place = mandara_ui::state::config(cx).read(cx).get().location.name.clone();
        let location_zone = self.location_zone(cx);
        let mut rows: Vec<AnyElement> = Vec::new();
        let mut banner = None;
        match self.status.clone() {
            None => rows.push(Self::entry(&p, "globe", t("Time zone").to_string(), t("Checking…").to_string(), false, None)),
            Some(Err(e)) => rows.push(Self::entry(&p, "globe", t("Time zone").to_string(), tf("Couldn't read the clock: {error}", &[("error", &e)]), false, None)),
            Some(Ok(s)) => {
                if let Some(zone) = timedate::mismatch(&s.timezone, &location_zone, chrono::Utc::now()) {
                    banner = Some(self.mismatch_banner(&p, &s.timezone, zone, &place, cx));
                }
                let offset = timedate::offset_seconds(&s.timezone, chrono::Utc::now()).map(timedate::offset_label).unwrap_or_default();
                let change = Self::button(&p, "time-zone-change", if self.picking { t("Close") } else { t("Change") }.to_string(), if self.picking { "close" } else { "pencil" }, false).on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.picking = !this.picking;
                    cx.notify();
                }));
                rows.push(Self::entry(&p, "globe", t("Time zone").to_string(), format!("{} · {offset}", s.timezone), true, Some(change.into_any_element())));
                if self.picking {
                    let field = self.search_field(cx);
                    let hits: Vec<String> = matching(&self.zones, &self.query).into_iter().map(str::to_string).collect();
                    let list = hits.into_iter().enumerate().map(|(i, zone)| {
                        let offset = timedate::offset_seconds(&zone, chrono::Utc::now()).map(timedate::offset_label).unwrap_or_default();
                        let pick = zone.clone();
                        div()
                            .id(("time-zone-pick", i))
                            .px(px(SPACE_12_PX))
                            .py(px(SPACE_8_PX))
                            .rounded(px(RADIUS_10_PX))
                            .row()
                            .justify_between()
                            .gap(px(SPACE_12_PX))
                            .cursor_pointer()
                            .hover(|s| s.bg(p.surface_container_high.hsla()))
                            .child(div().text_size(px(TEXT_13_PX)).child(zone.clone()))
                            .child(div().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).font_features(mandara_ui::fonts::tabular_nums()).child(offset))
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.set_zone(pick.clone(), cx)))
                    });
                    rows.push(div().px(px(SPACE_14_PX)).pb(px(SPACE_8_PX)).row().flex_col().gap(px(SPACE_4_PX)).child(field).children(list).into_any_element());
                }
                let (sub, ok) = match (s.can_ntp, s.ntp, s.synced) {
                    (false, _, _) => (t("No network time service is installed (for example systemd-timesyncd)"), false),
                    (true, true, true) => (t("On, and in sync with internet time servers"), true),
                    (true, true, false) => (t("On, waiting to reach a time server"), false),
                    (true, false, _) => (t("Off: the clock keeps its own time"), false),
                };
                let ntp = s.ntp;
                let toggle = s.can_ntp.then(|| div().id("time-ntp").cursor_pointer().child(super::form::switch(ntp, &p)).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.set_ntp(!ntp, cx))).into_any_element());
                rows.push(Self::entry(&p, "clock", t("Set time automatically").to_string(), sub.to_string(), ok, toggle));
            }
        }
        let refresh = Self::button(&p, "time-refresh", t("Refresh").to_string(), "refresh", false).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.refresh(cx)));
        div()
            .row()
            .flex_col()
            .gap(px(SPACE_6_PX))
            .child(div().px(px(SPACE_6_PX)).row().items_center().justify_between().child(div().text_size(px(TEXT_12_PX)).font_weight(FontWeight::SEMIBOLD).text_color(p.on_surface_variant.hsla()).child(t("System clock"))).child(refresh))
            .children(banner)
            .child(div().row().flex_col().py(px(SPACE_4_PX)).rounded(px(RADIUS_18_PX)).bg(p.surface_container.hsla()).children(rows))
            .when_some(self.message.clone(), |el, (ok, text)| el.child(div().px(px(SPACE_6_PX)).text_size(px(TEXT_12_PX)).text_color(if ok { p.on_surface_variant.hsla() } else { p.urgent_coral.hsla() }).child(text)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_prefers_the_city_and_ignores_case() {
        let zones: Vec<String> = ["Africa/Cairo", "America/Caracas", "Asia/Riyadh", "Europe/Berlin", "America/Argentina/Catamarca"].map(String::from).to_vec();
        assert_eq!(matching(&zones, "cai"), ["Africa/Cairo"]);
        assert_eq!(matching(&zones, "CA"), ["Africa/Cairo", "America/Caracas", "America/Argentina/Catamarca"]);
        assert_eq!(matching(&zones, "ri")[0], "Asia/Riyadh", "a city starting with the text comes before Africa / America");
        assert_eq!(matching(&zones, "europe"), ["Europe/Berlin"]);
        assert_eq!(matching(&zones, "new york").len(), 0);
        assert!(matching(&zones, "  ").is_empty());
        assert_eq!(coord_key(31.2, 29.92), (3120, 2992));
    }
}
