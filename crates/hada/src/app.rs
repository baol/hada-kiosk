//! UI-thread application state: owns the Slint models, applies Home Assistant
//! events to them and turns UI actions into service calls.

use crate::build_ui::{self, Built, CardSlot, Ctx, RowRef};
use crate::{ActionKind, Actions, AppWindow};
use hada_core::actions::{header_toggle, Action, EntityAction};
use hada_core::ha::{HaCommand, HaEvent};
use hada_core::icons::{IconResolver, IconResources};
use hada_core::model::{ActionSpec, Dashboard};
use hada_core::state::StateStore;
use hada_core::timefmt::Locale;
use hada_core::translations::Translations;
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc::UnboundedSender;

thread_local! {
    static APP: RefCell<Option<Rc<RefCell<App>>>> = const { RefCell::new(None) };
}

/// Run `f` with the application state (UI thread only).
pub fn with_app(f: impl FnOnce(&mut App)) {
    APP.with(|a| {
        if let Some(app) = a.borrow().as_ref() {
            f(&mut app.borrow_mut());
        }
    });
}

pub struct App {
    ui: slint::Weak<AppWindow>,
    cmd_tx: Option<UnboundedSender<HaCommand>>,
    pub store: StateStore,
    pub icons: IconResolver,
    dashboard: Option<Dashboard>,
    built: Option<Built>,
    views_model: Rc<VecModel<crate::ViewData>>,
    content_width: f32,
    initial_view: Option<String>,
    icon_cache_path: Option<std::path::PathBuf>,
    pub translations: Option<Translations>,
    translations_cache_path: Option<std::path::PathBuf>,
    locale: &'static Locale,
    calendar_events: std::collections::HashMap<String, Vec<hada_core::ha::ws::CalendarEvent>>,
}

impl App {
    /// Create the state, install it in the thread-local and wire the UI callbacks.
    pub fn install(
        ui: &AppWindow,
        cmd_tx: Option<UnboundedSender<HaCommand>>,
        initial_view: Option<String>,
        cache_dir: Option<std::path::PathBuf>,
        language: &str,
    ) -> Rc<RefCell<App>> {
        let icon_cache_path = cache_dir.as_ref().map(|d| d.join("icons.json"));
        let translations_cache_path = cache_dir
            .as_ref()
            .map(|d| d.join(format!("translations-{language}.json")));
        let translations = translations_cache_path
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| Translations::from_json_str(&s).ok())
            .filter(|t| !t.is_empty());
        let views_model = Rc::new(VecModel::from(Vec::<crate::ViewData>::new()));
        ui.set_views(ModelRc::from(views_model.clone()));
        let mut icons = IconResolver::default();
        if let Some(p) = &icon_cache_path {
            if let Ok(s) = std::fs::read_to_string(p) {
                if let Ok(r) = IconResources::from_json_str(&s) {
                    tracing::info!("loaded cached icon resources from {}", p.display());
                    icons.set_resources(r);
                }
            }
        }
        let app = Rc::new(RefCell::new(App {
            ui: ui.as_weak(),
            cmd_tx,
            store: StateStore::new(),
            icons,
            dashboard: None,
            built: None,
            views_model,
            content_width: 900.0,
            initial_view,
            icon_cache_path,
            translations,
            translations_cache_path,
            locale: Locale::for_language(language),
            calendar_events: std::collections::HashMap::new(),
        }));
        APP.with(|a| *a.borrow_mut() = Some(app.clone()));

        ui.global::<Actions>()
            .on_action(|entity_id, kind, value, card_id| {
                with_app(|app| app.on_action(entity_id.as_str(), kind, value, card_id));
            });
        ui.on_relayout(|width| {
            with_app(|app| app.on_relayout(width));
        });
        // The kiosk window never resizes, so `changed avail` never fires there:
        // take the initial width now, and once more when the event loop runs.
        let initial = ui.get_avail();
        tracing::info!("initial content width {initial} px");
        with_app(|app| app.on_relayout(initial));
        let weak = ui.as_weak();
        slint::Timer::single_shot(Duration::ZERO, move || {
            if let Some(ui) = weak.upgrade() {
                let w = ui.get_avail();
                with_app(|app| app.on_relayout(w));
            }
        });
        app
    }

    pub fn ui(&self) -> Option<AppWindow> {
        self.ui.upgrade()
    }

    fn set_status(&self, text: &str, connected: bool) {
        if let Some(ui) = self.ui() {
            ui.set_status_text(text.into());
            ui.set_connected(connected);
        }
    }

    pub fn handle(&mut self, ev: HaEvent) {
        match ev {
            HaEvent::Connecting { attempt } => {
                if attempt > 1 {
                    self.set_status(&format!("Reconnecting ({attempt})…"), false);
                } else {
                    self.set_status("Connecting…", false);
                }
            }
            HaEvent::Connected { ha_version } => self.set_status(&format!("HA {ha_version}"), true),
            HaEvent::Dashboard(d) => self.set_dashboard(d),
            HaEvent::Icons(r) => {
                if let Some(p) = &self.icon_cache_path {
                    if let Some(parent) = p.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    if let Ok(s) = serde_json::to_string(&r) {
                        let _ = std::fs::write(p, s);
                    }
                }
                self.icons.set_resources(r);
                self.refresh_all();
            }
            HaEvent::Translations(t) => {
                if let Some(p) = &self.translations_cache_path {
                    if let Some(parent) = p.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    if let Ok(s) = serde_json::to_string(&t) {
                        let _ = std::fs::write(p, s);
                    }
                }
                self.translations = Some(t);
                self.refresh_all();
            }
            HaEvent::Calendar { entity_id, events } => {
                self.calendar_events
                    .insert(entity_id.clone(), events.clone());
                let now = hada_core::timefmt::now_unix();
                if let Some(refs) = self
                    .built
                    .as_ref()
                    .and_then(|b| b.calendars.get(entity_id.as_str()))
                {
                    for r in refs {
                        let (rows, subtitle) =
                            build_ui::calendar_rows(&events, now, r.days, self.locale);
                        r.rows.set_vec(rows);
                        let title = r
                            .title_from
                            .as_ref()
                            .map(|e| build_ui::display_name(e, self.store.get(&e.entity_id)));
                        if let Some(slot) = &r.slot {
                            slot.update(|leaf| {
                                leaf.subtitle = subtitle.as_str().into();
                                if let Some(t) = &title {
                                    leaf.title = t.as_str().into();
                                }
                            });
                        }
                    }
                }
            }
            HaEvent::Entities(ev) => {
                let changed = self.store.apply(ev);
                for id in changed {
                    self.refresh_entity(&id);
                }
            }
            HaEvent::CallFailed { call, error } => {
                self.set_status(
                    &format!("{}.{} failed: {error}", call.domain, call.service),
                    true,
                );
                self.refresh_all();
            }
            HaEvent::Disconnected { reason, retry_in } => {
                self.set_status(
                    &format!("Offline: {reason} (retry {}s)", retry_in.as_secs()),
                    false,
                );
            }
            HaEvent::AuthFailed(msg) => self.set_status(&format!("Auth failed: {msg}"), false),
        }
    }

    pub fn set_dashboard(&mut self, d: Dashboard) {
        if self.dashboard.as_ref() == Some(&d) {
            return;
        }
        tracing::info!("building UI for {} views", d.views.len());
        self.dashboard = Some(d);
        self.rebuild();
    }

    /// Width available to the content, read from the window itself (logical px).
    /// Mirrors `avail` in app.slint: full width in portrait, minus the side rail otherwise.
    fn window_avail(&self) -> Option<f32> {
        let ui = self.ui()?;
        let size = ui.window().size();
        let scale = ui.window().scale_factor().max(0.1);
        let (w, h) = (size.width as f32 / scale, size.height as f32 / scale);
        if w <= 0.0 || h <= 0.0 {
            return None;
        }
        Some(if w < h { w } else { w - 104.0 })
    }

    fn rebuild(&mut self) {
        let Some(d) = self.dashboard.clone() else {
            return;
        };
        let d = &d;
        if let Some(w) = self.window_avail() {
            self.content_width = w;
        }
        tracing::info!("layout for content width {} px", self.content_width);
        let built = build_ui::build(
            d,
            Ctx {
                store: &self.store,
                icons: &self.icons,
                translations: self.translations.as_ref(),
                locale: self.locale,
            },
            self.content_width,
        );
        let views: Vec<crate::ViewData> = built.views.iter().map(|v| v.data.clone()).collect();
        self.views_model.set_vec(views);
        self.built = Some(built);
        // A rebuilt calendar card starts empty; refill it from the last fetch.
        for (entity_id, events) in self.calendar_events.clone() {
            self.handle(HaEvent::Calendar { entity_id, events });
        }
        if let Some(ui) = self.ui() {
            if let Some(key) = self.initial_view.take() {
                if let Some(i) = d.view_index(&key) {
                    ui.set_current_view(i as i32);
                } else {
                    tracing::warn!("view {key:?} not found; showing the first view");
                }
            }
            let cur = ui.get_current_view();
            if cur < 0 || cur as usize >= d.views.len() {
                ui.set_current_view(0);
            }
        }
    }

    fn on_relayout(&mut self, width: f32) {
        if width <= 0.0 || (width - self.content_width).abs() < 1.0 {
            return;
        }
        tracing::info!("content width {width} px");
        self.content_width = width;
        let Some(built) = &mut self.built else { return };
        let mut rebuild = false;
        for (i, v) in built.views.iter_mut().enumerate() {
            if let Some(cards) = &v.masonry {
                let n = build_ui::masonry_columns(width);
                if v.columns != n {
                    v.columns = n;
                    v.rows_model.set_vec(build_ui::masonry_rows(cards, n));
                    if let Some(mut data) = self.views_model.row_data(i) {
                        data.max_columns = n as i32;
                        self.views_model.set_row_data(i, data);
                    }
                }
            } else if let (Some(sections), Some(key)) = (&v.sections, &v.sections_key) {
                let spans: Vec<u32> = sections.iter().map(|(_, span)| *span).collect();
                if build_ui::sections_key(&spans, v.max_columns, width) != *key {
                    rebuild = true; // card density changes: regroup the cards
                }
            }
        }
        if rebuild {
            self.rebuild();
        }
    }

    fn refresh_all(&mut self) {
        let ids: Vec<Arc<str>> = self
            .built
            .as_ref()
            .map(|b| b.index.keys().cloned().collect())
            .unwrap_or_default();
        for id in ids {
            self.refresh_entity(&id);
        }
    }

    fn refresh_entity(&mut self, id: &Arc<str>) {
        let Some(built) = &self.built else { return };
        let Some(refs) = built.index.get(id) else {
            return;
        };
        let st = self.store.get(id);
        for r in refs {
            match r {
                RowRef::Row { model, index, spec } => {
                    if let Some(mut row) = model.row_data(*index) {
                        let keep_id = row.entity_id.clone();
                        build_ui::fill_row(
                            &mut row,
                            spec,
                            st,
                            &self.icons,
                            self.translations.as_ref(),
                        );
                        if keep_id.starts_with("card:") {
                            row.entity_id = keep_id; // button cards route through a synthetic key
                        }
                        model.set_row_data(*index, row);
                    }
                }
                RowRef::Feature {
                    model,
                    index,
                    entity,
                    feature,
                } => {
                    if let Some(mut f) = model.row_data(*index) {
                        build_ui::fill_feature(&mut f, entity, feature, st);
                        model.set_row_data(*index, f);
                    }
                }
                RowRef::Header { card_id } => {
                    let ctx = &built.cards[*card_id];
                    let on = build_ui::header_state(&ctx.header_ids, &self.store);
                    if let Some(slot) = &ctx.slot {
                        slot.update(|leaf| leaf.header_toggle = i32::from(on));
                    }
                }
            }
        }
    }

    fn mark_pending(&mut self, id: &Arc<str>) {
        let Some(built) = &self.built else { return };
        let Some(refs) = built.index.get(id) else {
            return;
        };
        for r in refs {
            match r {
                RowRef::Row { model, index, .. } => {
                    if let Some(mut row) = model.row_data(*index) {
                        row.pending = true;
                        model.set_row_data(*index, row);
                    }
                }
                RowRef::Feature { model, index, .. } => {
                    if let Some(mut f) = model.row_data(*index) {
                        f.pending = true;
                        model.set_row_data(*index, f);
                    }
                }
                RowRef::Header { .. } => {}
            }
        }
        // Clear the pending marker if no state change arrives in time.
        let id = id.clone();
        slint::Timer::single_shot(Duration::from_secs(5), move || {
            with_app(|app| app.refresh_entity(&id));
        });
    }

    fn send(&self, call: hada_core::actions::ServiceCall) {
        tracing::info!(
            "call_service {}.{} {:?}",
            call.domain,
            call.service,
            call.target
        );
        match &self.cmd_tx {
            Some(tx) => {
                if tx.send(HaCommand::CallService(call)).is_err() {
                    tracing::error!("network thread is gone");
                }
            }
            None => tracing::info!("(no connection; action dropped)"),
        }
    }

    fn on_action(&mut self, entity_id: &str, kind: ActionKind, value: f32, card_id: i32) {
        let pct = value.round().clamp(0.0, 255.0) as u8;
        let action = match kind {
            ActionKind::Toggle => Some(Action::Toggle),
            ActionKind::TurnOn => Some(Action::TurnOn),
            ActionKind::TurnOff => Some(Action::TurnOff),
            ActionKind::SetBrightness => Some(Action::SetBrightnessPct(pct)),
            ActionKind::SetFan => Some(Action::SetPercentage(pct)),
            ActionKind::SetCoverPosition => Some(Action::SetCoverPosition(pct)),
            ActionKind::SetCoverTilt => Some(Action::SetCoverTilt(pct)),
            ActionKind::SetNumber => Some(Action::SetValue(value as f64)),
            ActionKind::SetTemperature => {
                Some(Action::SetTemperature((value as f64 * 2.0).round() / 2.0))
            }
            ActionKind::Open => Some(Action::OpenCover),
            ActionKind::Close => Some(Action::CloseCover),
            ActionKind::Stop => Some(Action::StopCover),
            ActionKind::Press => Some(Action::Press),
            ActionKind::Run => Some(Action::Run),
            ActionKind::Install => Some(Action::Install),
            ActionKind::HeaderToggle => {
                let Some(ctx) = self
                    .built
                    .as_ref()
                    .and_then(|b| b.cards.get(card_id as usize))
                else {
                    return;
                };
                let ids = ctx.header_ids.clone();
                let on = value >= 0.5;
                if let Some(slot) = ctx.slot.clone() {
                    CardSlot::update(&slot, |leaf| leaf.header_toggle = i32::from(on));
                }
                for id in &ids {
                    self.mark_pending(id);
                }
                self.send(header_toggle(&ids, on));
                return;
            }
            ActionKind::RowAction => {
                let Some(ctx) = self
                    .built
                    .as_ref()
                    .and_then(|b| b.cards.get(card_id as usize))
                else {
                    return;
                };
                let Some((entity, spec)) = ctx.row_actions.get(entity_id).cloned() else {
                    tracing::warn!("no action configured for {entity_id}");
                    return;
                };
                let target = entity.as_deref().unwrap_or("");
                let ea = match (&spec, entity.as_ref()) {
                    (Some(ActionSpec::PerformAction { .. }) | Some(ActionSpec::Toggle), _) => {
                        EntityAction::from_spec(target, spec.as_ref().unwrap())
                    }
                    (None, Some(e)) => Some(EntityAction::new(e, Action::Toggle)),
                    _ => None,
                };
                match ea.and_then(|a| a.to_service_call()) {
                    Some(call) => {
                        if let Some(e) = entity {
                            self.mark_pending(&e);
                        }
                        self.send(call);
                    }
                    None => {
                        tracing::info!("action for {entity_id} is not a service call ({spec:?})")
                    }
                }
                return;
            }
        };
        let Some(action) = action else { return };
        // Row-level tap_action overrides for toggles (e.g. perform-action on an entities row).
        if matches!(action, Action::Toggle) {
            if let Some(ctx) = self
                .built
                .as_ref()
                .and_then(|b| b.cards.get(card_id as usize))
            {
                if let Some((_, Some(spec))) = ctx.row_actions.get(entity_id) {
                    if let Some(ea) = EntityAction::from_spec(entity_id, spec) {
                        if let Some(call) = ea.to_service_call() {
                            self.mark_pending(&Arc::from(entity_id));
                            self.send(call);
                        }
                        return;
                    }
                }
            }
        }
        let ea = EntityAction::new(entity_id, action);
        match ea.to_service_call() {
            Some(call) => {
                self.mark_pending(&ea.entity_id);
                self.send(call);
            }
            None => tracing::warn!("no service for {:?} on {entity_id}", ea.action),
        }
    }
}
