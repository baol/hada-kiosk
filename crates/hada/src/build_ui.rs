//! Turn a normalised [`Dashboard`] into Slint models, and keep the per-entity
//! rows up to date when states change.

use crate::mdi;
use crate::{
    CardData, CardRow, ColumnData, ControlKind, FeatureData, LeafCard, RowData, SectionRow,
    SliderKind, ViewData,
};
use hada_core::domain::{self, Control, SliderKind as CoreSliderKind};
use hada_core::icons::IconResolver;
use hada_core::model::*;
use hada_core::state::{EntityState, StateStore};
use hada_core::timefmt::Locale;
use hada_core::translations::Translations;
use slint::{Model, ModelRc, SharedString, VecModel};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

/// Width (logical px) one masonry column wants.
pub const MASONRY_COLUMN_WIDTH: f32 = 330.0;
pub const MASONRY_MAX_COLUMNS: usize = 5;
/// A sections-view column narrower than this is not worth splitting off: on a
/// portrait kiosk (600 or 800 px) every section gets the full width.
pub const SECTION_MIN_WIDTH: f32 = 420.0;
/// Target width of a half-size card (tile) in a section; decides 2, 3 or 4 per row.
pub const TILE_TARGET_WIDTH: f32 = 240.0;

/// Where a card's `LeafCard` lives, so it can be rewritten in place.
#[derive(Clone)]
pub enum CardSlot {
    Card(Rc<VecModel<CardData>>, usize),
    Leaf(Rc<VecModel<LeafCard>>, usize),
}

impl CardSlot {
    pub fn update(&self, f: impl FnOnce(&mut LeafCard)) {
        match self {
            CardSlot::Card(m, i) => {
                if let Some(mut c) = m.row_data(*i) {
                    f(&mut c.leaf);
                    m.set_row_data(*i, c);
                }
            }
            CardSlot::Leaf(m, i) => {
                if let Some(mut l) = m.row_data(*i) {
                    f(&mut l);
                    m.set_row_data(*i, l);
                }
            }
        }
    }
}

/// What a `RowData` shows, so it can be recomputed from a new state.
#[derive(Clone)]
pub struct RowSpec {
    pub entity: EntityRef,
    /// Rows in read-only lists (placeholders, attribute rows) have no control.
    pub interactive: bool,
    pub action_name: Option<String>,
    pub attribute: Option<String>,
    /// Badge rows: which parts are hidden (`show_name`, `show_state`, `show_icon` = false).
    pub hide: Option<(bool, bool, bool)>,
}

#[derive(Clone)]
pub enum RowRef {
    Row {
        model: Rc<VecModel<RowData>>,
        index: usize,
        spec: RowSpec,
    },
    Feature {
        model: Rc<VecModel<FeatureData>>,
        index: usize,
        entity: EntityRef,
        feature: Feature,
    },
    /// This entity participates in a card's header toggle.
    Header { card_id: usize },
}

pub struct CardCtx {
    pub slot: Option<CardSlot>,
    /// Entities behind the header toggle.
    pub header_ids: Vec<Arc<str>>,
    /// Synthetic row keys → configured actions (button rows / button cards).
    pub row_actions: HashMap<String, (Option<Arc<str>>, Option<ActionSpec>)>,
}

pub struct ViewBuild {
    pub data: ViewData,
    pub rows_model: Rc<VecModel<SectionRow>>,
    /// Masonry only: each card wrapped in its own single-element model with its height estimate.
    pub masonry: Option<Vec<(Rc<VecModel<CardData>>, u32)>>,
    /// Sections only: the sections with their configured column span, and the configured maximum.
    pub sections: Option<Vec<(ColumnData, u32)>>,
    pub max_columns: u32,
    /// Columns currently in use (masonry columns, or section grid columns).
    pub columns: usize,
    /// Sections only: the layout this view was built for (see `sections_key`).
    pub sections_key: Option<(usize, Vec<u32>)>,
}

pub struct Built {
    pub views: Vec<ViewBuild>,
    pub cards: Vec<CardCtx>,
    pub index: HashMap<Arc<str>, Vec<RowRef>>,
    /// Calendar entity → calendar cards showing it (rows model, card slot for the subtitle, days).
    pub calendars: HashMap<Arc<str>, Vec<CalendarRef>>,
}

#[derive(Clone)]
pub struct CalendarRef {
    pub rows: Rc<VecModel<RowData>>,
    pub slot: Option<CardSlot>,
    pub days: u32,
    /// Set when the card has no title: it shows this entity's name instead,
    /// recomputed once states are known.
    pub title_from: Option<EntityRef>,
}

pub struct Ctx<'a> {
    pub store: &'a StateStore,
    pub icons: &'a IconResolver,
    pub translations: Option<&'a Translations>,
    pub locale: &'static Locale,
}

struct Builder<'a> {
    ctx: Ctx<'a>,
    cards: Vec<CardCtx>,
    index: HashMap<Arc<str>, Vec<RowRef>>,
    calendars: HashMap<Arc<str>, Vec<CalendarRef>>,
}

pub fn build(dash: &Dashboard, ctx: Ctx<'_>, content_width: f32) -> Built {
    let mut b = Builder {
        ctx,
        cards: Vec::new(),
        index: HashMap::new(),
        calendars: HashMap::new(),
    };
    let mut views = Vec::with_capacity(dash.views.len());
    for v in &dash.views {
        views.push(b.view(v, content_width));
    }
    Built {
        views,
        cards: b.cards,
        index: b.index,
        calendars: b.calendars,
    }
}

/// Rows of a calendar card: a day header (kind 6: weekday / full date) followed
/// by its events (kind 7). `now` is Unix seconds; the day labels use the UTC
/// offset found in the events. Also returns the range subtitle ("9 – 15 nov 2026").
pub fn calendar_rows(
    events: &[hada_core::ha::ws::CalendarEvent],
    now: f64,
    days: u32,
    loc: &Locale,
) -> (Vec<RowData>, String) {
    use hada_core::timefmt::{parse_offset, WallClock};
    let offset = events
        .iter()
        .find_map(|e| parse_offset(&e.start))
        .unwrap_or(0);
    let today = WallClock::from_unix(now, offset);
    let tomorrow = WallClock::from_unix(now + 86_400.0, offset);
    let last = WallClock::from_unix(now + 86_400.0 * f64::from(days.max(1) - 1), offset);
    let subtitle = if days > 1 {
        loc.range_label(&today, &last)
    } else {
        loc.long_date(&today)
    };
    let mut rows = Vec::new();
    if events.is_empty() {
        rows.push(RowData {
            kind: 6,
            name: loc.no_events.into(),
            ..RowData::default()
        });
        return (rows, subtitle);
    }
    let mut last_day: Option<WallClock> = None;
    for e in events {
        let Some(start) = WallClock::parse(&e.start) else {
            continue;
        };
        let end = WallClock::parse(&e.end);
        if !last_day.is_some_and(|d| d.same_day(&start)) {
            let weekday = if start.same_day(&today) {
                loc.today.to_string()
            } else if start.same_day(&tomorrow) {
                loc.tomorrow.to_string()
            } else {
                loc.weekday(&start).to_string()
            };
            rows.push(RowData {
                kind: 6,
                name: weekday.into(),
                state_text: loc.long_date(&start).into(),
                active: start.same_day(&today),
                ..RowData::default()
            });
            last_day = Some(start);
        }
        let time = if !start.has_time {
            loc.all_day.to_string()
        } else {
            match end {
                Some(en) if en.has_time && en.same_day(&start) => {
                    format!("{} - {}", start.time_label(), en.time_label())
                }
                _ => start.time_label(),
            }
        };
        rows.push(RowData {
            kind: 7,
            name: e.summary.as_str().into(),
            state_text: time.into(),
            secondary: e.location.as_deref().unwrap_or("").into(),
            active: start.same_day(&today),
            ..RowData::default()
        });
    }
    (rows, subtitle)
}

pub fn masonry_columns(content_width: f32) -> usize {
    ((content_width - 24.0) / MASONRY_COLUMN_WIDTH)
        .floor()
        .clamp(1.0, MASONRY_MAX_COLUMNS as f32) as usize
}

/// Columns of a sections view for the available width (collapses on narrow screens).
pub fn section_columns(content_width: f32, max_columns: u32) -> usize {
    ((content_width - 24.0) / SECTION_MIN_WIDTH)
        .floor()
        .clamp(1.0, max_columns.max(1) as f32) as usize
}

/// Cards with a default half width (tiles, sensors, …) per row in a section this wide.
pub fn tiles_per_row(section_width: f32) -> u32 {
    (section_width / TILE_TARGET_WIDTH).floor().clamp(2.0, 4.0) as u32
}

/// What a sections view's layout depends on: column count and cards per row of each section.
pub fn sections_key(spans: &[u32], max_columns: u32, content_width: f32) -> (usize, Vec<u32>) {
    let cols = section_columns(content_width, max_columns);
    let unit = (content_width - 24.0) / cols as f32;
    let per_row = spans
        .iter()
        .map(|span| tiles_per_row(unit * (*span).min(cols as u32) as f32))
        .collect();
    (cols, per_row)
}

/// Group sections into rows of at most `cols` column units.
pub fn section_rows(sections: &[(ColumnData, u32)], cols: usize) -> Vec<SectionRow> {
    let cols = cols.max(1) as u32;
    let mut rows: Vec<SectionRow> = Vec::new();
    let mut current: Vec<ColumnData> = Vec::new();
    let mut used = 0u32;
    for (section, span) in sections {
        let span = (*span).min(cols);
        if used + span > cols && !current.is_empty() {
            rows.push(SectionRow {
                sections: ModelRc::new(VecModel::from(std::mem::take(&mut current))),
                fill: (cols - used) as i32,
            });
            used = 0;
        }
        let mut s = section.clone();
        s.span = span as i32;
        current.push(s);
        used += span;
    }
    if !current.is_empty() {
        rows.push(SectionRow {
            sections: ModelRc::new(VecModel::from(current)),
            fill: (cols - used) as i32,
        });
    }
    rows
}

/// Distribute masonry cards over `n` columns (shortest column first).
pub fn masonry_rows(cards: &[(Rc<VecModel<CardData>>, u32)], n: usize) -> Vec<SectionRow> {
    let n = n.max(1);
    let mut heights = vec![0u32; n];
    let mut columns: Vec<Vec<CardRow>> = vec![Vec::new(); n];
    for (model, est) in cards {
        let (col, _) = heights
            .iter()
            .enumerate()
            .min_by_key(|(i, h)| (**h, *i))
            .unwrap();
        heights[col] += *est + 1;
        columns[col].push(CardRow {
            cards: ModelRc::from(model.clone()),
            fill: 0,
        });
    }
    let sections: Vec<ColumnData> = columns
        .into_iter()
        .map(|rows| ColumnData {
            title: SharedString::default(),
            span: 1,
            rows: ModelRc::new(VecModel::from(rows)),
        })
        .collect();
    vec![SectionRow {
        sections: ModelRc::new(VecModel::from(sections)),
        fill: 0,
    }]
}

impl Builder<'_> {
    fn view(&mut self, v: &View, content_width: f32) -> ViewBuild {
        let badges_model: Rc<VecModel<RowData>> = Rc::new(VecModel::from(Vec::<RowData>::new()));
        for b in &v.badges {
            let spec = RowSpec {
                entity: b.entity.clone(),
                interactive: false,
                action_name: None,
                attribute: None,
                hide: Some((!b.show_name, !b.show_state, !b.show_icon)),
            };
            let mut row = RowData::default();
            fill_row(
                &mut row,
                &spec,
                self.ctx.store.get(&b.entity.entity_id),
                self.ctx.icons,
                self.ctx.translations,
            );
            let index = badges_model.row_count();
            badges_model.push(row);
            // Registered like any other row so state changes reach the badge.
            self.index
                .entry(b.entity.entity_id.clone())
                .or_default()
                .push(RowRef::Row {
                    model: badges_model.clone(),
                    index,
                    spec,
                });
        }
        let mut sections_key_out: Option<(usize, Vec<u32>)> = None;
        let (rows, masonry, sections_built, columns, is_sections, max_columns) = match &v.layout {
            Layout::Masonry(cards) => {
                let wrapped: Vec<(Rc<VecModel<CardData>>, u32)> = cards
                    .iter()
                    .map(|c| {
                        let est = c.estimated_rows();
                        let model = Rc::new(VecModel::from(vec![CardData::default()]));
                        let slot = CardSlot::Card(model.clone(), 0);
                        let data = self.card(c, Some(slot), false);
                        model.set_row_data(0, data);
                        (model, est)
                    })
                    .collect();
                let n = masonry_columns(content_width);
                (
                    masonry_rows(&wrapped, n),
                    Some(wrapped),
                    None,
                    n,
                    false,
                    n as u32,
                )
            }
            Layout::Sections {
                max_columns,
                sections,
            } => {
                // An empty section (no cards, no title) would only hold empty space.
                let kept: Vec<&Section> = sections
                    .iter()
                    .filter(|s| !s.cards.is_empty() || s.title.is_some())
                    .collect();
                let spans: Vec<u32> = kept
                    .iter()
                    .map(|s| s.column_span.clamp(1, *max_columns))
                    .collect();
                let key = sections_key(&spans, *max_columns, content_width);
                let built: Vec<(ColumnData, u32)> = kept
                    .iter()
                    .zip(&spans)
                    .zip(&key.1)
                    .map(|((s, span), per_row)| (self.section(s, *span, *per_row), *span))
                    .collect();
                sections_key_out = Some(key);
                let cols = section_columns(content_width, *max_columns);
                (
                    section_rows(&built, cols),
                    None,
                    Some(built),
                    cols,
                    true,
                    *max_columns,
                )
            }
        };
        let rows_model = Rc::new(VecModel::from(rows));
        let data = ViewData {
            title: v.title.as_str().into(),
            glyph: glyph_str(v.icon.as_deref().unwrap_or("mdi:view-dashboard")),
            path: v.path.as_str().into(),
            is_sections,
            max_columns: columns as i32,
            badges: ModelRc::from(badges_model),
            rows: ModelRc::from(rows_model.clone()),
        };
        ViewBuild {
            data,
            rows_model,
            masonry,
            sections: sections_built,
            max_columns,
            columns,
            sections_key: sections_key_out,
        }
    }

    fn section(&mut self, s: &Section, span: u32, per_row: u32) -> ColumnData {
        let mut rows: Vec<CardRow> = Vec::new();
        let mut current_cards: Vec<CardNode> = Vec::new();
        let mut used = 0u32;
        let flush =
            |b: &mut Self, cards: &mut Vec<CardNode>, used: u32, rows: &mut Vec<CardRow>| {
                if cards.is_empty() {
                    return;
                }
                let model = Rc::new(VecModel::from(vec![CardData::default(); cards.len()]));
                for (i, c) in cards.drain(..).enumerate() {
                    let data = b.card(&c, Some(CardSlot::Card(model.clone(), i)), true);
                    model.set_row_data(i, data);
                }
                rows.push(CardRow {
                    cards: ModelRc::from(model),
                    fill: (12 - used.min(12)) as i32,
                });
            };
        for c in &s.cards {
            let mut c = c.clone();
            // Half-width default cards (tiles…) follow the density that fits the section.
            if c.grid.auto_columns && c.grid.columns == 6 {
                c.grid.columns = 12 / per_row.clamp(1, 12);
            }
            let cols = c.grid.columns.clamp(1, 12);
            if used + cols > 12 && !current_cards.is_empty() {
                flush(self, &mut current_cards, used, &mut rows);
                used = 0;
            }
            current_cards.push(c);
            used += cols;
        }
        flush(self, &mut current_cards, used, &mut rows);
        ColumnData {
            title: s.title.as_deref().unwrap_or("").into(),
            span: span as i32,
            rows: ModelRc::new(VecModel::from(rows)),
        }
    }

    fn card(&mut self, c: &CardNode, slot: Option<CardSlot>, in_sections: bool) -> CardData {
        match &c.kind {
            CardKind::Grid { columns, cards, .. } => {
                let card_id = self.new_card(slot.clone());
                let children_model =
                    Rc::new(VecModel::from(vec![LeafCard::default(); cards.len()]));
                for (i, child) in cards.iter().enumerate() {
                    let mut leaf = match &child.kind {
                        // Nested grids cannot be represented; show a placeholder.
                        CardKind::Grid { .. } => self.placeholder_leaf(
                            child,
                            Some(CardSlot::Leaf(children_model.clone(), i)),
                        ),
                        _ => self.leaf(
                            child,
                            Some(CardSlot::Leaf(children_model.clone(), i)),
                            in_sections,
                        ),
                    };
                    leaf.grid_row = (i / (*columns as usize).max(1)) as i32;
                    leaf.grid_col = (i % (*columns as usize).max(1)) as i32;
                    children_model.set_row_data(i, leaf);
                }
                let mut leaf = LeafCard {
                    kind: 5,
                    raw_type: "grid".into(),
                    title: c.title.as_deref().unwrap_or("").into(),
                    card_id: card_id as i32,
                    header_toggle: -1,
                    rows: ModelRc::new(VecModel::from(Vec::<RowData>::new())),
                    features: ModelRc::new(VecModel::from(Vec::<FeatureData>::new())),
                    ..LeafCard::default()
                };
                apply_grid(&mut leaf, c, in_sections);
                CardData {
                    leaf,
                    is_grid: true,
                    grid_columns: *columns as i32,
                    children: ModelRc::from(children_model),
                }
            }
            _ => {
                let leaf = self.leaf(c, slot, in_sections);
                CardData {
                    leaf,
                    is_grid: false,
                    grid_columns: 0,
                    children: ModelRc::new(VecModel::from(Vec::<LeafCard>::new())),
                }
            }
        }
    }

    fn new_card(&mut self, slot: Option<CardSlot>) -> usize {
        self.cards.push(CardCtx {
            slot,
            header_ids: Vec::new(),
            row_actions: HashMap::new(),
        });
        self.cards.len() - 1
    }

    fn placeholder_leaf(&mut self, c: &CardNode, slot: Option<CardSlot>) -> LeafCard {
        let card_id = self.new_card(slot);
        let mut leaf = LeafCard {
            kind: 8,
            raw_type: c.raw_type.as_str().into(),
            title: c.title.as_deref().unwrap_or("").into(),
            card_id: card_id as i32,
            header_toggle: -1,
            rows: ModelRc::new(VecModel::from(Vec::<RowData>::new())),
            features: ModelRc::new(VecModel::from(Vec::<FeatureData>::new())),
            ..LeafCard::default()
        };
        apply_grid(&mut leaf, c, false);
        leaf
    }

    fn leaf(&mut self, c: &CardNode, slot: Option<CardSlot>, in_sections: bool) -> LeafCard {
        let card_id = self.new_card(slot.clone());
        let rows_model: Rc<VecModel<RowData>> = Rc::new(VecModel::from(Vec::<RowData>::new()));
        let features_model: Rc<VecModel<FeatureData>> =
            Rc::new(VecModel::from(Vec::<FeatureData>::new()));
        let mut leaf = LeafCard {
            raw_type: c.raw_type.as_str().into(),
            title: c.title.as_deref().unwrap_or("").into(),
            card_id: card_id as i32,
            header_toggle: -1,
            rows: ModelRc::from(rows_model.clone()),
            features: ModelRc::from(features_model.clone()),
            show_name: true,
            show_icon: true,
            show_state: true,
            ..LeafCard::default()
        };
        apply_grid(&mut leaf, c, in_sections);

        match &c.kind {
            CardKind::Tile {
                entity,
                features,
                hide_state,
                tap_toggles,
                ..
            } => {
                leaf.kind = 0;
                leaf.hide_state = *hide_state;
                leaf.tap_toggles = *tap_toggles;
                self.push_row(&rows_model, card_id, entity, false, None, 0);
                for f in features {
                    if matches!(f, Feature::Unknown(name) if { tracing::info!("ignoring unsupported tile feature {name}"); true })
                    {
                        continue;
                    }
                    let idx = features_model.row_count();
                    let data = self.feature_data(entity, f);
                    features_model.push(data);
                    self.index
                        .entry(entity.entity_id.clone())
                        .or_default()
                        .push(RowRef::Feature {
                            model: features_model.clone(),
                            index: idx,
                            entity: entity.clone(),
                            feature: f.clone(),
                        });
                }
            }
            CardKind::Entities {
                rows,
                header_toggle,
                state_color,
            } => {
                leaf.kind = 1;
                leaf.state_color = *state_color;
                for r in rows {
                    self.push_entities_row(&rows_model, card_id, r);
                }
                if *header_toggle {
                    let ids: Vec<Arc<str>> = rows
                        .iter()
                        .filter_map(|r| match r {
                            Row::Entity { entity, .. }
                                if domain::is_toggleable(entity.domain()) =>
                            {
                                Some(entity.entity_id.clone())
                            }
                            _ => None,
                        })
                        .collect();
                    for id in &ids {
                        self.index
                            .entry(id.clone())
                            .or_default()
                            .push(RowRef::Header { card_id });
                    }
                    leaf.header_toggle = i32::from(header_state(&ids, self.ctx.store));
                    self.cards[card_id].header_ids = ids;
                }
            }
            CardKind::Glance {
                items,
                show_name,
                show_icon,
                show_state,
                columns,
            } => {
                leaf.kind = 2;
                leaf.show_name = *show_name;
                leaf.show_icon = *show_icon;
                leaf.show_state = *show_state;
                let cols = columns
                    .map(|c| c as usize)
                    .unwrap_or_else(|| items.len().clamp(1, 5))
                    .max(1);
                for (i, e) in items.iter().enumerate() {
                    let mut row = self.entity_row(e, true, None, 0);
                    row.grid_row = (i / cols) as i32;
                    row.grid_col = (i % cols) as i32;
                    let idx = rows_model.row_count();
                    rows_model.push(row);
                    self.index
                        .entry(e.entity_id.clone())
                        .or_default()
                        .push(RowRef::Row {
                            model: rows_model.clone(),
                            index: idx,
                            spec: RowSpec {
                                entity: e.clone(),
                                interactive: true,
                                action_name: None,
                                attribute: None,
                                hide: None,
                            },
                        });
                }
            }
            CardKind::Sensor { entity, unit } => {
                leaf.kind = 3;
                leaf.unit = unit.as_deref().unwrap_or("").into();
                self.push_row(&rows_model, card_id, entity, false, None, 0);
            }
            CardKind::Gauge {
                entity,
                min,
                max,
                unit,
                severity,
                ..
            } => {
                leaf.kind = 4;
                leaf.min = *min as f32;
                leaf.max = *max as f32;
                leaf.unit = unit.as_deref().unwrap_or("").into();
                if let Some(s) = severity {
                    leaf.has_severity = true;
                    leaf.sev_green = s.green as f32;
                    leaf.sev_yellow = s.yellow as f32;
                    leaf.sev_red = s.red as f32;
                }
                self.push_row(&rows_model, card_id, entity, false, None, 0);
            }
            CardKind::Markdown { lines } => {
                leaf.kind = 6;
                for l in lines {
                    rows_model.push(RowData {
                        kind: 4,
                        name: l.text.as_str().into(),
                        state_text: l.url.as_deref().unwrap_or("").into(),
                        control: ControlKind::None,
                        ..RowData::default()
                    });
                }
            }
            CardKind::Button {
                entity,
                name,
                icon,
                action,
            } => {
                leaf.kind = 7;
                let key = format!("card:{card_id}");
                self.cards[card_id].row_actions.insert(
                    key.clone(),
                    (entity.as_ref().map(|e| e.entity_id.clone()), action.clone()),
                );
                if let Some(e) = entity {
                    let mut row = self.entity_row(e, false, None, 0);
                    row.entity_id = key.into();
                    if let Some(n) = name {
                        row.name = n.as_str().into();
                    }
                    if let Some(i) = icon {
                        row.glyph = glyph_str(i);
                    }
                    rows_model.push(row);
                    self.index
                        .entry(e.entity_id.clone())
                        .or_default()
                        .push(RowRef::Row {
                            model: rows_model.clone(),
                            index: 0,
                            spec: RowSpec {
                                entity: e.clone(),
                                interactive: false,
                                action_name: None,
                                attribute: None,
                                hide: None,
                            },
                        });
                } else {
                    rows_model.push(RowData {
                        entity_id: key.into(),
                        name: name.as_deref().unwrap_or("").into(),
                        glyph: glyph_str(icon.as_deref().unwrap_or("mdi:gesture-tap-button")),
                        control: ControlKind::None,
                        ..RowData::default()
                    });
                }
            }
            CardKind::Calendar { entities, days } => {
                leaf.kind = 9;
                let title_from = if leaf.title.is_empty() {
                    entities.first().cloned()
                } else {
                    None
                };
                if leaf.title.is_empty() {
                    let name = entities
                        .first()
                        .map(|e| display_name(e, self.ctx.store.get(&e.entity_id)))
                        .unwrap_or_else(|| "Calendar".into());
                    leaf.title = name.into();
                }
                rows_model.push(RowData {
                    kind: 6,
                    name: self.ctx.locale.loading.into(),
                    ..RowData::default()
                });
                for e in entities {
                    self.calendars
                        .entry(e.entity_id.clone())
                        .or_default()
                        .push(CalendarRef {
                            rows: rows_model.clone(),
                            slot: slot.clone(),
                            days: *days,
                            title_from: title_from.clone(),
                        });
                }
            }
            CardKind::Unsupported { entities } => {
                leaf.kind = 8;
                for e in entities {
                    self.push_row(&rows_model, card_id, e, false, None, 5);
                }
            }
            CardKind::Grid { .. } => unreachable!("grids are handled in card()"),
        }
        leaf
    }

    fn push_entities_row(&mut self, rows_model: &Rc<VecModel<RowData>>, card_id: usize, r: &Row) {
        match r {
            Row::Entity {
                entity,
                action_name,
                secondary_info,
                tap_action,
            } => {
                let idx = rows_model.row_count();
                let mut row = self.entity_row(entity, true, action_name.as_deref(), 0);
                if let Some(s) = secondary_info {
                    row.secondary = secondary_text(s, self.ctx.store.get(&entity.entity_id)).into();
                }
                if let Some(a) = tap_action {
                    self.cards[card_id].row_actions.insert(
                        entity.entity_id.to_string(),
                        (Some(entity.entity_id.clone()), Some(a.clone())),
                    );
                }
                rows_model.push(row);
                self.index
                    .entry(entity.entity_id.clone())
                    .or_default()
                    .push(RowRef::Row {
                        model: rows_model.clone(),
                        index: idx,
                        spec: RowSpec {
                            entity: entity.clone(),
                            interactive: true,
                            action_name: action_name.clone(),
                            attribute: None,
                            hide: None,
                        },
                    });
            }
            Row::Section { label } => rows_model.push(RowData {
                kind: 1,
                name: label.as_deref().unwrap_or("").into(),
                ..RowData::default()
            }),
            Row::Divider => rows_model.push(RowData {
                kind: 2,
                ..RowData::default()
            }),
            Row::Button {
                name,
                icon,
                action_name,
                action,
            } => {
                let key = format!("row:{card_id}:{}", rows_model.row_count());
                self.cards[card_id]
                    .row_actions
                    .insert(key.clone(), (None, action.clone()));
                rows_model.push(RowData {
                    kind: 3,
                    entity_id: key.into(),
                    name: name.as_deref().unwrap_or("").into(),
                    glyph: icon.as_deref().map(glyph_str).unwrap_or_default(),
                    action_name: action_name.as_deref().unwrap_or("Run").into(),
                    ..RowData::default()
                });
            }
            Row::Attribute {
                entity,
                attribute,
                prefix,
                suffix,
            } => {
                let idx = rows_model.row_count();
                let spec = RowSpec {
                    entity: entity.clone(),
                    interactive: false,
                    action_name: None,
                    attribute: Some(format!(
                        "{}\u{1}{}\u{1}{}",
                        prefix.as_deref().unwrap_or(""),
                        attribute,
                        suffix.as_deref().unwrap_or("")
                    )),
                    hide: None,
                };
                let mut row = RowData {
                    kind: 5,
                    ..RowData::default()
                };
                fill_row(
                    &mut row,
                    &spec,
                    self.ctx.store.get(&entity.entity_id),
                    self.ctx.icons,
                    self.ctx.translations,
                );
                rows_model.push(row);
                self.index
                    .entry(entity.entity_id.clone())
                    .or_default()
                    .push(RowRef::Row {
                        model: rows_model.clone(),
                        index: idx,
                        spec,
                    });
            }
            Row::Weblink { name, url } => rows_model.push(RowData {
                kind: 4,
                name: name.as_deref().unwrap_or(url).into(),
                state_text: url.as_str().into(),
                ..RowData::default()
            }),
            Row::Buttons(list) => {
                for e in list {
                    self.push_row(rows_model, card_id, e, true, None, 0);
                }
            }
        }
    }

    fn push_row(
        &mut self,
        rows_model: &Rc<VecModel<RowData>>,
        _card_id: usize,
        entity: &EntityRef,
        interactive: bool,
        action_name: Option<&str>,
        kind: i32,
    ) {
        let idx = rows_model.row_count();
        let row = self.entity_row(entity, interactive, action_name, kind);
        rows_model.push(row);
        self.index
            .entry(entity.entity_id.clone())
            .or_default()
            .push(RowRef::Row {
                model: rows_model.clone(),
                index: idx,
                spec: RowSpec {
                    entity: entity.clone(),
                    interactive,
                    action_name: action_name.map(str::to_string),
                    attribute: None,
                    hide: None,
                },
            });
    }

    fn entity_row(
        &self,
        entity: &EntityRef,
        interactive: bool,
        action_name: Option<&str>,
        kind: i32,
    ) -> RowData {
        let spec = RowSpec {
            entity: entity.clone(),
            interactive,
            action_name: action_name.map(str::to_string),
            attribute: None,
            hide: None,
        };
        let mut row = RowData {
            kind,
            ..RowData::default()
        };
        fill_row(
            &mut row,
            &spec,
            self.ctx.store.get(&entity.entity_id),
            self.ctx.icons,
            self.ctx.translations,
        );
        row
    }

    fn feature_data(&self, entity: &EntityRef, feature: &Feature) -> FeatureData {
        let mut f = FeatureData::default();
        fill_feature(
            &mut f,
            entity,
            feature,
            self.ctx.store.get(&entity.entity_id),
        );
        f
    }
}

fn apply_grid(leaf: &mut LeafCard, c: &CardNode, in_sections: bool) {
    // grid_options only apply inside sections; masonry cards always fill their column.
    leaf.span = if in_sections {
        c.grid.columns.clamp(1, 12) as i32
    } else {
        12
    };
    leaf.fixed_rows = if in_sections {
        c.grid.rows.unwrap_or(0) as i32
    } else {
        0
    };
}

pub fn glyph_str(icon: &str) -> SharedString {
    let mut s = SharedString::new();
    s.push_str(mdi::glyph(icon).encode_utf8(&mut [0u8; 4]));
    s
}

pub fn display_name(entity: &EntityRef, st: Option<&EntityState>) -> String {
    if let Some(n) = &entity.name {
        return n.clone();
    }
    if let Some(n) = st.and_then(|s| s.friendly_name()) {
        return n.to_string();
    }
    let tail = entity
        .entity_id
        .split_once('.')
        .map(|(_, t)| t)
        .unwrap_or(&entity.entity_id);
    tail.replace('_', " ")
}

fn secondary_text(kind: &str, st: Option<&EntityState>) -> String {
    let Some(st) = st else { return String::new() };
    let now = hada_core::timefmt::now_unix();
    match kind {
        "last-changed" | "last_changed" => hada_core::timefmt::relative(st.last_changed, now),
        "last-updated" | "last_updated" => hada_core::timefmt::relative(st.last_updated, now),
        "entity-id" | "entity_id" => String::new(),
        other => st.attr_str(other).map(str::to_string).unwrap_or_default(),
    }
}

pub fn header_state(ids: &[Arc<str>], store: &StateStore) -> bool {
    ids.iter().any(|id| {
        store
            .get(id)
            .is_some_and(|s| domain::is_active(domain::domain_of(id), &s.state))
    })
}

fn slider_kind(k: CoreSliderKind) -> SliderKind {
    match k {
        CoreSliderKind::BrightnessPct => SliderKind::BrightnessPct,
        CoreSliderKind::FanPercentage => SliderKind::FanPercentage,
        CoreSliderKind::CoverPosition => SliderKind::CoverPosition,
        CoreSliderKind::CoverTilt => SliderKind::CoverTilt,
        CoreSliderKind::Number => SliderKind::Number,
        CoreSliderKind::Temperature => SliderKind::Temperature,
    }
}

fn control_kind(c: &Control) -> ControlKind {
    match c {
        Control::None => ControlKind::None,
        Control::Toggle => ControlKind::Toggle,
        Control::Slider(_) => ControlKind::Slider,
        Control::OpenCloseStop => ControlKind::OpenCloseStop,
        Control::Press => ControlKind::Press,
        Control::Run => ControlKind::Run,
        Control::Install => ControlKind::Install,
    }
}

/// Recompute everything state-dependent in a row.
pub fn fill_row(
    row: &mut RowData,
    spec: &RowSpec,
    st: Option<&EntityState>,
    icons: &IconResolver,
    tr: Option<&Translations>,
) {
    let id = &*spec.entity.entity_id;
    let domain = domain::domain_of(id);
    row.entity_id = id.into();
    row.name = display_name(&spec.entity, st).into();
    row.glyph = glyph_str(&icons.resolve(id, st, spec.entity.icon.as_deref()));
    row.action_name = spec.action_name.as_deref().unwrap_or("").into();
    match st {
        Some(s) => {
            row.unavailable = s.is_unavailable();
            row.active = domain::is_active(domain, &s.state);
            row.state_text = if let Some(attr) = &spec.attribute {
                let mut parts = attr.split('\u{1}');
                let prefix = parts.next().unwrap_or("");
                let name = parts.next().unwrap_or("");
                let suffix = parts.next().unwrap_or("");
                let v = s.attributes.get(name).map(|v| match v {
                    serde_json::Value::String(x) => x.clone(),
                    other => other.to_string(),
                });
                format!("{prefix}{}{suffix}", v.unwrap_or_else(|| "—".into())).into()
            } else {
                domain::format_state_tr(id, s, hada_core::timefmt::now_unix(), tr).into()
            };
            row.value = s.numeric_state().unwrap_or(0.0) as f32;
        }
        None => {
            row.unavailable = true;
            row.active = false;
            row.state_text = "—".into();
            row.value = 0.0;
        }
    }
    row.pending = false;
    if let Some((hide_name, hide_state, hide_icon)) = spec.hide {
        if hide_name {
            row.name = SharedString::default();
        }
        if hide_state {
            row.state_text = SharedString::default();
        }
        if hide_icon {
            row.glyph = SharedString::default();
        }
    }
    let control = if spec.interactive && spec.attribute.is_none() {
        domain::primary_control(id, st)
    } else {
        Control::None
    };
    row.control = control_kind(&control);
    if let Control::Slider(sl) = &control {
        row.slider_kind = slider_kind(sl.kind);
        row.value = sl.value as f32;
        row.min = sl.min as f32;
        row.max = sl.max as f32;
        row.step = sl.step as f32;
        row.unit = sl.unit.into();
    }
}

pub fn fill_feature(
    f: &mut FeatureData,
    entity: &EntityRef,
    feature: &Feature,
    st: Option<&EntityState>,
) {
    let id = &*entity.entity_id;
    f.entity_id = id.into();
    f.available = st.is_some_and(|s| !s.is_unavailable());
    f.on = st.is_some_and(|s| domain::is_active(domain::domain_of(id), &s.state));
    f.pending = false;
    let control = domain::feature_control(id, feature, st);
    f.kind = control_kind(&control);
    if let Control::Slider(sl) = &control {
        f.slider_kind = slider_kind(sl.kind);
        f.value = sl.value as f32;
        f.min = sl.min as f32;
        f.max = sl.max as f32;
        f.step = sl.step as f32;
        f.unit = sl.unit.into();
    }
    if let Feature::Button { action_name } = feature {
        f.label = action_name.as_deref().unwrap_or("").into();
    }
}
