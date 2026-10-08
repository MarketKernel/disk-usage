//! Application state and UI layout.

use std::path::PathBuf;
use std::sync::atomic::Ordering::Relaxed;
use std::time::Duration;

use eframe::egui::{
    self, Align, Align2, Button, Color32, CornerRadius, FontId, Frame, Id, Key, Layout as UiLayout, Margin,
    Modal, RichText, Sense, Shape, Ui, UiBuilder, vec2,
};

use crate::colors;
use crate::format;
use crate::platform;
use crate::queue::TrashQueue;
use crate::scan::{Outcome, Scan};
use crate::sunburst::{self, Highlight, Hit, Item, Layout, Transition};
use crate::tree::{NodeId, Tree};

const ROW_HEIGHT: f32 = 24.0;
const STATUS_TEXT: f32 = 13.0;

pub struct App {
    tree: Option<Tree>,
    scan: Option<Scan>,
    /// Errors and duration of the last finished scan.
    stats: Option<(u64, Duration)>,
    /// Free and total bytes of the scanned volume.
    disk: Option<(u64, u64)>,
    /// Node shown in the center of the chart.
    view: NodeId,
    selected: Option<NodeId>,
    layout: Option<Layout>,
    /// Zoom animation between the previous and the current layout.
    transition: Option<Transition>,
    chart_hover: Option<Item>,
    list_hover: Option<NodeId>,
    /// Node the chart's context menu was opened for.
    menu_target: Option<NodeId>,
    /// Folder count seen during a scan and when it last changed, to notice stalls.
    scan_seen: (u64, f64),
    /// Last clicked list row and when, to recognize a double-click on it.
    last_row_click: Option<(NodeId, f64)>,
    /// Items to move to the Trash together, once the user confirms the whole list.
    queue: TrashQueue,
    about: bool,
    message: Option<String>,
    quick: Vec<PathBuf>,
    #[cfg(debug_assertions)]
    shot: Option<crate::shot::Shot>,
}

enum Action {
    PickFolder,
    Scan(PathBuf),
    Rescan,
    CancelScan,
    Select(Option<NodeId>),
    Zoom(NodeId),
    Up,
    Reveal(NodeId),
    Open(NodeId),
    CopyPath(NodeId),
    /// Selects a node, moving the view to its folder if it isn't on the chart.
    Locate(NodeId),
    Queue(NodeId),
    Unqueue(NodeId),
    ClearQueue,
    TrashQueued,
    About,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, path: Option<PathBuf>) -> Self {
        setup_style(&cc.egui_ctx);
        let mut quick: Vec<PathBuf> = platform::home_dir().into_iter().collect();
        quick.extend(platform::volumes());
        let mut app = Self {
            tree: None,
            scan: None,
            stats: None,
            disk: None,
            view: Tree::ROOT,
            selected: None,
            layout: None,
            transition: None,
            chart_hover: None,
            list_hover: None,
            menu_target: None,
            scan_seen: (0, 0.0),
            last_row_click: None,
            queue: TrashQueue::default(),
            about: false,
            message: None,
            quick,
            #[cfg(debug_assertions)]
            shot: crate::shot::Shot::from_env(),
        };
        if let Some(path) = path {
            app.start_scan(&cc.egui_ctx, path);
        }
        app
    }

    /// Starts scanning `path`. The current results stay until the new ones arrive,
    /// so cancelling a rescan keeps them.
    fn start_scan(&mut self, ctx: &egui::Context, path: PathBuf) {
        if let Some(scan) = self.scan.take() {
            scan.cancel();
        }
        let path = std::path::absolute(&path).unwrap_or(path);
        self.message = None;
        let ctx2 = ctx.clone();
        self.scan = Some(Scan::start(path, move || ctx2.request_repaint()));
    }

    fn poll_scan(&mut self, ctx: &egui::Context) {
        let Some(done) = self.scan.as_ref().and_then(Scan::poll) else { return };
        self.scan = None;
        match done.outcome {
            Outcome::Done(tree) => {
                ctx.send_viewport_cmd(egui::ViewportCommand::Title(format!(
                    "{} — {}",
                    crate::TITLE,
                    tree.root_path().display()
                )));
                self.disk = platform::disk_space(tree.root_path());
                self.stats = Some((done.errors, done.elapsed));
                if let Some(old) = self.tree.replace(tree) {
                    // Freeing millions of nodes takes a moment; don't stall the UI for it.
                    std::thread::spawn(move || drop(old));
                }
                // Node ids of the old tree must not leak into the new one.
                self.layout = None;
                self.transition = None;
                self.view = Tree::ROOT;
                self.selected = None;
                self.chart_hover = None;
                self.list_hover = None;
                self.menu_target = None;
                self.last_row_click = None;
                self.queue.clear();
            }
            Outcome::Cancelled => self.message = Some("Scan cancelled".into()),
            Outcome::Failed(error) => self.message = Some(error),
        }
    }

    fn apply(&mut self, ctx: &egui::Context, action: Action) {
        match action {
            Action::PickFolder => {
                if let Some(path) = rfd::FileDialog::new().pick_folder() {
                    self.start_scan(ctx, path);
                }
            }
            Action::Scan(path) => self.start_scan(ctx, path),
            Action::Rescan => {
                let root = self.tree.as_ref().map(|t| t.root_path().to_path_buf());
                if let Some(root) = root {
                    self.start_scan(ctx, root);
                }
            }
            Action::CancelScan => {
                if let Some(scan) = &self.scan {
                    scan.cancel();
                }
            }
            Action::Select(node) => self.selected = node.filter(|&n| n != self.view),
            Action::Zoom(node) => {
                let Some(tree) = &self.tree else { return };
                if tree.node(node).is_dir() {
                    // Zooming out keeps the folder we came from selected.
                    let came_from = tree.is_within(self.view, node).then_some(self.view);
                    self.view = node;
                    self.selected = came_from.filter(|&n| n != node);
                } else if let Some(parent) = tree.parent(node) {
                    self.view = parent;
                    self.selected = Some(node);
                }
            }
            Action::Up => {
                let Some(tree) = &self.tree else { return };
                if let Some(parent) = tree.parent(self.view) {
                    self.selected = Some(self.view);
                    self.view = parent;
                }
            }
            Action::Reveal(node) => self.with_path(node, |p| platform::reveal(&p)),
            Action::Open(node) => self.with_path(node, |p| platform::open(&p)),
            Action::CopyPath(node) => {
                self.with_path(node, |p| ctx.copy_text(p.to_string_lossy().into_owned()))
            }
            Action::Locate(node) => {
                let Some(tree) = &self.tree else { return };
                if self.layout.as_ref().is_none_or(|l| l.sector_of(node).is_none()) {
                    self.view = tree.parent(node).unwrap_or(Tree::ROOT);
                }
                self.selected = Some(node).filter(|&n| n != self.view);
            }
            Action::Queue(node) => {
                if let Some(tree) = &self.tree {
                    self.queue.add(tree, node);
                }
            }
            Action::Unqueue(node) => self.queue.remove(node),
            Action::ClearQueue => self.queue.clear(),
            Action::TrashQueued => self.trash_queued(),
            Action::About => self.about = true,
        }
    }

    fn with_path(&self, node: NodeId, f: impl FnOnce(PathBuf)) {
        if let Some(tree) = &self.tree {
            f(tree.path(node));
        }
    }

    /// Moves everything queued to the Trash; what fails stays in the queue.
    fn trash_queued(&mut self) {
        let Some(tree) = &mut self.tree else { return };
        let (mut moved, mut freed, mut failed) = (Vec::new(), 0, Vec::new());
        for node in self.queue.items().to_vec() {
            let path = tree.path(node);
            match platform::move_to_trash(&path) {
                Ok(()) => {
                    freed += tree.node(node).size;
                    tree.remove(node);
                    self.queue.remove(node);
                    // Queued items don't overlap, so the parent of one is never inside another.
                    if tree.is_within(self.view, node) {
                        self.view = tree.parent(node).unwrap_or(Tree::ROOT);
                    }
                    if self.selected.is_some_and(|s| tree.is_within(s, node)) {
                        self.selected = None;
                    }
                    moved.push(path);
                }
                Err(e) => failed.push(format!("Could not move {} to Trash: {e}", path.display())),
            }
        }
        if !moved.is_empty() {
            self.layout = None;
            self.disk = platform::disk_space(tree.root_path());
        }
        let mut parts = Vec::new();
        match moved.as_slice() {
            [] => {}
            [path] => parts.push(format!("Moved to Trash: {} ({})", path.display(), format::bytes(freed))),
            _ => parts.push(format!("Moved {} items to Trash ({})", moved.len(), format::bytes(freed))),
        }
        if let Some(first) = failed.first() {
            parts.push(match failed.len() {
                1 => first.clone(),
                n => format!("{first} (and {} more, still queued)", n - 1),
            });
        }
        self.message = Some(parts.join("  ·  "));
    }

    fn handle_input(&self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        let dropped = ctx.input(|i| i.raw.dropped_files.iter().find_map(|f| f.path.clone()));
        if let Some(path) = dropped {
            actions.push(Action::Scan(path));
        }
        if ctx.egui_wants_keyboard_input() || self.about {
            return;
        }
        ctx.input_mut(|i| {
            if i.consume_key(egui::Modifiers::COMMAND, Key::O) {
                actions.push(Action::PickFolder);
            }
            if i.consume_key(egui::Modifiers::COMMAND, Key::R)
                || i.consume_key(egui::Modifiers::NONE, Key::F5)
            {
                actions.push(Action::Rescan);
            }
            if self.tree.is_none() {
                return;
            }
            if i.consume_key(egui::Modifiers::NONE, Key::Escape) {
                actions.push(if self.selected.is_some() { Action::Select(None) } else { Action::Up });
            }
            // Delete, or ⌘⌫ like in Finder; checked before the plain Backspace.
            if let Some(sel) = self.selected
                && (i.consume_key(egui::Modifiers::NONE, Key::Delete)
                    || i.consume_key(egui::Modifiers::COMMAND, Key::Backspace))
            {
                actions.push(if self.queue.contains(sel) {
                    Action::Unqueue(sel)
                } else {
                    Action::Queue(sel)
                });
            }
            if i.consume_key(egui::Modifiers::NONE, Key::Backspace) {
                actions.push(Action::Up);
            }
            if i.consume_key(egui::Modifiers::NONE, Key::Enter)
                && let Some(sel) = self.selected
            {
                actions.push(Action::Zoom(sel));
            }
        });
    }
}

impl eframe::App for App {
    #[cfg(debug_assertions)]
    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        if let Some(shot) = &self.shot {
            shot.input_hook(raw_input);
        }
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        colors::BG.to_normalized_gamma_f32()
    }

    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.poll_scan(&ctx);

        let mut actions = Vec::new();
        self.handle_input(&ctx, &mut actions);

        if let (Some(tree), true) = (&self.tree, self.layout.as_ref().is_none_or(|l| l.root != self.view)) {
            let layout = Layout::compute(tree, self.view);
            let now = ctx.input(|i| i.time);
            self.transition = self.layout.take().and_then(|old| Transition::new(tree, old, &layout, now));
            self.layout = Some(layout);
        }

        toolbar(ui, self, &mut actions);
        status_bar(ui, self);

        if let Some(scan) = &self.scan {
            scanning_screen(ui, scan, &mut self.scan_seen, &mut actions);
            ctx.request_repaint_after(Duration::from_millis(80));
        } else if let (Some(tree), Some(layout)) = (&self.tree, &self.layout) {
            let view = View {
                tree,
                layout,
                transition: self.transition.as_ref(),
                root: self.view,
                selected: self.selected,
                chart_hover: self.chart_hover,
                list_hover: self.list_hover,
                queue: &self.queue,
            };
            let details_hover = details_panel(ui, &view, &mut self.last_row_click, &mut actions);
            let queue_hover = if self.queue.is_empty() { None } else { queue_panel(ui, &view, &mut actions) };
            self.list_hover = details_hover.or(queue_hover);
            let (hover, menu_target) = chart_panel(ui, &view, self.menu_target, &mut actions);
            self.chart_hover = hover;
            self.menu_target = menu_target;
        } else {
            start_screen(ui, &self.quick, self.message.as_deref(), &mut actions);
        }

        if self.about && about_modal(&ctx) {
            self.about = false;
        }

        #[cfg(debug_assertions)]
        if let Some(shot) = &mut self.shot
            && shot.update(&ctx, if shot.during_scan { self.scan.is_some() } else { self.scan.is_none() })
            && let Some(tree) = &self.tree
        {
            let nth = |n: Option<usize>| n.and_then(|n| tree.children(Tree::ROOT).nth(n));
            if let Some(n) = nth(shot.zoom) {
                actions.push(Action::Zoom(n));
            }
            if let Some(n) = nth(shot.select) {
                actions.push(Action::Select(Some(n)));
            }
            for &q in &shot.queue {
                if let Some(n) = nth(Some(q)) {
                    actions.push(Action::Queue(n));
                }
            }
        }

        for action in actions {
            self.apply(&ctx, action);
        }
    }
}

/// Read-only snapshot of what the panels need.
struct View<'a> {
    tree: &'a Tree,
    layout: &'a Layout,
    transition: Option<&'a Transition>,
    root: NodeId,
    selected: Option<NodeId>,
    chart_hover: Option<Item>,
    list_hover: Option<NodeId>,
    queue: &'a TrashQueue,
}

fn setup_style(ctx: &egui::Context) {
    ctx.set_theme(egui::ThemePreference::Dark);
    ctx.style_mut_of(egui::Theme::Dark, |style| {
        let v = &mut style.visuals;
        v.panel_fill = colors::BG;
        v.window_fill = colors::PANEL;
        v.extreme_bg_color = colors::BG;
        v.faint_bg_color = colors::PANEL;
        v.override_text_color = Some(colors::TEXT);
        style.interaction.tooltip_delay = 0.0;
        style.spacing.item_spacing = vec2(8.0, 6.0);
        style.spacing.button_padding = vec2(8.0, 4.0);
    });
}

fn toolbar(ui: &mut Ui, app: &App, actions: &mut Vec<Action>) {
    egui::Panel::top("toolbar")
        .frame(Frame::NONE.fill(colors::PANEL).inner_margin(Margin::symmetric(10, 8)))
        .show(ui, |ui| {
            ui.with_layout(UiLayout::right_to_left(Align::Center), |ui| {
                if ui.button("ℹ").on_hover_text("About").clicked() {
                    actions.push(Action::About);
                }
                ui.with_layout(UiLayout::left_to_right(Align::Center), |ui| toolbar_left(ui, app, actions));
            });
        });
}

fn toolbar_left(ui: &mut Ui, app: &App, actions: &mut Vec<Action>) {
    if ui.button("🗁 Open…").on_hover_text("Choose a folder to scan (Ctrl/⌘+O)").clicked() {
        actions.push(Action::PickFolder);
    }
    let can_rescan = app.tree.is_some() && app.scan.is_none();
    if ui.add_enabled(can_rescan, Button::new("⟳ Rescan")).on_hover_text("Scan again (F5)").clicked() {
        actions.push(Action::Rescan);
    }
    let Some(tree) = app.tree.as_ref().filter(|_| app.scan.is_none()) else { return };
    let can_up = app.view != Tree::ROOT;
    if ui.add_enabled(can_up, Button::new("⬆")).on_hover_text("Up one level (Backspace)").clicked() {
        actions.push(Action::Up);
    }
    ui.separator();
    breadcrumbs(ui, tree, app.view, actions);
}

fn breadcrumbs(ui: &mut Ui, tree: &Tree, view: NodeId, actions: &mut Vec<Action>) {
    egui::ScrollArea::horizontal().stick_to_right(true).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            let chain = tree.ancestry(view);
            for (i, &n) in chain.iter().enumerate() {
                if i > 0 {
                    ui.label(RichText::new("›").color(colors::TEXT_WEAK));
                }
                let last = i + 1 == chain.len();
                let text = RichText::new(tree.display_name(n));
                let text = if last { text.strong() } else { text };
                if ui.add(Button::new(text).frame(false)).clicked() && !last {
                    actions.push(Action::Zoom(n));
                }
            }
        });
    });
}

fn status_bar(ui: &mut Ui, app: &App) {
    egui::Panel::bottom("status")
        .frame(Frame::NONE.fill(colors::PANEL).inner_margin(Margin::symmetric(12, 7)))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let weak = |s: String| RichText::new(s).color(colors::TEXT_WEAK).size(STATUS_TEXT);
                if let Some(msg) = &app.message {
                    ui.label(RichText::new(msg).size(STATUS_TEXT));
                }
                if let (Some(tree), None) = (&app.tree, &app.scan) {
                    if app.message.is_some() {
                        ui.separator();
                    }
                    let root = tree.node(Tree::ROOT);
                    let mut parts =
                        vec![format!("{} files", format::count(root.files.into())), format::bytes(root.size)];
                    if let Some((errors, elapsed)) = app.stats {
                        if errors > 0 {
                            parts.push(format!("{} unreadable", format::count(errors)));
                        }
                        parts.push(format!("scanned in {:.1} s", elapsed.as_secs_f32()));
                    }
                    if let Some((free, total)) = app.disk {
                        parts.push(format!("disk: {} free of {}", format::bytes(free), format::bytes(total)));
                    }
                    ui.label(weak(parts.join("  ·  ")));
                    ui.with_layout(UiLayout::right_to_left(Align::Center), |ui| {
                        ui.label(weak("click: select · center: deselect / up · double-click: zoom · right-click: actions".into()));
                    });
                }
            });
        });
}

fn start_screen(ui: &mut Ui, quick: &[PathBuf], message: Option<&str>, actions: &mut Vec<Action>) {
    egui::CentralPanel::no_frame().show(ui, |ui| {
        ui.vertical_centered(|ui| {
            ui.add_space((ui.available_height() * 0.25).max(20.0));
            ui.label(RichText::new("Disk Usage").size(32.0).strong());
            ui.label(RichText::new("See where your disk space goes").color(colors::TEXT_WEAK));
            if let Some(message) = message {
                ui.add_space(12.0);
                ui.label(RichText::new(message).color(colors::WARNING));
            }
            ui.add_space(24.0);
            if ui
                .add(Button::new(RichText::new("🗁  Choose folder…").size(18.0)).min_size(vec2(220.0, 40.0)))
                .clicked()
            {
                actions.push(Action::PickFolder);
            }
            ui.add_space(16.0);
            ui.label(RichText::new("or drop a folder here, or scan:").color(colors::TEXT_WEAK));
            ui.add_space(4.0);
            for path in quick {
                if ui.add(Button::new(path.to_string_lossy()).min_size(vec2(220.0, 0.0))).clicked() {
                    actions.push(Action::Scan(path.clone()));
                }
            }
        });
    });
}

#[cfg(target_os = "macos")]
const STALL_HINT: &str = "Waiting for the file system… macOS may be asking for permission to access a folder.\n\
     Grant \"Full Disk Access\" in System Settings → Privacy & Security to avoid the prompts.";
#[cfg(not(target_os = "macos"))]
const STALL_HINT: &str = "Waiting for the file system… (a slow network or removable drive?)";

fn scanning_screen(ui: &mut Ui, scan: &Scan, seen: &mut (u64, f64), actions: &mut Vec<Action>) {
    egui::CentralPanel::no_frame().show(ui, |ui| {
        ui.vertical_centered(|ui| {
            ui.add_space((ui.available_height() * 0.3).max(20.0));
            ui.add(egui::Spinner::new().size(40.0));
            ui.add_space(12.0);
            ui.label(RichText::new(format!("Scanning {}", scan.root.display())).size(20.0));
            ui.add_space(4.0);
            let p = &scan.progress;
            ui.label(
                RichText::new(format!(
                    "{} files  ·  {} folders  ·  {}  ·  {:.0} s",
                    format::count(p.files.load(Relaxed)),
                    format::count(p.dirs.load(Relaxed)),
                    format::bytes(p.bytes.load(Relaxed)),
                    scan.started.elapsed().as_secs_f32(),
                ))
                .size(16.0),
            );
            ui.add_space(8.0);
            // Left-aligned in a fixed box, so the common start of the path stays put and
            // only its changing tail flickers.
            let width = (ui.available_width() - 48.0).clamp(200.0, 900.0);
            ui.allocate_ui_with_layout(vec2(width, 22.0), UiLayout::left_to_right(Align::Center), |ui| {
                ui.set_width(width);
                if let Ok(current) = p.current.try_lock() {
                    let text = RichText::new(current.to_string_lossy()).size(15.0).color(colors::TEXT_WEAK);
                    ui.add(egui::Label::new(text).truncate());
                }
            });
            // Opening a protected folder blocks until the user answers the OS prompt.
            let (dirs, now) = (p.dirs.load(Relaxed), ui.input(|i| i.time));
            if dirs != seen.0 {
                *seen = (dirs, now);
            } else if now - seen.1 > 3.0 {
                ui.add_space(8.0);
                ui.label(RichText::new(STALL_HINT).color(colors::WARNING));
            }
            ui.add_space(20.0);
            let cancel = Button::new(RichText::new("Cancel").size(17.0)).min_size(vec2(160.0, 38.0));
            if ui.add(cancel).clicked() {
                actions.push(Action::CancelScan);
            }
        });
    });
}

/// Draws the chart; returns the hovered item and the context menu target.
fn chart_panel(
    ui: &mut Ui,
    view: &View,
    menu_target: Option<NodeId>,
    actions: &mut Vec<Action>,
) -> (Option<Item>, Option<NodeId>) {
    let tree = view.tree;
    let mut menu_target = menu_target;
    let mut hovered_item = None;
    egui::CentralPanel::no_frame().show(ui, |ui| {
        let highlight = Highlight {
            selected: view.selected,
            hovered: view.list_hover.map(Item::Node),
            queue: view.queue,
        };
        let out = sunburst::show(ui, tree, view.layout, view.transition, &highlight);
        let response = out.response;

        if let Some(Hit::Sector(item)) = out.hovered {
            hovered_item = Some(item);
        }
        if response.double_clicked() {
            if let Some(Hit::Sector(Item::Node(n))) = out.hovered
                && tree.node(n).is_dir()
            {
                actions.push(Action::Zoom(n));
            }
        } else if response.clicked() {
            actions.push(match out.hovered {
                // Like Escape: first clear the selection, then go up.
                Some(Hit::Center) if view.selected.is_some() => Action::Select(None),
                Some(Hit::Center) => Action::Up,
                Some(Hit::Sector(item)) => Action::Select(Some(item.node())),
                None => Action::Select(None),
            });
        }
        if response.secondary_clicked() {
            menu_target = match out.hovered {
                Some(Hit::Sector(item)) => Some(item.node()),
                _ => Some(view.root),
            };
        }

        let response = match hovered_item {
            Some(item) if !response.context_menu_opened() => {
                response.on_hover_ui_at_pointer(|ui| sunburst::tooltip(ui, tree, item))
            }
            _ => response,
        };
        if let Some(target) = menu_target {
            response.context_menu(|ui| context_menu(ui, view, target, actions));
        }
    });
    (hovered_item, menu_target)
}

/// Lists the content of the selected item; returns the hovered row.
fn details_panel(
    ui: &mut Ui,
    view: &View,
    last_click: &mut Option<(NodeId, f64)>,
    actions: &mut Vec<Action>,
) -> Option<NodeId> {
    let tree = view.tree;
    let target = view.selected.unwrap_or(view.root);
    let node = tree.node(target);
    // For a file, list its folder with the file highlighted.
    let (folder, marked) =
        if node.is_dir() { (target, None) } else { (tree.parent(target).unwrap_or(target), Some(target)) };
    let mut hovered = None;

    // Drag its right edge to resize; the chart takes whatever is left.
    let max_width = (ui.available_width() * 0.6).max(280.0);
    egui::Panel::left("details")
        .resizable(true)
        .default_size(380.0)
        .size_range(260.0..=max_width)
        .frame(Frame::NONE.fill(colors::BG).inner_margin(Margin { left: 12, right: 10, top: 12, bottom: 4 }))
        .show(ui, |ui| {
            details_header(ui, view, target, actions);
            ui.add_space(6.0);
            ui.separator();
            ui.add_space(2.0);

            // The first click of a double-click on a folder row already swaps the list
            // to its content, so the second one lands on another row: zoom into the
            // row that was clicked first.
            let now = ui.input(|i| i.time);
            let double = ui.input(|i| i.pointer.button_double_clicked(egui::PointerButton::Primary));
            if double
                && ui.ui_contains_pointer()
                && let Some((id, at)) = *last_click
                && now - at < 1.0
                && tree.node(id).is_dir()
            {
                actions.push(Action::Zoom(id));
                *last_click = None;
            }

            let rows = tree.sorted_children(folder);
            if rows.is_empty() {
                let text = if node.is_dir() { "Empty folder" } else { "" };
                ui.label(RichText::new(text).color(colors::TEXT_WEAK));
                return;
            }
            let max = tree.node(rows[0]).size.max(1);
            let total = tree.node(folder).size;
            egui::ScrollArea::vertical().auto_shrink([false, false]).show_rows(
                ui,
                ROW_HEIGHT,
                rows.len(),
                |ui, range| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    for &id in &rows[range] {
                        let chart_hovered = view.chart_hover.map(Item::node) == Some(id);
                        let active = marked == Some(id) || chart_hovered;
                        let resp = list_row(ui, view, id, max, total, active);
                        if resp.hovered() {
                            hovered = Some(id);
                        }
                        if resp.clicked() && !double {
                            actions.push(Action::Select(Some(id)));
                            *last_click = Some((id, now));
                        }
                        resp.context_menu(|ui| context_menu(ui, view, id, actions));
                    }
                },
            );
        });
    hovered
}

fn details_header(ui: &mut Ui, view: &View, target: NodeId, actions: &mut Vec<Action>) {
    let tree = view.tree;
    let node = tree.node(target);
    let path = tree.path(target);

    // The full path is right below the name.
    let name = tree.short_name(target);
    ui.horizontal(|ui| {
        let mut color = view.layout.sector_of(target).map_or(colors::SMALL, |s| s.color);
        if view.queue.covers(tree, target) {
            color = colors::queued(color);
        }
        let (rect, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
        ui.painter().rect_filled(rect, CornerRadius::same(3), color);
        ui.add(egui::Label::new(RichText::new(name).size(18.0).strong()).wrap());
    });

    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(format::bytes(node.size)).size(22.0).strong());
        let mut facts = Vec::new();
        if target != view.root {
            let share = format::percent(node.size, tree.node(view.root).size);
            facts.push(format!("{share} of {}", tree.short_name(view.root)));
        }
        if node.is_dir() {
            facts.push(format!("{} files", format::count(node.files.into())));
        }
        if !facts.is_empty() {
            ui.label(RichText::new(facts.join("  ·  ")).size(14.0).color(colors::TEXT_WEAK));
        }
    });

    ui.add(
        egui::Label::new(RichText::new(path.to_string_lossy()).size(14.0).color(colors::TEXT_WEAK)).wrap(),
    );

    ui.add_space(4.0);
    ui.horizontal(|ui| {
        if node.is_dir() && target != view.root && ui.button("Zoom in").clicked() {
            actions.push(Action::Zoom(target));
        }
        if ui.button("Reveal").on_hover_text(format!("Show in {}", platform::FILE_MANAGER)).clicked() {
            actions.push(Action::Reveal(target));
        }
        if target != Tree::ROOT {
            trash_button(ui, view, target, actions);
        }
    });
}

/// Adds `node` to the Trash queue, or takes it off.
fn trash_button(ui: &mut Ui, view: &View, node: NodeId, actions: &mut Vec<Action>) {
    if view.queue.contains(node) {
        if ui.button("Remove from queue").on_hover_text("Keep it: take it off the Trash queue").clicked() {
            actions.push(Action::Unqueue(node));
        }
        return;
    }
    let covered = view.queue.covers(view.tree, node);
    let resp = ui
        .add_enabled(!covered, Button::new("🗑 Trash"))
        .on_hover_text("Add to the Trash queue (Del)")
        .on_disabled_hover_text("Its folder is already in the Trash queue");
    if resp.clicked() {
        actions.push(Action::Queue(node));
    }
}

fn list_row(ui: &mut Ui, view: &View, id: NodeId, max: u64, total: u64, active: bool) -> egui::Response {
    let tree = view.tree;
    let node = tree.node(id);
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), Sense::click());
    let painter = ui.painter_at(rect);
    let queued = view.queue.covers(tree, id);
    let color = view.layout.sector_of(id).map_or(colors::SMALL, |s| s.color);
    let color = if queued { colors::queued(color) } else { color };

    if resp.hovered() || active {
        painter.rect_filled(rect, CornerRadius::same(4), colors::PANEL);
    }
    // Size bar behind the text, relative to the largest sibling.
    let frac = node.size as f32 / max as f32;
    let bar =
        egui::Rect::from_min_size(rect.min + vec2(0.0, 3.0), vec2(rect.width() * frac, rect.height() - 6.0));
    painter.rect_filled(bar, CornerRadius::same(3), color.gamma_multiply(0.18));
    painter.rect_filled(
        egui::Rect::from_min_size(rect.min + vec2(0.0, 3.0), vec2(4.0, rect.height() - 6.0)),
        CornerRadius::same(2),
        color,
    );

    let font = FontId::proportional(14.0);
    let right = rect.right() - 8.0;
    let y = rect.center().y;
    painter.text(
        egui::pos2(right, y),
        Align2::RIGHT_CENTER,
        format::percent(node.size, total),
        font.clone(),
        colors::TEXT_WEAK,
    );
    painter.text(
        egui::pos2(right - 52.0, y),
        Align2::RIGHT_CENTER,
        format::bytes(node.size),
        font.clone(),
        colors::TEXT,
    );

    let icon = if node.is_dir() { "🗀" } else { "🗋" };
    let name_rect = egui::Rect::from_min_max(rect.min, egui::pos2(right - 128.0, rect.max.y));
    let text_color = match (active, queued) {
        (true, _) => Color32::WHITE,
        (false, true) => colors::TEXT_WEAK,
        (false, false) => colors::TEXT,
    };
    painter.with_clip_rect(name_rect).text(
        egui::pos2(rect.left() + 12.0, y),
        Align2::LEFT_CENTER,
        format!("{icon}  {}", node.name_lossy()),
        font,
        text_color,
    );
    resp
}

/// The items queued for the Trash; returns the hovered one.
fn queue_panel(ui: &mut Ui, view: &View, actions: &mut Vec<Action>) -> Option<NodeId> {
    let tree = view.tree;
    let items = view.queue.items();
    let mut hovered = None;

    let max_width = (ui.available_width() * 0.5).max(260.0);
    egui::Panel::right("queue")
        .resizable(true)
        .default_size(320.0)
        .size_range(240.0..=max_width)
        .frame(Frame::NONE.fill(colors::BG).inner_margin(Margin { left: 10, right: 12, top: 12, bottom: 4 }))
        .show(ui, |ui| {
            ui.label(RichText::new("Trash queue").size(18.0).strong());
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(format::bytes(view.queue.size(tree))).size(22.0).strong());
                let count = match items.len() {
                    1 => "1 item".to_owned(),
                    n => format!("{} items", format::count(n as u64)),
                };
                ui.label(RichText::new(count).size(14.0).color(colors::TEXT_WEAK));
            });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let trash =
                    Button::new(RichText::new("🗑 Move to Trash").color(Color32::WHITE)).fill(colors::DANGER);
                if ui.add(trash).on_hover_text("Move everything listed here to the Trash").clicked() {
                    actions.push(Action::TrashQueued);
                }
                if ui.button("Clear all").on_hover_text("Empty the list; nothing is moved").clicked() {
                    actions.push(Action::ClearQueue);
                }
            });
            ui.add_space(6.0);
            ui.separator();
            ui.add_space(2.0);

            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                for &id in items {
                    let resp = queue_row(ui, view, id, actions);
                    if resp.contains_pointer() {
                        hovered = Some(id);
                    }
                    if resp.clicked() {
                        actions.push(Action::Locate(id));
                    }
                    resp.context_menu(|ui| context_menu(ui, view, id, actions));
                }
            });
        });
    hovered
}

/// Name and size, then the folder it is in, with a button to take it off the queue.
fn queue_row(ui: &mut Ui, view: &View, id: NodeId, actions: &mut Vec<Action>) -> egui::Response {
    let tree = view.tree;
    let node = tree.node(id);
    // Painted once the row's height is known.
    let background = ui.painter().add(Shape::Noop);
    let resp = ui
        .scope_builder(UiBuilder::new().sense(Sense::click()), |ui| {
            Frame::NONE.inner_margin(Margin::symmetric(6, 4)).show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                ui.horizontal(|ui| {
                    ui.with_layout(UiLayout::right_to_left(Align::Center), |ui| {
                        let remove =
                            Button::new(RichText::new("×").size(18.0).color(colors::TEXT_WEAK)).frame(false);
                        if ui.add(remove).on_hover_text("Remove from the queue").clicked() {
                            actions.push(Action::Unqueue(id));
                        }
                        ui.add(
                            egui::Label::new(RichText::new(format::bytes(node.size)).size(14.0))
                                .selectable(false),
                        );
                        ui.with_layout(UiLayout::left_to_right(Align::Center), |ui| {
                            let icon = if node.is_dir() { "🗀" } else { "🗋" };
                            let name = RichText::new(format!("{icon}  {}", node.name_lossy())).size(14.0);
                            ui.add(egui::Label::new(name).truncate().selectable(false));
                        });
                    });
                });
                let folder = tree.parent(id).map(|p| tree.path(p)).unwrap_or_default();
                let folder = RichText::new(folder.to_string_lossy()).size(14.0).color(colors::TEXT_WEAK);
                ui.add(egui::Label::new(folder).wrap().selectable(false));
            });
        })
        .response;
    let chart_hovered = view.chart_hover.map(Item::node) == Some(id);
    if resp.contains_pointer() || chart_hovered {
        ui.painter().set(background, Shape::rect_filled(resp.rect, CornerRadius::same(4), colors::PANEL));
    }
    resp
}

fn context_menu(ui: &mut Ui, view: &View, node: NodeId, actions: &mut Vec<Action>) {
    let tree = view.tree;
    ui.set_min_width(180.0);
    ui.label(RichText::new(tree.display_name(node)).strong());
    ui.separator();
    let mut item = |ui: &mut Ui, label: &str, action: Action| {
        if ui.button(label).clicked() {
            actions.push(action);
            ui.close();
        }
    };
    if tree.node(node).is_dir() {
        item(ui, "Zoom in", Action::Zoom(node));
    }
    item(ui, &format!("Show in {}", platform::FILE_MANAGER), Action::Reveal(node));
    item(ui, "Open", Action::Open(node));
    item(ui, "Copy path", Action::CopyPath(node));
    if node != Tree::ROOT {
        ui.separator();
        if view.queue.contains(node) {
            item(ui, "Remove from Trash queue", Action::Unqueue(node));
        } else if view.queue.covers(tree, node) {
            ui.add_enabled(false, Button::new("In Trash queue with its folder"));
        } else {
            item(ui, "Add to Trash queue", Action::Queue(node));
        }
    }
}

/// Returns true once the dialog should close.
fn about_modal(ctx: &egui::Context) -> bool {
    let mut close = false;
    let modal = Modal::new(Id::new("about")).show(ctx, |ui| {
        ui.set_max_width(360.0);
        ui.vertical_centered(|ui| {
            ui.label(RichText::new("Disk Usage").size(22.0).strong());
            ui.label(
                RichText::new(format!("Version {}", env!("CARGO_PKG_VERSION"))).color(colors::TEXT_WEAK),
            );
            ui.add_space(8.0);
            ui.label(env!("CARGO_PKG_DESCRIPTION"));
            ui.add_space(8.0);
            let repo = env!("CARGO_PKG_REPOSITORY");
            // egui's own link opener is compiled out (eframe's default features are off).
            if ui.link(repo).on_hover_text("Open in the browser").clicked() {
                platform::open(repo);
            }
            ui.add_space(10.0);
            close = ui.button("Close").clicked();
        });
    });
    close || modal.should_close()
}
