use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::thread;

use eframe::egui;
use eframe::egui::{Align, Context, Layout, ProgressBar, RichText, ScrollArea};

use crate::engine::{self, ApplyOutcome, Plan};
use crate::model::{DriveInfo, Node, NodeKind};
use crate::rules::{self, Rule, RuleKind};
use crate::scan;
use crate::util::{self, human_size, percent};
use crate::winapi::{drive_space, is_elevated};

pub enum Msg {
    ScanProgress(u32),
    ScanDone(ScanDone),
    CleanProgress {
        done: usize,
        total: usize,
        label: String,
    },
    CleanDone(ApplyOutcome),
    RollbackDone {
        restored: u32,
        failed: Vec<(PathBuf, String)>,
    },
}

pub struct ScanDone {
    pub tree: Option<Node>,
    pub statuses: Vec<RuleStatus>,
}

#[derive(Clone)]
pub struct RuleStatus {
    pub id: String,
    pub size: u64,
    pub files: u32,
}

pub struct WinPurge {
    drives: Vec<DriveInfo>,
    selected_drive: usize,
    rules: Vec<Rule>,
    statuses: Vec<RuleStatus>,
    checked: HashSet<String>,
    admin: bool,
    quarantine_root: PathBuf,

    tree: Option<Node>,
    scan_depth: usize,
    scanning: bool,
    scanned: u32,

    cleaning: bool,
    clean_done: usize,
    clean_total: usize,
    clean_label: String,

    selected: Option<PathBuf>,
    expanded: HashSet<PathBuf>,
    confirm: Option<Plan>,
    batches: Vec<(String, u64)>,

    log: Vec<String>,
    rx: Option<Receiver<Msg>>,
}

impl WinPurge {
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        let drives = enumerate_drives();
        let rules = rules::all_rules();
        let statuses = statuses_for(&rules);
        let quarantine_root = util::quarantine_root();
        let batches = engine::list_batches(&quarantine_root);

        Self {
            drives,
            selected_drive: 0,
            rules,
            statuses,
            // Nothing is pre-selected: cleaning is always an explicit choice.
            checked: HashSet::new(),
            admin: is_elevated(),
            quarantine_root,
            tree: None,
            scan_depth: 4,
            scanning: false,
            scanned: 0,
            cleaning: false,
            clean_done: 0,
            clean_total: 0,
            clean_label: String::new(),
            selected: None,
            expanded: HashSet::new(),
            confirm: None,
            batches,
            log: vec!["Ready. Pick a drive and press Scan.".to_string()],
            rx: None,
        }
    }

    fn push_log(&mut self, line: impl Into<String>) {
        self.log.push(line.into());
        if self.log.len() > 200 {
            self.log.remove(0);
        }
    }

    fn start_scan(&mut self) {
        if self.scanning {
            return;
        }
        let Some(drive) = self.drives.get(self.selected_drive) else {
            self.push_log("No drive selected.");
            return;
        };
        let root = drive.path.clone();
        let rules = self.rules.clone();
        let depth = self.scan_depth;

        self.scanning = true;
        self.scanned = 0;
        self.tree = None;
        self.selected = None;
        self.expanded.clear();
        self.push_log(format!("Scanning {} (depth {depth}) ...", root.display()));

        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);

        thread::spawn(move || {
            let tx_progress = tx.clone();
            let tree = scan::scan_tree(&root, depth, move |count| {
                let _ = tx_progress.send(Msg::ScanProgress(count));
            });
            let _ = tx.send(Msg::ScanDone(ScanDone {
                tree: Some(tree),
                statuses: statuses_for(&rules),
            }));
        });
    }

    fn request_clean(&mut self) {
        if self.cleaning {
            return;
        }
        let selected: Vec<String> = self.checked.iter().cloned().collect();
        if selected.is_empty() {
            self.push_log("Nothing selected. Tick at least one category first.");
            return;
        }
        let plan = engine::build_plan(&self.rules, &selected);
        if plan.items.is_empty() {
            self.push_log("Selected categories are already empty.");
            return;
        }
        self.confirm = Some(plan);
    }

    fn start_clean(&mut self) {
        let Some(plan) = self.confirm.take() else {
            return;
        };
        if self.cleaning {
            return;
        }
        self.cleaning = true;
        self.clean_done = 0;
        self.clean_total = plan.items.len();
        self.clean_label = "starting ...".to_string();
        self.push_log(format!(
            "Cleaning {} item(s), about {} ...",
            plan.items.len(),
            human_size(plan.total)
        ));

        let quarantine_root = self.quarantine_root.clone();
        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);

        thread::spawn(move || {
            let tx_progress = tx.clone();
            let outcome = engine::apply(&plan, &quarantine_root, move |done, total, label| {
                let _ = tx_progress.send(Msg::CleanProgress {
                    done,
                    total,
                    label,
                });
            });
            let _ = tx.send(Msg::CleanDone(outcome));
        });
    }

    fn start_rollback(&mut self, id: String) {
        let quarantine_root = self.quarantine_root.clone();
        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        self.push_log(format!("Restoring batch {id} ..."));
        thread::spawn(move || {
            let (restored, failed) = engine::rollback(&quarantine_root, &id);
            let _ = tx.send(Msg::RollbackDone { restored, failed });
        });
    }

    fn handle(&mut self, msg: Msg) {
        match msg {
            Msg::ScanProgress(count) => self.scanned = count,
            Msg::ScanDone(done) => {
                self.scanning = false;
                if let Some(tree) = done.tree {
                    self.push_log(format!(
                        "Scan complete: {} directories, {} total.",
                        count_dirs(&tree),
                        human_size(tree.size)
                    ));
                    self.tree = Some(tree);
                }
                self.statuses = done.statuses;
            }
            Msg::CleanProgress {
                done,
                total,
                label,
            } => {
                self.clean_done = done;
                self.clean_total = total;
                self.clean_label = label;
            }
            Msg::CleanDone(outcome) => {
                self.cleaning = false;
                self.clean_label = String::new();
                self.push_log(format!(
                    "Moved {} item(s) to quarantine, freed {}.",
                    outcome.moved,
                    human_size(outcome.freed)
                ));
                for (path, error) in &outcome.failed {
                    self.push_log(format!("  ! {}: {error}", path.display()));
                }
                for (path, reason) in &outcome.skipped {
                    self.push_log(format!("  - {}: {reason}", path.display()));
                }
                if let Some(id) = &outcome.manifest_id {
                    self.push_log(format!("Quarantine batch: {id}"));
                }
                self.statuses = statuses_for(&self.rules);
                self.batches = engine::list_batches(&self.quarantine_root);
            }
            Msg::RollbackDone { restored, failed } => {
                self.push_log(format!("Restored {restored} item(s)."));
                for (path, error) in &failed {
                    self.push_log(format!("  ! {}: {error}", path.display()));
                }
                self.batches = engine::list_batches(&self.quarantine_root);
                self.statuses = statuses_for(&self.rules);
            }
        }
    }

    fn drain_messages(&mut self) {
        let rx = self.rx.take();
        let Some(rx) = rx else { return };
        let mut messages = Vec::new();
        while let Ok(msg) = rx.try_recv() {
            messages.push(msg);
        }
        for msg in messages {
            self.handle(msg);
        }
        self.rx = Some(rx);
    }

    fn top_panel(&mut self, ctx: &Context) {
        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("WinPurge");
                ui.separator();
                if self.admin {
                    ui.colored_label(egui::Color32::LIGHT_GREEN, "Elevated");
                } else {
                    ui.colored_label(
                        egui::Color32::YELLOW,
                        "Not elevated - system categories are disabled",
                    );
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.button("Refresh drives").clicked() {
                        self.drives = enumerate_drives();
                    }
                });
            });

            ui.horizontal(|ui| {
                ui.label("Drive");
                let drive_label = self
                    .drives
                    .get(self.selected_drive)
                    .map(|d| {
                        format!(
                            "{}:  {} free of {}",
                            d.letter,
                            human_size(d.free),
                            human_size(d.total)
                        )
                    })
                    .unwrap_or_else(|| "none".to_string());
                egui::ComboBox::from_id_salt("drive-selector")
                    .selected_text(drive_label)
                    .show_ui(ui, |ui| {
                        for (index, drive) in self.drives.iter().enumerate() {
                            ui.selectable_value(
                                &mut self.selected_drive,
                                index,
                                format!("{}:", drive.letter),
                            );
                        }
                    });

                ui.label("Depth");
                egui::ComboBox::from_id_salt("depth-selector")
                    .selected_text(self.scan_depth.to_string())
                    .show_ui(ui, |ui| {
                        for depth in 3..=6 {
                            ui.selectable_value(&mut self.scan_depth, depth, depth.to_string());
                        }
                    });

                ui.separator();
                ui.add_enabled_ui(!self.scanning && !self.cleaning, |ui| {
                    if ui.button("Scan").clicked() {
                        self.start_scan();
                    }
                });
                ui.add_enabled_ui(!self.cleaning && !self.scanning && self.confirm.is_none(), |ui| {
                    if ui.button("Clean selected").clicked() {
                        self.request_clean();
                    }
                });
                ui.add_enabled_ui(!self.cleaning && !self.scanning, |ui| {
                    if ui.button("Re-scan sizes").clicked() {
                        self.statuses = statuses_for(&self.rules);
                        self.push_log("Category sizes refreshed.");
                    }
                });

                if self.scanning {
                    ui.spinner();
                    ui.label(format!("scanned {}+ dirs", self.scanned));
                }
            });

            if self.cleaning {
                let fraction = if self.clean_total == 0 {
                    0.0
                } else {
                    self.clean_done as f32 / self.clean_total as f32
                };
                ui.add(ProgressBar::new(fraction).show_percentage());
                ui.label(
                    RichText::new(format!(
                        "{}/{}  {}",
                        self.clean_done,
                        self.clean_total,
                        self.clean_label
                    ))
                    .small(),
                );
            }
        });
    }

    fn rules_panel(&mut self, ctx: &Context) {
        egui::SidePanel::left("rules")
            .resizable(true)
            .default_width(340.0)
            .width_range(260.0..=520.0)
            .show(ctx, |ui| {
                ui.heading("Junk categories");
                ui.separator();

                let busy = self.scanning || self.cleaning;
                ui.set_enabled(!busy);
                ScrollArea::vertical()
                    .auto_shrink(false)
                    .max_height(ui.available_height() * 0.55)
                    .show(ui, |ui| {
                        for rule in &self.rules {
                            let status = self
                                .statuses
                                .iter()
                                .find(|s| s.id == rule.id)
                                .cloned()
                                .unwrap_or(RuleStatus {
                                    id: rule.id.to_string(),
                                    size: 0,
                                    files: 0,
                                });

                            let blocked = rule.needs_admin && !self.admin;
                            let label = format!(
                                "{}   {}",
                                rule.name,
                                if status.size > 0 {
                                    human_size(status.size)
                                } else {
                                    "-".to_string()
                                }
                            );

                            ui.set_enabled(!blocked);
                            let mut on = self.checked.contains(rule.id);
                            if ui.checkbox(&mut on, label.as_str()).changed() {
                                if on {
                                    self.checked.insert(rule.id.to_string());
                                } else {
                                    self.checked.remove(rule.id);
                                }
                            }

                            ui.horizontal(|ui| {
                                ui.add_space(10.0);
                                ui.vertical(|ui| {
                                    ui.label(
                                        RichText::new(rule.description)
                                            .small()
                                            .weak(),
                                    );
                                    if blocked {
                                        ui.colored_label(
                                            egui::Color32::YELLOW,
                                            RichText::new("needs administrator").small(),
                                        );
                                    } else if status.size > 0 {
                                        ui.label(
                                            RichText::new(format!(
                                                "{} file(s) in scope",
                                                status.files
                                            ))
                                            .small()
                                            .weak(),
                                        );
                                    }
                                });
                            });
                            ui.separator();
                        }
                    });
                ui.set_enabled(true);

                ui.heading("Quarantine");
                ui.separator();
                if self.batches.is_empty() {
                    ui.label(RichText::new("empty").weak());
                } else {
                    ScrollArea::vertical()
                        .auto_shrink(false)
                        .max_height(ui.available_height())
                        .show(ui, |ui| {
                            for (id, size) in self.batches.clone() {
                                ui.horizontal(|ui| {
                                    ui.label(id.as_str());
                                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                        ui.label(format!("{} ", human_size(size)));
                                        if ui.button("Restore").clicked() {
                                            self.start_rollback(id.clone());
                                        }
                                    });
                                });
                            }
                        });
                }
            });
    }

    fn tree_panel(&mut self, ctx: &Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            let drive_total = self
                .drives
                .get(self.selected_drive)
                .map(|d| d.total)
                .unwrap_or(0);

            match &self.tree {
                None => {
                    ui.vertical_centered(|ui| {
                        ui.add_space(ui.available_height() * 0.3);
                        ui.heading("No scan yet");
                        ui.label("Press Scan to analyse the selected drive.");
                    });
                }
                Some(tree) => {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(format!(
                                "{}   total {}",
                                tree.path.display(),
                                human_size(tree.size)
                            ))
                            .strong(),
                        );
                        ui.separator();
                        if let Some(selected) = &self.selected {
                            ui.label(
                                RichText::new(format!(
                                    "{}  ({}, {} of drive)",
                                    selected.display(),
                                    human_size(
                                        find_size(tree, selected).unwrap_or(0)
                                    ),
                                    format!(
                                        "{:.1}%",
                                        percent(
                                            find_size(tree, selected).unwrap_or(0),
                                            drive_total
                                        )
                                    )
                                ))
                                .small(),
                            );
                        }
                    });
                    ui.separator();

                    let mut selected_mut = self.selected.clone();
                    // Taken out of `self` so the scroll closure does not borrow
                    // the whole app while `tree` still borrows `self.tree`.
                    let mut expanded = std::mem::take(&mut self.expanded);
                    ScrollArea::vertical().show(ui, |ui| {
                        for child in &tree.children {
                            show_node(
                                ui,
                                child,
                                0,
                                tree.size,
                                &mut selected_mut,
                                &mut expanded,
                            );
                        }
                    });
                    self.expanded = expanded;
                    if selected_mut != self.selected {
                        self.selected = selected_mut;
                    }
                }
            }
        });
    }

    fn log_panel(&mut self, ctx: &Context) {
        egui::TopBottomPanel::bottom("log")
            .resizable(false)
            .default_height(140.0)
            .show(ctx, |ui| {
                ui.heading(RichText::new("Log").small());
                ScrollArea::vertical().show(ui, |ui| {
                    for line in &self.log {
                        ui.label(RichText::new(line.as_str()).small().monospace());
                    }
                    ui.scroll_to_cursor(Some(Align::BOTTOM));
                });
            });
    }

    fn confirm_dialog(&mut self, ctx: &Context) {
        let Some(plan) = self.confirm.clone() else {
            return;
        };
        egui::Window::new("Confirm cleanup")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(format!(
                    "Move {} item(s) to quarantine, freeing about {}?",
                    plan.items.len(),
                    human_size(plan.total)
                ));
                ui.label(
                    RichText::new(
                        "Files are moved, not deleted. You can restore them from the Quarantine panel.",
                    )
                    .small()
                    .weak(),
                );
                ui.separator();
                ScrollArea::vertical()
                    .max_height(220.0)
                    .show(ui, |ui| {
                        for item in &plan.items {
                            ui.label(
                                RichText::new(format!(
                                    "{}  {}",
                                    item.path.display(),
                                    human_size(item.size)
                                ))
                                .small()
                                .monospace(),
                            );
                        }
                    });
                ui.separator();
                ui.horizontal(|ui| {
                    ui.add_enabled_ui(!self.cleaning, |ui| {
                        if ui.button("Cancel").clicked() {
                            self.confirm = None;
                        }
                    });
                    ui.add_enabled_ui(!self.cleaning, |ui| {
                        if ui.button(RichText::new("Move to quarantine").strong()).clicked() {
                            self.start_clean();
                        }
                    });
                });
            });
    }
}

impl eframe::App for WinPurge {
    fn update(&mut self, ctx: &Context, _frame: &eframe::Frame) {
        self.drain_messages();
        self.top_panel(ctx);
        self.rules_panel(ctx);
        self.tree_panel(ctx);
        self.log_panel(ctx);
        self.confirm_dialog(ctx);
    }
}

fn show_node(
    ui: &mut egui::Ui,
    node: &Node,
    depth: usize,
    parent_size: u64,
    selected: &mut Option<PathBuf>,
    expanded: &mut HashSet<PathBuf>,
) {
    let is_dir = node.is_dir();
    let open = expanded.contains(&node.path);

    ui.horizontal(|ui| {
        ui.add_space(depth as f32 * 14.0);

        if is_dir {
            let arrow = if open { "\u{25be}" } else { "\u{25b8}" };
            if ui.small_button(arrow).clicked() {
                if open {
                    expanded.remove(&node.path);
                } else {
                    expanded.insert(node.path.clone());
                }
            }
        } else {
            ui.add_space(14.0);
        }

        let is_selected = selected.as_deref() == Some(node.path.as_path());
        let name = match node.kind {
            NodeKind::Directory => node.name.clone(),
            NodeKind::LooseFiles(count) => {
                format!("{} loose file(s)", count)
            }
            NodeKind::Inaccessible => format!("{} (inaccessible)", node.name),
        };

        if ui.selectable_label(is_selected, name).clicked() && is_dir {
            *selected = Some(node.path.clone());
            expanded.insert(node.path.clone());
        }

        ui.add_sized(
            [120.0, 12.0],
            ProgressBar::new(percent(node.size, parent_size) / 100.0),
        );

        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(human_size(node.size));
        });
    });

    if is_dir && open {
        for child in &node.children {
            show_node(ui, child, depth + 1, node.size, selected, expanded);
        }
    }
}

fn enumerate_drives() -> Vec<DriveInfo> {
    let mut out = Vec::new();
    for path in util::drives() {
        let letter = path
            .to_string_lossy()
            .chars()
            .next()
            .unwrap_or('?');
        let (total, free) = drive_space(&path)
            .map(|space| (space.total, space.free))
            .unwrap_or((0, 0));
        out.push(DriveInfo {
            letter,
            path,
            total,
            free,
        });
    }
    out
}

fn statuses_for(rules: &[Rule]) -> Vec<RuleStatus> {
    rules
        .iter()
        .map(|rule| {
            let (size, files) = match &rule.kind {
                RuleKind::Dirs(dirs) => scan::total_size(dirs),
                RuleKind::Files { dirs, pred } => scan::matching_files_size(dirs, pred),
            };
            RuleStatus {
                id: rule.id.to_string(),
                size,
                files,
            }
        })
        .collect()
}

fn count_dirs(node: &Node) -> u32 {
    let mut count = if node.is_dir() { 1 } else { 0 };
    for child in &node.children {
        count = count.saturating_add(count_dirs(child));
    }
    count
}

fn find_size(node: &Node, target: &PathBuf) -> Option<u64> {
    if &node.path == target {
        return Some(node.size);
    }
    for child in &node.children {
        if let Some(size) = find_size(child, target) {
            return Some(size);
        }
    }
    None
}
