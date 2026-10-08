//! Sunburst chart: layout of the tree into rings of sectors, painting and hit-testing.
//!
//! Angles are in radians, `0` at 12 o'clock, growing clockwise.

use std::collections::HashMap;
use std::f32::consts::TAU;

use eframe::egui::{
    self, Align2, Color32, FontId, Mesh, Painter, Pos2, Rect, Response, Sense, Shape, Stroke, Ui, Vec2, pos2,
};

use crate::colors;
use crate::format;
use crate::tree::{NodeId, Tree};

/// Rings shown at most around the center.
pub const MAX_RINGS: usize = 8;
/// Sectors narrower than this are merged into one "smaller objects" sector.
const MIN_ANGLE: f64 = 0.004;
/// Size of the center hole relative to the chart radius.
const HOLE: f32 = 0.2;
/// Each ring is this much thinner than the previous one.
const RING_DECAY: f32 = 0.9;
/// Arc tessellation step.
const ARC_STEP: f32 = 1.5 * TAU / 360.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Item {
    Node(NodeId),
    /// Several items of `parent` that are too small to draw on their own.
    Small {
        parent: NodeId,
        count: u32,
        size: u64,
    },
}

impl Item {
    /// The node whose content this sector stands for.
    pub fn node(self) -> NodeId {
        match self {
            Item::Node(n) => n,
            Item::Small { parent, .. } => parent,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Sector {
    pub item: Item,
    pub a0: f32,
    pub a1: f32,
    pub color: Color32,
}

pub struct Layout {
    pub root: NodeId,
    /// Size of the root: sector colors encode each item's share of it.
    total: u64,
    /// Sectors per ring, ordered by angle.
    pub rings: Vec<Vec<Sector>>,
    index: HashMap<NodeId, (usize, usize)>,
}

impl Layout {
    pub fn compute(tree: &Tree, root: NodeId) -> Self {
        let total = tree.node(root).size;
        let mut layout = Self { root, total, rings: Vec::new(), index: HashMap::new() };
        layout.place(tree, root, 0, 0.0, TAU as f64);
        layout
    }

    fn place(&mut self, tree: &Tree, node: NodeId, ring: usize, a0: f64, a1: f64) {
        let total = tree.node(node).size;
        if ring >= MAX_RINGS || total == 0 {
            return;
        }
        let scale = (a1 - a0) / total as f64;
        let mut a = a0;
        let (mut small_count, mut small_size) = (0u32, 0u64);
        for child in tree.sorted_children(node) {
            let n = tree.node(child);
            let da = n.size as f64 * scale;
            if da < MIN_ANGLE {
                if n.size > 0 {
                    small_count += 1;
                    small_size += n.size;
                }
                continue;
            }
            self.push(ring, Item::Node(child), a, a + da, n.size, n.is_dir());
            if n.is_dir() {
                self.place(tree, child, ring + 1, a, a + da);
            }
            a += da;
        }
        if small_count > 0 {
            let da = small_size as f64 * scale;
            let item = Item::Small { parent: node, count: small_count, size: small_size };
            self.push(ring, item, a, (a + da.max(MIN_ANGLE / 2.0)).min(a1), small_size, false);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn push(&mut self, ring: usize, item: Item, a0: f64, a1: f64, size: u64, is_dir: bool) {
        if self.rings.len() <= ring {
            self.rings.resize_with(ring + 1, Vec::new);
        }
        let (a0, a1) = (a0 as f32, a1 as f32);
        let color = match item {
            Item::Node(_) => colors::sector(size as f32 / self.total.max(1) as f32, ring as f32, is_dir),
            Item::Small { .. } => colors::SMALL,
        };
        if let Item::Node(n) = item {
            self.index.insert(n, (ring, self.rings[ring].len()));
        }
        self.rings[ring].push(Sector { item, a0, a1, color });
    }

    pub fn sector_of(&self, node: NodeId) -> Option<&Sector> {
        self.locate(node).map(|(_, s)| s)
    }

    fn locate(&self, node: NodeId) -> Option<(usize, &Sector)> {
        self.index.get(&node).map(|&(r, i)| (r, &self.rings[r][i]))
    }

    fn find(&self, ring: usize, angle: f32) -> Option<&Sector> {
        let sectors = self.rings.get(ring)?;
        let i = sectors.partition_point(|s| s.a1 <= angle);
        sectors.get(i).filter(|s| s.a0 <= angle)
    }
}

/// Screen-space ring geometry for a given rect.
#[derive(Clone, Copy)]
struct Geometry {
    center: Pos2,
    hole: f32,
    /// Width of the innermost ring; each next one is `RING_DECAY` times thinner.
    base: f32,
    rings: usize,
}

impl Geometry {
    fn new(rect: Rect, rings: usize) -> Self {
        let center = rect.center();
        let radius = (rect.width().min(rect.height()) / 2.0 - 12.0).max(40.0);
        let hole = radius * HOLE;
        // Fewer rings than the maximum get proportionally wider, but never fewer than 4 slots.
        let slots = rings.clamp(4, MAX_RINGS);
        let base = (radius - hole) * (1.0 - RING_DECAY) / (1.0 - RING_DECAY.powi(slots as i32));
        Self { center, hole, base, rings }
    }

    /// Radius of the inner edge of ring `x`. Fractional rings are used while animating,
    /// negative ones collapse into the center hole.
    fn boundary(&self, x: f32) -> f32 {
        if x >= 0.0 {
            self.hole + self.base * (1.0 - RING_DECAY.powf(x)) / (1.0 - RING_DECAY)
        } else {
            (self.hole + self.base * x).max(0.0)
        }
    }

    fn radii(&self, ring: f32) -> (f32, f32) {
        (self.boundary(ring), self.boundary(ring + 1.0))
    }

    fn point(&self, r: f32, a: f32) -> Pos2 {
        let (s, c) = a.sin_cos();
        pos2(self.center.x + r * s, self.center.y - r * c)
    }

    /// Ring index and angle under `pos`; ring `None` means the center hole.
    fn locate(&self, pos: Pos2) -> Option<(Option<usize>, f32)> {
        let d = pos - self.center;
        let r = d.length();
        let angle = d.x.atan2(-d.y).rem_euclid(TAU);
        if r < self.hole {
            return Some((None, angle));
        }
        (0..self.rings)
            .find(|&i| {
                let (r0, r1) = self.radii(i as f32);
                r >= r0 && r < r1
            })
            .map(|ring| (Some(ring), angle))
    }
}

const ZOOM_SECONDS: f64 = 0.35;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Zoom {
    In,
    Out,
}

/// Animated change of the chart's root: the part of the chart that is shown in both
/// views morphs into place, everything else fades.
pub struct Transition {
    old: Layout,
    zoom: Zoom,
    /// Angular span of the smaller view inside the larger one.
    span: (f32, f32),
    /// Rings between the two roots' children.
    shift: f32,
    old_slots: usize,
    started: f64,
}

impl Transition {
    /// `None` if the views are unrelated or the target isn't visible in the other view.
    pub fn new(tree: &Tree, old: Layout, new: &Layout, now: f64) -> Option<Self> {
        if old.root == new.root {
            return None;
        }
        let (zoom, (ring, sector)) = if tree.is_within(new.root, old.root) {
            (Zoom::In, old.locate(new.root)?)
        } else if tree.is_within(old.root, new.root) {
            (Zoom::Out, new.locate(old.root)?)
        } else {
            return None;
        };
        let span = (sector.a0, sector.a1);
        let old_slots = old.rings.len();
        Some(Self { old, zoom, span, shift: ring as f32 + 1.0, old_slots, started: now })
    }

    /// Eased progress in `0..=1`.
    fn progress(&self, now: f64) -> f32 {
        let t = ((now - self.started) / ZOOM_SECONDS).clamp(0.0, 1.0) as f32;
        1.0 - (1.0 - t).powi(3)
    }

    pub fn finished(&self, now: f64) -> bool {
        now - self.started >= ZOOM_SECONDS
    }

    /// Where a sector of the new layout is drawn at progress `e`: (a0, a1, ring).
    fn morph(&self, s: &Sector, ring: usize, e: f32) -> (f32, f32, f32) {
        let (s0, s1) = self.span;
        let lerp = |a: f32, b: f32| a + (b - a) * e;
        match self.zoom {
            // Start where the new root's content was drawn in the old view.
            Zoom::In => {
                let from = |a: f32| s0 + a / TAU * (s1 - s0);
                (lerp(from(s.a0), s.a0), lerp(from(s.a1), s.a1), ring as f32 + self.shift * (1.0 - e))
            }
            // Start blown up so that the old root's content fills the whole circle.
            Zoom::Out => {
                let from = |a: f32| (a - s0) / (s1 - s0) * TAU;
                (lerp(from(s.a0), s.a0), lerp(from(s.a1), s.a1), ring as f32 - self.shift * (1.0 - e))
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    Center,
    Sector(Item),
}

/// What the app wants emphasized on the chart.
pub struct Highlight {
    pub selected: Option<NodeId>,
    pub hovered: Option<Item>,
}

pub struct ChartOutput {
    pub response: Response,
    pub hovered: Option<Hit>,
}

pub fn show(
    ui: &mut Ui,
    tree: &Tree,
    layout: &Layout,
    transition: Option<&Transition>,
    highlight: &Highlight,
) -> ChartOutput {
    let (response, painter) = ui.allocate_painter(ui.available_size(), Sense::click());
    let now = ui.input(|i| i.time);
    let target = Geometry::new(response.rect, layout.rings.len());
    let mut geo = target;
    let anim = transition.filter(|t| !t.finished(now)).map(|t| (t, t.progress(now)));
    if let Some((t, e)) = anim {
        let old = Geometry::new(response.rect, t.old_slots);
        geo.base = old.base + (target.base - old.base) * e;
        ui.ctx().request_repaint();
    }

    // Hit-test against where things end up, so clicks during the animation still work.
    let hovered = response.hover_pos().and_then(|pos| match target.locate(pos)? {
        (None, _) => Some(Hit::Center),
        (Some(ring), angle) => layout.find(ring, angle).map(|s| Hit::Sector(s.item)),
    });
    let hovered_item = match hovered {
        Some(Hit::Sector(item)) => Some(item),
        _ => highlight.hovered,
    };

    // Colors encode the share of the view; while zooming, slide the reference size from
    // the old root to the new one so hues shift smoothly.
    let reference = match anim {
        Some((t, e)) => {
            let (from, to) = ((t.old.total.max(1) as f32).ln(), (layout.total.max(1) as f32).ln());
            (from + (to - from) * e).exp()
        }
        None => layout.total.max(1) as f32,
    };

    let mut mesh = Mesh::default();

    // Zooming in: whatever is outside the new root stays in place and fades out.
    if let Some((t, e)) = anim
        && t.zoom == Zoom::In
    {
        for (ring, sectors) in t.old.rings.iter().enumerate() {
            let (r0, r1) = geo.radii(ring as f32);
            for s in sectors.iter().filter(|s| !tree.is_within(s.item.node(), layout.root)) {
                add_sector(&mut mesh, &geo, r0, r1, s.a0, s.a1, s.color.gamma_multiply(1.0 - e));
            }
        }
    }

    let mut selected_outline = None;
    for (ring, sectors) in layout.rings.iter().enumerate() {
        for s in sectors {
            let (a0, a1, ring_pos, mut color, alpha) = match anim {
                Some((t, e)) if t.zoom == Zoom::In || tree.is_within(s.item.node(), t.old.root) => {
                    let (a0, a1, r) = t.morph(s, ring, e);
                    let color = match s.item {
                        Item::Node(n) => {
                            let node = tree.node(n);
                            colors::sector(node.size as f32 / reference, r.max(0.0), node.is_dir())
                        }
                        Item::Small { .. } => colors::SMALL,
                    };
                    (a0, a1, r, color, 1.0)
                }
                // Zooming out: the surroundings of the old root fade in at their place.
                Some((_, e)) => (s.a0, s.a1, ring as f32, s.color, e),
                None => (s.a0, s.a1, ring as f32, s.color, 1.0),
            };
            if let Some(sel) = highlight.selected {
                if !tree.is_within(s.item.node(), sel) {
                    color = colors::dimmed(color);
                }
                if s.item == Item::Node(sel) {
                    selected_outline = Some((ring, *s));
                }
            }
            if hovered_item == Some(s.item) {
                color = colors::hovered(color);
            }
            let (r0, r1) = geo.radii(ring_pos);
            add_sector(&mut mesh, &geo, r0, r1, a0, a1, color.gamma_multiply(alpha));
        }
    }
    painter.add(Shape::mesh(mesh));

    if let (Some((ring, s)), None) = (selected_outline, anim) {
        let (r0, r1) = geo.radii(ring as f32);
        painter.add(Shape::closed_line(outline(&geo, r0, r1, s.a0, s.a1), Stroke::new(2.0, colors::ACCENT)));
    }

    let center_hint = match (hovered, highlight.selected) {
        (Some(Hit::Center), Some(_)) => Some("× deselect"),
        (Some(Hit::Center), None) if layout.root != Tree::ROOT => Some("⬆ up"),
        _ => None,
    };
    paint_center(&painter, &geo, tree, layout.root, center_hint);
    paint_legend(&painter, response.rect);

    ChartOutput { response, hovered }
}

fn arc_segments(span: f32) -> usize {
    ((span / ARC_STEP).ceil() as usize).max(1)
}

/// Adds a ring segment to `mesh`, inset by about a pixel so neighbours are visually separated.
///
/// Edges are anti-aliased with a one pixel alpha ramp (no MSAA needed, which not every
/// GPU/driver offers).
fn add_sector(mesh: &mut Mesh, geo: &Geometry, r0: f32, r1: f32, a0: f32, a1: f32, color: Color32) {
    const GAP: f32 = 0.6;
    const FEATHER: f32 = 1.0;
    let span = a1 - a0;
    if span <= 0.0 || r1 - r0 < 2.0 * GAP + 0.5 {
        return;
    }
    // A full ring has no radial edges: no gap and no feathering across its seam.
    let full = span >= TAU - 1e-4;
    let (r0, r1) = (r0 + GAP, r1 - GAP);
    let inset = |r: f32| if !full && span * r > 4.0 * GAP { GAP / r } else { 0.0 };
    let (i0, i1) = (inset(r0), inset(r1));
    let n = arc_segments(span);
    let lerp = |i: usize, inset: f32| a0 + inset + (span - 2.0 * inset) * i as f32 / n as f32;

    // Outline: outer arc forward, then inner arc backward.
    let mut pts = Vec::with_capacity(2 * (n + 1));
    pts.extend((0..=n).map(|i| geo.point(r1, lerp(i, i1))));
    pts.extend((0..=n).rev().map(|i| geo.point(r0, lerp(i, i0))));
    let m = pts.len();

    // Slivers thinner than the feather fade out by coverage instead.
    let thin = ((span - 2.0 * i0) * r0).min(r1 - r0);
    let color = if thin < 1.0 { color.gamma_multiply(thin.max(0.25)) } else { color };
    let half = FEATHER.min(thin) / 2.0;

    let area: f32 = (0..m).map(|k| pts[k].to_vec2().rot90().dot(pts[(k + 1) % m].to_vec2())).sum();
    let sign = if area < 0.0 { 1.0 } else { -1.0 };
    let seam = |k: usize| full && (k == n || k == m - 1); // edge k -> k+1 along the seam
    let normal = |k: usize| {
        if seam(k) {
            return Vec2::ZERO;
        }
        let d = pts[(k + 1) % m] - pts[k];
        let len = d.length();
        if len < 1e-6 { Vec2::ZERO } else { sign * Vec2::new(d.y, -d.x) / len }
    };

    let base = mesh.vertices.len() as u32;
    mesh.reserve_vertices(2 * m);
    mesh.reserve_triangles(2 * n + 2 * m);
    for (k, &p) in pts.iter().enumerate() {
        let (n_prev, n_next) = (normal((k + m - 1) % m), normal(k));
        let sum = n_prev + n_next;
        let offset = if sum.length() < 1e-6 {
            Vec2::ZERO
        } else {
            // Miter so both adjacent edges move by `half`; capped at sharp corners.
            let dir = sum.normalized();
            let edge = if n_prev == Vec2::ZERO { n_next } else { n_prev };
            dir * (half / dir.dot(edge).max(0.4))
        };
        mesh.colored_vertex(p - offset, color);
        mesh.colored_vertex(p + offset, Color32::TRANSPARENT);
    }
    let inner = |k: usize| base + 2 * k as u32;
    let outer = |k: usize| base + 2 * k as u32 + 1;
    for i in 0..n {
        mesh.add_triangle(inner(i), inner(i + 1), inner(m - 1 - i));
        mesh.add_triangle(inner(i + 1), inner(m - 2 - i), inner(m - 1 - i));
    }
    for k in (0..m).filter(|&k| !seam(k)) {
        let next = (k + 1) % m;
        mesh.add_triangle(inner(k), outer(k), inner(next));
        mesh.add_triangle(outer(k), outer(next), inner(next));
    }
}

fn outline(geo: &Geometry, r0: f32, r1: f32, a0: f32, a1: f32) -> Vec<Pos2> {
    let n = arc_segments(a1 - a0);
    let lerp = |i: usize| a0 + (a1 - a0) * i as f32 / n as f32;
    let mut points: Vec<Pos2> = (0..=n).map(|i| geo.point(r1, lerp(i))).collect();
    if (a1 - a0) >= TAU - 1e-4 {
        // Full ring: draw the outer circle only, the inner one would need a second path.
        points.pop();
        return points;
    }
    points.extend((0..=n).rev().map(|i| geo.point(r0, lerp(i))));
    points
}

/// Gradient explaining the colors, in the bottom-left corner.
fn paint_legend(painter: &Painter, rect: Rect) {
    const STEPS: usize = 24;
    let font = FontId::proportional(13.0);
    let bar = Rect::from_min_size(rect.left_bottom() + Vec2::new(16.0, -34.0), Vec2::new(180.0, 9.0));

    painter.text(
        bar.left_top() - Vec2::new(0.0, 5.0),
        Align2::LEFT_BOTTOM,
        "share of view",
        font.clone(),
        colors::TEXT_WEAK,
    );
    let mut mesh = Mesh::default();
    let (hi, lo) = (colors::SHARE_RED.log10(), colors::SHARE_VIOLET.log10());
    for i in 0..=STEPS {
        let t = i as f32 / STEPS as f32;
        let share = 10f32.powf(hi + (lo - hi) * t);
        let color = colors::sector(share, 0.0, true);
        let x = bar.left() + bar.width() * t;
        mesh.colored_vertex(pos2(x, bar.top()), color);
        mesh.colored_vertex(pos2(x, bar.bottom()), color);
        if i > 0 {
            let v = 2 * i as u32;
            mesh.add_triangle(v - 2, v - 1, v);
            mesh.add_triangle(v - 1, v + 1, v);
        }
    }
    painter.add(Shape::mesh(mesh));
    let below = |x: f32| pos2(x, bar.bottom() + 3.0);
    let pct = |share: f32| format::percent((share * 1e6) as u64, 1_000_000);
    let big = format!("≥{}", pct(colors::SHARE_RED));
    painter.text(below(bar.left()), Align2::LEFT_TOP, big, font.clone(), colors::TEXT_WEAK);
    let small = format!("≤{}", format_small_share(colors::SHARE_VIOLET));
    painter.text(below(bar.right()), Align2::RIGHT_TOP, small, font, colors::TEXT_WEAK);
}

/// `0.001` -> `0.1%` (format::percent stops at 0.1%).
fn format_small_share(share: f32) -> String {
    let p = share * 100.0;
    let digits = (-p.log10()).ceil().max(0.0) as usize;
    format!("{p:.digits$}%")
}

/// `hint` names what a click on the center does; the center is highlighted when it has one.
fn paint_center(painter: &Painter, geo: &Geometry, tree: &Tree, root: NodeId, hint: Option<&str>) {
    let fill = if hint.is_some() { colors::PANEL } else { colors::BG };
    painter.circle_filled(geo.center, geo.hole - 1.0, fill);

    let size = format::bytes(tree.node(root).size);
    let (number, unit) = size.split_once(' ').unwrap_or((&size, ""));
    let big = (geo.hole * 0.42).clamp(12.0, 44.0);
    painter.text(
        geo.center - Vec2::new(0.0, big * 0.45),
        Align2::CENTER_CENTER,
        number,
        FontId::proportional(big),
        colors::TEXT,
    );
    painter.text(
        geo.center + Vec2::new(0.0, big * 0.6),
        Align2::CENTER_CENTER,
        unit,
        FontId::proportional(big * 0.8),
        colors::TEXT,
    );
    if let Some(hint) = hint {
        painter.text(
            geo.center + Vec2::new(0.0, big * 1.55),
            Align2::CENTER_CENTER,
            hint,
            FontId::proportional((big * 0.4).max(11.0)),
            colors::TEXT_WEAK,
        );
    }
}

pub fn tooltip(ui: &mut egui::Ui, tree: &Tree, item: Item) {
    // The chart is one widget, so every sector shares one tooltip area, which remembers
    // its width. With wrapping text that width only ever shrinks (down to one letter per
    // line); unwrapped lines make the area fit its content every frame.
    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
    match item {
        Item::Node(n) => {
            let node = tree.node(n);
            ui.strong(ellipsize(&tree.display_name(n), 80));
            let parent_size = tree.parent(n).map_or(node.size, |p| tree.node(p).size);
            ui.label(format!(
                "{}  ·  {} of parent",
                format::bytes(node.size),
                format::percent(node.size, parent_size)
            ));
            if node.is_dir() {
                ui.label(format!("{} files", format::count(node.files.into())));
            }
        }
        Item::Small { parent, count, size } => {
            ui.strong(format!("{} smaller objects", format::count(count.into())));
            ui.label(format!("{}  ·  in {}", format::bytes(size), ellipsize(&tree.display_name(parent), 60)));
        }
    }
}

/// Shortens `text` to at most `max` characters, keeping both ends.
fn ellipsize(text: &str, max: usize) -> String {
    let count = text.chars().count();
    if count <= max {
        return text.to_owned();
    }
    let head = (max - 1) / 2;
    let tail = max - 1 - head;
    let start: String = text.chars().take(head).collect();
    let end: String = text.chars().skip(count - tail).collect();
    format!("{start}…{end}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tree::tests::{dir, file};
    use std::path::PathBuf;

    #[test]
    fn layout_covers_the_circle_in_order() {
        let t = crate::tree::tests::sample();
        let l = Layout::compute(&t, Tree::ROOT);
        // big.iso, photos, small.txt on ring 0 (empty dir has size 0), photos' files on ring 1.
        assert_eq!(l.rings.len(), 2);
        assert_eq!(l.rings[0].len(), 3);
        assert_eq!(l.rings[1].len(), 2);
        let ring0 = &l.rings[0];
        assert!(ring0[0].a0.abs() < 1e-6);
        assert!((ring0.last().unwrap().a1 - TAU).abs() < 1e-4);
        for w in ring0.windows(2) {
            assert!((w[0].a1 - w[1].a0).abs() < 1e-5);
        }
        let photos = l.sector_of(t.children(Tree::ROOT).nth(1).unwrap()).unwrap();
        assert!((l.rings[1][0].a0 - photos.a0).abs() < 1e-6);
        assert!((l.rings[1][1].a1 - photos.a1).abs() < 1e-5);
    }

    #[test]
    fn tiny_items_are_merged() {
        let mut children = vec![file("huge", 1_000_000)];
        children.extend((0..50).map(|i| file(&format!("tiny{i}"), 10)));
        let t = Tree::from_scan(PathBuf::from("/x"), dir("/x", children));
        let l = Layout::compute(&t, Tree::ROOT);
        assert_eq!(l.rings[0].len(), 2);
        assert_eq!(l.rings[0][1].item, Item::Small { parent: Tree::ROOT, count: 50, size: 500 });
    }

    #[test]
    fn ellipsizes_long_names() {
        assert_eq!(ellipsize("short", 10), "short");
        assert_eq!(ellipsize("abcdefghijkl", 7), "abc…jkl");
        assert_eq!(ellipsize("абвгдежзик", 5).chars().count(), 5);
    }

    #[test]
    fn hit_testing() {
        let t = crate::tree::tests::sample();
        let l = Layout::compute(&t, Tree::ROOT);
        let geo = Geometry::new(Rect::from_min_size(Pos2::ZERO, Vec2::splat(400.0)), l.rings.len());
        assert_eq!(geo.locate(geo.center).map(|h| h.0), Some(None));
        // Just right of 12 o'clock on the first ring: the largest item.
        let (r0, r1) = geo.radii(0.0);
        let (ring, angle) = geo.locate(geo.point((r0 + r1) / 2.0, 0.01)).unwrap();
        assert_eq!(ring, Some(0));
        let big = t.children(Tree::ROOT).next().unwrap();
        assert_eq!(l.find(0, angle).unwrap().item, Item::Node(big));
        assert!(geo.locate(pos2(0.0, 0.0)).is_none());
    }
}
