#![windows_subsystem = "windows"]

use std::io::{BufRead, BufReader};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use eframe::egui;
use serde::{Deserialize, Serialize};

static MAIN_HWND: AtomicIsize = AtomicIsize::new(0);
static TRAY_READY: AtomicBool = AtomicBool::new(false);

#[derive(Serialize, Deserialize, Clone)]
struct TabConfig {
    path: String,
    custom_name: String,
}

#[derive(Serialize, Deserialize, Default)]
struct Config {
    tabs: Vec<TabConfig>,
}

impl Config {
    fn path() -> PathBuf {
        let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("."));
        exe.parent()
            .unwrap_or(Path::new("."))
            .join("cmdrunner.toml")
    }

    fn load() -> Self {
        let p = Self::path();
        std::fs::read_to_string(&p)
            .ok()
            .and_then(|s| toml::from_str(&s).ok())
            .unwrap_or_default()
    }

    fn save(&self) {
        if let Ok(s) = toml::to_string_pretty(self) {
            let _ = std::fs::write(Self::path(), s);
        }
    }
}

fn create_egui_icon() -> egui::IconData {
    let size = 32;
    let mut rgba = vec![0u8; size * size * 4];
    for y in 0..size {
        for x in 0..size {
            let idx = (y * size + x) * 4;
            rgba[idx] = 0x33;
            rgba[idx + 1] = 0x66;
            rgba[idx + 2] = 0xCC;
            rgba[idx + 3] = 255;
        }
    }
    let c_pixels: &[(i32, i32)] = &[
        (8,4),(9,4),(10,4),(11,4),(12,4),(13,4),
        (6,5),(7,5),
        (5,6),(6,6),
        (5,7),(6,7),
        (5,8),(6,8),
        (5,9),(6,9),
        (5,10),(6,10),
        (5,11),(6,11),
        (5,12),(6,12),
        (5,13),(6,13),
        (5,14),(6,14),
        (5,15),(6,15),
        (5,16),(6,16),
        (6,17),(7,17),
        (8,18),(9,18),(10,18),(11,18),(12,18),(13,18),
    ];
    for &(x, y) in c_pixels {
        for dy in 0..2i32 {
            for dx in 0..2i32 {
                let px = x + dx;
                let py = y + dy;
                if px >= 0 && py >= 0 && (px as usize) < size && (py as usize) < size {
                    let idx = ((py as usize) * size + (px as usize)) * 4;
                    rgba[idx] = 255;
                    rgba[idx + 1] = 255;
                    rgba[idx + 2] = 255;
                    rgba[idx + 3] = 255;
                }
            }
        }
    }
    egui::IconData {
        rgba,
        width: size as u32,
        height: size as u32,
    }
}

fn main() -> eframe::Result {
    std::thread::spawn(setup_tray);

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("CMD Runner v0.7.0")
            .with_inner_size([900.0, 650.0])
            .with_min_inner_size([600.0, 400.0])
            .with_icon(create_egui_icon()),
        ..Default::default()
    };

    eframe::run_native(
        "CMD Runner",
        options,
        Box::new(|cc| {
            setup_theme(&cc.egui_ctx);
            Ok(Box::new(App::new()))
        }),
    )
}

fn setup_theme(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "msyh".to_owned(),
        egui::FontData::from_static(include_bytes!("C:\\Windows\\Fonts\\msyh.ttc")),
    );
    if let Some(family) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
        family.insert(0, "msyh".to_owned());
    }
    if let Some(family) = fonts.families.get_mut(&egui::FontFamily::Monospace) {
        family.insert(0, "msyh".to_owned());
    }
    ctx.set_fonts(fonts);

    let mut visuals = egui::Visuals::dark();
    visuals.override_text_color = Some(egui::Color32::from_rgb(220, 220, 220));
    visuals.panel_fill = egui::Color32::from_rgb(25, 25, 30);
    visuals.window_fill = egui::Color32::from_rgb(35, 35, 40);
    visuals.extreme_bg_color = egui::Color32::from_rgb(30, 30, 35);
    visuals.faint_bg_color = egui::Color32::from_rgb(30, 30, 35);
    visuals.widgets.noninteractive.bg_fill = egui::Color32::from_rgb(35, 35, 40);
    visuals.widgets.inactive.bg_fill = egui::Color32::from_rgb(40, 40, 48);
    visuals.widgets.hovered.bg_fill = egui::Color32::from_rgb(50, 50, 58);
    visuals.widgets.active.bg_fill = egui::Color32::from_rgb(55, 55, 65);
    visuals.selection.bg_fill = egui::Color32::from_rgb(50, 80, 120);
    ctx.set_visuals(visuals);
}

struct BatTab {
    path: String,
    custom_name: String,
    output: Arc<Mutex<String>>,
    running: bool,
    done: bool,
    child: Option<Child>,
    done_flag: Arc<AtomicBool>,
}

struct App {
    tabs: Vec<BatTab>,
    active_tab: Option<usize>,
    edit_buffers: std::collections::HashMap<usize, String>,
    confirm_delete: Option<usize>,
    detect_links: bool,
}

impl App {
    fn new() -> Self {
        let config = Config::load();
        let mut app = Self {
            tabs: Vec::new(),
            active_tab: None,
            edit_buffers: std::collections::HashMap::new(),
            confirm_delete: None,
            detect_links: true,
        };
        for tc in config.tabs {
            let path = PathBuf::from(&tc.path);
            if path.exists() {
                let name = path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                app.tabs.push(BatTab {
                    path: tc.path,
                    custom_name: if tc.custom_name.is_empty() {
                        name
                    } else {
                        tc.custom_name
                    },
                    output: Arc::new(Mutex::new(String::new())),
                    running: false,
                    done: false,
                    child: None,
                    done_flag: Arc::new(AtomicBool::new(false)),
                });
            }
        }
        app
    }

    fn save_config(&self) {
        let config = Config {
            tabs: self
                .tabs
                .iter()
                .map(|t| TabConfig {
                    path: t.path.clone(),
                    custom_name: t.custom_name.clone(),
                })
                .collect(),
        };
        config.save();
    }

    fn add_file(&mut self, path: PathBuf) {
        let path_str = path.to_string_lossy().to_string();
        let lower = path_str.to_lowercase();
        if !lower.ends_with(".bat") && !lower.ends_with(".cmd") {
            return;
        }
        if self.tabs.iter().any(|t| t.path == path_str) {
            let idx = self.tabs.iter().position(|t| t.path == path_str);
            self.active_tab = idx;
            return;
        }
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let idx = self.tabs.len();
        self.tabs.push(BatTab {
            path: path_str,
            custom_name: name,
            output: Arc::new(Mutex::new(String::new())),
            running: false,
            done: false,
            child: None,
            done_flag: Arc::new(AtomicBool::new(false)),
        });
        self.active_tab = Some(idx);
        self.save_config();
    }

    fn hide_to_tray(&self) {
        let hwnd = MAIN_HWND.load(Ordering::SeqCst) as *mut std::ffi::c_void;
        if !hwnd.is_null() {
            unsafe {
                windows_sys::Win32::UI::WindowsAndMessaging::ShowWindow(
                    hwnd,
                    windows_sys::Win32::UI::WindowsAndMessaging::SW_HIDE,
                );
            }
        }
    }

    fn run_tab(&mut self, idx: usize) {
        if idx >= self.tabs.len() {
            return;
        }
        self.stop_tab(idx);

        let tab = &mut self.tabs[idx];
        let path = tab.path.clone();
        let dir = std::path::Path::new(&path)
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_default();

        {
            let mut out = tab.output.lock().unwrap();
            out.clear();
            out.push_str(&format!("> 运行: {}\r\n", tab.custom_name));
            out.push_str(&format!("> 路径: {}\r\n", path));
            out.push_str(&"─".repeat(50));
            out.push_str("\r\n");
        }
        tab.running = true;
        tab.done = false;

        let done_flag = Arc::new(AtomicBool::new(false));
        tab.done_flag = done_flag.clone();
        let output = tab.output.clone();

        match Command::new("cmd.exe")
            .args(["/C", &path])
            .current_dir(&dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .creation_flags(0x08000000)
            .spawn()
        {
            Ok(mut child) => {
                let stdout = child.stdout.take().unwrap();
                let stderr = child.stderr.take().unwrap();

                std::thread::spawn(move || {
                    let reader = BufReader::new(stdout);
                    for line in reader.lines() {
                        if let Ok(l) = line {
                            let mut out = output.lock().unwrap();
                            out.push_str(&l);
                            out.push_str("\r\n");
                            if out.len() > 200000 {
                                let cut = out.len() - 160000;
                                *out = out[cut..].to_string();
                            }
                        }
                    }
                    let reader2 = BufReader::new(stderr);
                    for line in reader2.lines() {
                        if let Ok(l) = line {
                            let mut out = output.lock().unwrap();
                            out.push_str(&l);
                            out.push_str("\r\n");
                        }
                    }
                    done_flag.store(true, Ordering::SeqCst);
                });

                self.tabs[idx].child = Some(child);
            }
            Err(e) => {
                self.tabs[idx]
                    .output
                    .lock()
                    .unwrap()
                    .push_str(&format!("[ERROR] {}\r\n", e));
                self.tabs[idx].running = false;
                self.tabs[idx].done = true;
            }
        }
    }

    fn stop_tab(&mut self, idx: usize) {
        if idx >= self.tabs.len() {
            return;
        }
        if let Some(mut child) = self.tabs[idx].child.take() {
            let _ = child.kill();
        }
        self.tabs[idx].running = false;
    }

    fn close_tab(&mut self, idx: usize) {
        if idx >= self.tabs.len() {
            return;
        }
        self.stop_tab(idx);
        self.tabs.remove(idx);
        if self.tabs.is_empty() {
            self.active_tab = None;
        } else if self.active_tab == Some(idx) {
            self.active_tab = Some(idx.min(self.tabs.len() - 1));
        } else if let Some(a) = self.active_tab {
            if a > idx {
                self.active_tab = Some(a - 1);
            }
        }
        self.save_config();
    }

    fn poll_all(&mut self) {
        for tab in &mut self.tabs {
            if tab.running && tab.done_flag.load(Ordering::SeqCst) {
                tab.running = false;
                tab.done = true;
                tab.output.lock().unwrap().push_str(&"─".repeat(50));
                tab.output.lock().unwrap().push_str("\r\n");
                tab.output.lock().unwrap().push_str("> ✅ 执行完毕\r\n");
                let _ = tab.child.take();
            }
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .filter_map(|f| f.path.clone())
                .collect()
        });
        for path in dropped {
            self.add_file(path);
        }

        if ctx.input(|i| i.viewport().close_requested()) {
            self.hide_to_tray();
            return;
        }

        if !TRAY_READY.load(Ordering::SeqCst) && MAIN_HWND.load(Ordering::SeqCst) == 0 {
            find_my_hwnd();
        }

        self.poll_all();
        ctx.request_repaint_after(Duration::from_millis(100));

        let mut close_idx: Option<usize> = None;
        let mut run_idx: Option<usize> = None;
        let mut stop_idx: Option<usize> = None;
        let mut rename_idx: Option<usize> = None;
        let mut new_name = String::new();

        egui::TopBottomPanel::top("top")
            .frame(egui::Frame::none().fill(egui::Color32::from_rgb(255, 255, 255)).inner_margin(4.0))
            .show(ctx, |ui| {
                ui.visuals_mut().override_text_color = Some(egui::Color32::from_rgb(40, 40, 40));
                ui.horizontal(|ui| {
                    ui.label("📋");
                ui.separator();
                if ui.button("添加文件").clicked() {
                    let paths = rfd::FileDialog::new()
                        .add_filter("Batch Files", &["bat", "cmd"])
                        .pick_files();
                    if let Some(files) = paths {
                        for p in files {
                            self.add_file(p);
                        }
                    }
                }
                if ui.button("导入配置").clicked() {
                    if let Some(p) = rfd::FileDialog::new()
                        .add_filter("TOML", &["toml"])
                        .pick_file()
                    {
                        if let Ok(s) = std::fs::read_to_string(&p) {
                            if let Ok(cfg) = toml::from_str::<Config>(&s) {
                                for tc in cfg.tabs {
                                    let path = PathBuf::from(&tc.path);
                                    if path.exists()
                                        && !self.tabs.iter().any(|t| t.path == tc.path)
                                    {
                                        let name = path
                                            .file_name()
                                            .unwrap_or_default()
                                            .to_string_lossy()
                                            .to_string();
                                        self.tabs.push(BatTab {
                                            path: tc.path,
                                            custom_name: if tc.custom_name.is_empty() {
                                                name
                                            } else {
                                                tc.custom_name
                                            },
                                            output: Arc::new(Mutex::new(String::new())),
                                            running: false,
                                            done: false,
                                            child: None,
                                            done_flag: Arc::new(AtomicBool::new(false)),
                                        });
                                    }
                                }
                                self.save_config();
                            }
                        }
                    }
                }
                if ui.button("导出配置").clicked() {
                    if let Some(p) = rfd::FileDialog::new()
                        .add_filter("TOML", &["toml"])
                        .save_file()
                    {
                        self.save_config();
                        let cfg_path = Config::path();
                        if cfg_path.exists() {
                            let _ = std::fs::copy(&cfg_path, &p);
                        }
                    }
                }
                if ui.button("全部停止").clicked() {
                    for i in 0..self.tabs.len() {
                        self.stop_tab(i);
                    }
                }
                if ui.button("清空全部").clicked() {
                    for i in (0..self.tabs.len()).rev() {
                        self.close_tab(i);
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("最小化到托盘").clicked() {
                        self.hide_to_tray();
                    }
                    ui.separator();
                    ui.checkbox(&mut self.detect_links, "链接检测");
                });
            });
        });

        egui::TopBottomPanel::bottom("bottom")
            .frame(egui::Frame::none().fill(egui::Color32::from_rgb(255, 255, 255)).inner_margin(4.0))
            .show(ctx, |ui| {
                ui.visuals_mut().override_text_color = Some(egui::Color32::from_rgb(40, 40, 40));
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("拖拽 .bat / .cmd 文件到此窗口")
                            .color(egui::Color32::from_rgb(120, 120, 120))
                            .size(11.0),
                    );
                    let running_count = self.tabs.iter().filter(|t| t.running).count();
                    if running_count > 0 {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(
                                egui::RichText::new(format!("{} 个运行中", running_count))
                                    .color(egui::Color32::from_rgb(180, 180, 60))
                                    .size(11.0),
                            );
                        });
                    }
                });
        });

        let tab_card_height = 80.0;
        let cols = 4;
        let tab_count = self.tabs.len();
        let rows = (tab_count + cols - 1) / cols;
        let cards_height = (rows as f32 * (tab_card_height + 8.0) + 8.0).max(40.0);
        let available_h = ctx.available_rect().height();
        let cards_area_h = (cards_height + 16.0).min(available_h * 0.45);

        egui::TopBottomPanel::top("cards")
            .frame(egui::Frame::none().fill(egui::Color32::from_rgb(255, 255, 255)).inner_margin(4.0))
            .show(ctx, |ui| {
                ui.visuals_mut().override_text_color = Some(egui::Color32::from_rgb(40, 40, 40));
                ui.add_space(4.0);
            egui::ScrollArea::vertical()
                .max_height(cards_area_h)
                .show(ui, |ui| {
                    if self.tabs.is_empty() {
                        ui.vertical_centered(|ui| {
                            ui.add_space(16.0);
                            ui.label(
                                egui::RichText::new("拖入 .bat 文件或点击「添加文件」")
                                    .size(13.0)
                                    .color(ui.visuals().weak_text_color()),
                            );
                            ui.add_space(8.0);
                        });
                    } else {
                        let card_width = (ui.available_width() / cols as f32) - 8.0;
                        let mut rows_iter = self.tabs.chunks(cols as usize);
                        let mut row_idx = 0;
                        loop {
                            let row_tabs: Vec<usize> = rows_iter
                                .next()
                                .map(|chunk| {
                                    (0..chunk.len())
                                        .map(|i| row_idx * cols as usize + i)
                                        .collect()
                                })
                                .unwrap_or_default();
                            if row_tabs.is_empty() {
                                break;
                            }
                            ui.horizontal(|ui| {
                                for &i in &row_tabs {
                                    if i >= self.tabs.len() {
                                        continue;
                                    }
                                    let is_active = self.active_tab == Some(i);
                                    let tab = &self.tabs[i];
                                    let card_color = if is_active {
                                        egui::Color32::from_rgb(45, 55, 72)
                                    } else {
                                        egui::Color32::from_rgb(30, 35, 45)
                                    };
                                    let border_color = if tab.running {
                                        egui::Color32::from_rgb(72, 180, 100)
                                    } else if is_active {
                                        egui::Color32::from_rgb(100, 140, 200)
                                    } else {
                                        egui::Color32::from_rgb(60, 65, 75)
                                    };

                                    let frame = egui::Frame::none()
                                        .fill(card_color)
                                        .rounding(8.0)
                                        .inner_margin(8.0)
                                        .stroke(egui::Stroke::new(1.5, border_color));

                                    let resp = ui.allocate_ui_with_layout(
                                        egui::vec2(card_width, tab_card_height),
                                        egui::Layout::top_down(egui::Align::LEFT),
                                        |ui| {
                                            frame.show(ui, |ui| {
                                                ui.set_min_width(card_width - 18.0);

                                                ui.horizontal(|ui| {
                                                    let buf = self
                                                        .edit_buffers
                                                        .entry(i)
                                                        .or_insert_with(|| {
                                                            tab.custom_name.clone()
                                                        })
                                                        .clone();
                                                    let mut buf = buf;
                                                    let name_edit = egui::TextEdit::singleline(
                                                        &mut buf,
                                                    )
                                                    .desired_width(card_width - 80.0)
                                                    .font(egui::TextStyle::Small);
                                                    let resp = ui.add(name_edit);
                                                    if resp.changed() {
                                                        self.edit_buffers.insert(i, buf.clone());
                                                        rename_idx = Some(i);
                                                        new_name = buf;
                                                    }

                                                    if tab.running {
                                                        let btn = ui.add(
                                                            egui::Button::new(
                                                                egui::RichText::new("⏹")
                                                                    .color(
                                                                        egui::Color32::from_rgb(
                                                                            220, 80, 80,
                                                                        ),
                                                                    ),
                                                            )
                                                            .fill(egui::Color32::from_rgb(50, 40, 40)),
                                                        );
                                                        if btn.clicked() {
                                                            stop_idx = Some(i);
                                                        }
                                                    } else {
                                                        let btn = ui.add(
                                                            egui::Button::new(
                                                                egui::RichText::new("▶")
                                                                    .color(
                                                                        egui::Color32::from_rgb(
                                                                            80, 200, 120,
                                                                        ),
                                                                    ),
                                                            )
                                                            .fill(egui::Color32::from_rgb(35, 50, 40)),
                                                        );
                                                        if btn.clicked() {
                                                            run_idx = Some(i);
                                                        }
                                                    }

                                                    let close_btn = ui.add(
                                                        egui::Button::new(
                                                            egui::RichText::new("X")
                                                                .color(
                                                                    egui::Color32::from_rgb(
                                                                        160, 160, 160,
                                                                    ),
                                                                )
                                                                .size(10.0),
                                                        )
                                                        .fill(egui::Color32::TRANSPARENT),
                                                    );
                                                    if close_btn.clicked() {
                                                        self.confirm_delete = Some(i);
                                                    }
                                                });

                                                let status_color = if tab.running {
                                                    egui::Color32::from_rgb(72, 180, 100)
                                                } else if tab.done {
                                                    egui::Color32::from_rgb(100, 160, 220)
                                                } else {
                                                    ui.visuals().weak_text_color()
                                                };
                                                let status_text = if tab.running {
                                                    "● 运行中"
                                                } else if tab.done {
                                                    "● 完成"
                                                } else {
                                                    "○ 就绪"
                                                };
                                                ui.label(
                                                    egui::RichText::new(status_text)
                                                        .size(10.0)
                                                        .color(status_color),
                                                );

                                                ui.horizontal(|ui| {
                                                    let short_path = std::path::Path::new(&tab.path)
                                                        .parent()
                                                        .and_then(|p| p.file_name())
                                                        .unwrap_or_default()
                                                        .to_string_lossy()
                                                        .to_string();
                                                    ui.label(
                                                        egui::RichText::new(&short_path)
                                                            .size(9.0)
                                                            .color(egui::Color32::from_rgb(120, 120, 130)),
                                                    );
                                                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                                        let folder_btn = ui.add(
                                                            egui::Button::new(
                                                                egui::RichText::new("📂 打开")
                                                                    .size(10.0)
                                                                    .color(egui::Color32::from_rgb(100, 160, 220)),
                                                            )
                                                            .fill(egui::Color32::TRANSPARENT)
                                                            .rounding(4.0),
                                                        );
                                                        if folder_btn.clicked() {
                                                            if let Some(dir) = std::path::Path::new(&tab.path).parent() {
                                                                let _ = std::process::Command::new("explorer")
                                                                    .arg(dir)
                                                                    .spawn();
                                                            }
                                                        }
                                                    });
                                                });
                                            });
                                        },
                                    );

                                    if resp.response.interact(egui::Sense::click()).clicked() {
                                        self.active_tab = Some(i);
                                    }
                                }
                            });
                            row_idx += 1;
                        }
                    }
                });
            ui.add_space(4.0);
        });

        egui::CentralPanel::default()
            .frame(
                egui::Frame::none()
                    .fill(egui::Color32::from_rgb(30, 30, 35))
                    .inner_margin(egui::Margin::symmetric(8.0, 4.0))
                    .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(200, 200, 200))),
            )
            .show(ctx, |ui| {
                if let Some(idx) = self.active_tab {
                    if idx < self.tabs.len() {
                        let tab = &self.tabs[idx];
                        let status = if tab.running {
                            format!("▶ {} - 运行中...", tab.custom_name)
                        } else if tab.done {
                            format!("✅ {} - 完成", tab.custom_name)
                        } else {
                            format!("📄 {} - 就绪", tab.custom_name)
                        };
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(&status)
                                    .strong()
                                    .color(egui::Color32::from_rgb(220, 220, 220)),
                            );
                            ui.separator();
                            ui.label(
                                egui::RichText::new(&tab.path)
                                    .size(11.0)
                                    .color(egui::Color32::from_rgb(150, 150, 160)),
                            );
                        });
                        ui.separator();

                        let output = tab.output.lock().unwrap().clone();
                        ui.style_mut().visuals.override_text_color = Some(egui::Color32::from_rgb(220, 220, 220));
                        egui::ScrollArea::vertical()
                            .stick_to_bottom(true)
                            .show(ui, |ui| {
                                let lines: Vec<&str> = output.lines().collect();
                                let total = lines.len();
                                let start = if total > 500 { total - 500 } else { 0 };
                                let detect = self.detect_links;
                                for i in start..total {
                                    let line = lines[i];
                                    let urls: Vec<&str> = if detect {
                                        line.match_indices("http")
                                            .filter_map(|(pos, _)| {
                                                let rest = &line[pos..];
                                                if rest.starts_with("http://") || rest.starts_with("https://") {
                                                    let end = rest.find(|c: char| c.is_whitespace()).unwrap_or(rest.len());
                                                    Some(&rest[..end])
                                                } else {
                                                    None
                                                }
                                            })
                                            .collect()
                                    } else {
                                        Vec::new()
                                    };
                                    if urls.is_empty() {
                                        ui.label(
                                            egui::RichText::new(line)
                                                .monospace()
                                                .color(egui::Color32::from_rgb(220, 220, 220)),
                                        );
                                    } else {
                                        ui.horizontal(|ui| {
                                            ui.label(
                                                egui::RichText::new(line)
                                                    .monospace()
                                                    .color(egui::Color32::from_rgb(220, 220, 220)),
                                            );
                                            for url in &urls {
                                                let url_str = url.to_string();
                                                let btn = ui.add(
                                                    egui::Button::new(
                                                        egui::RichText::new("🔗 转到")
                                                            .size(11.0)
                                                            .color(egui::Color32::from_rgb(255, 255, 255)),
                                                    )
                                                    .fill(egui::Color32::from_rgb(50, 100, 180))
                                                    .rounding(4.0),
                                                );
                                                if btn.clicked() {
                                                    let _ = std::process::Command::new("cmd")
                                                        .args(["/C", "start", &url_str])
                                                        .spawn();
                                                }
                                            }
                                        });
                                    }
                                }
                            });
                    }
                } else {
                    ui.vertical_centered(|ui| {
                        ui.add_space(60.0);
                        ui.label(
                            egui::RichText::new("📋 CMD Runner")
                                .size(24.0)
                                .color(egui::Color32::from_rgb(120, 120, 130)),
                        );
                        ui.add_space(10.0);
                        ui.label(
                            egui::RichText::new("拖入 .bat / .cmd 文件开始使用")
                                .size(14.0)
                                .color(egui::Color32::from_rgb(100, 100, 110)),
                        );
                    });
                }
        });

        if let Some(idx) = self.confirm_delete {
            if idx < self.tabs.len() {
                let name = self.tabs[idx].custom_name.clone();
                egui::Window::new("确认删除")
                    .collapsible(false)
                    .resizable(false)
                    .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                    .show(ctx, |ui| {
                        ui.label(format!("确定要删除标签「{}」吗？", name));
                        ui.horizontal(|ui| {
                            if ui.button("取消").clicked() {
                                self.confirm_delete = None;
                            }
                            if ui.button("删除").clicked() {
                                close_idx = Some(idx);
                                self.confirm_delete = None;
                            }
                        });
                    });
            }
        }

        if let Some(i) = close_idx {
            self.close_tab(i);
        }
        if let Some(i) = run_idx {
            self.active_tab = Some(i);
            self.run_tab(i);
        }
        if let Some(i) = stop_idx {
            self.stop_tab(i);
        }
        if let Some(idx) = rename_idx {
            if !new_name.is_empty() && idx < self.tabs.len() {
                self.tabs[idx].custom_name = new_name;
                self.save_config();
            }
        }
    }
}

fn find_my_hwnd() {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumWindows, FindWindowW, GetWindowTextLengthW,
        GetWindowThreadProcessId, IsWindowVisible,
    };

    let our_pid = std::process::id();

    unsafe extern "system" fn enum_cb(
        hwnd: windows_sys::Win32::Foundation::HWND,
        lparam: isize,
    ) -> i32 {
        if IsWindowVisible(hwnd) == 0 {
            return 1;
        }
        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, &mut pid);
        if pid != lparam as u32 {
            return 1;
        }
        let len = GetWindowTextLengthW(hwnd);
        if len > 0 {
            MAIN_HWND.store(hwnd as isize, Ordering::SeqCst);
            TRAY_READY.store(true, Ordering::SeqCst);
            return 0;
        }
        1
    }

    unsafe {
        EnumWindows(Some(enum_cb), our_pid as isize);
    }

    if MAIN_HWND.load(Ordering::SeqCst) == 0 {
        let title: Vec<u16> = "CMD Runner v0.6.0\0".encode_utf16().collect();
        unsafe {
            let hwnd = FindWindowW(std::ptr::null(), title.as_ptr());
            if !hwnd.is_null() {
                MAIN_HWND.store(hwnd as isize, Ordering::SeqCst);
                TRAY_READY.store(true, Ordering::SeqCst);
            }
        }
    }
}

const WM_TRAYICON: u32 = 0x8000;
const ID_TRAY_SHOW: usize = 1;
const ID_TRAY_EXIT: usize = 2;

fn setup_tray() {
    use std::ffi::c_void;
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::UI::Shell::*;
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    unsafe extern "system" fn tray_wndproc(
        hwnd: *mut c_void,
        msg: u32,
        wparam: usize,
        lparam: isize,
    ) -> isize {
        match msg {
            WM_TRAYICON => {
                let low = lparam as u32;
                match low {
                    WM_RBUTTONUP => {
                        let mut pt = POINT { x: 0, y: 0 };
                        GetCursorPos(&mut pt);
                        SetForegroundWindow(hwnd);
                        let menu = CreatePopupMenu();
                        let show_text: Vec<u16> = "显示窗口\0".encode_utf16().collect();
                        let exit_text: Vec<u16> = "退出\0".encode_utf16().collect();
                        AppendMenuW(menu, MF_STRING, ID_TRAY_SHOW, show_text.as_ptr());
                        AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null());
                        AppendMenuW(menu, MF_STRING, ID_TRAY_EXIT, exit_text.as_ptr());
                        let ret = TrackPopupMenu(
                            menu,
                            TPM_RETURNCMD | TPM_NONOTIFY,
                            pt.x,
                            pt.y,
                            0,
                            hwnd,
                            std::ptr::null(),
                        );
                        DestroyMenu(menu);
                        match ret {
                            x if x == ID_TRAY_SHOW as i32 => {
                                let main_hwnd =
                                    MAIN_HWND.load(Ordering::SeqCst) as *mut c_void;
                                if !main_hwnd.is_null() {
                                    ShowWindow(main_hwnd, SW_SHOW);
                                    SetForegroundWindow(main_hwnd);
                                }
                            }
                            x if x == ID_TRAY_EXIT as i32 => {
                                PostQuitMessage(0);
                            }
                            _ => {}
                        }
                        0
                    }
                    WM_LBUTTONDBLCLK => {
                        let main_hwnd = MAIN_HWND.load(Ordering::SeqCst) as *mut c_void;
                        if !main_hwnd.is_null() {
                            ShowWindow(main_hwnd, SW_SHOW);
                            SetForegroundWindow(main_hwnd);
                        }
                        0
                    }
                    _ => 0,
                }
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                0
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }

    std::thread::sleep(Duration::from_millis(500));

    for _ in 0..40 {
        if TRAY_READY.load(Ordering::SeqCst) {
            break;
        }
        find_my_hwnd();
        std::thread::sleep(Duration::from_millis(200));
    }

    let class_name: Vec<u16> = "CMDRunnerTray\0".encode_utf16().collect();
    let wnd_class = WNDCLASSW {
        style: 0,
        lpfnWndProc: Some(tray_wndproc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: std::ptr::null_mut(),
        hIcon: std::ptr::null_mut(),
        hCursor: std::ptr::null_mut(),
        hbrBackground: std::ptr::null_mut(),
        lpszMenuName: std::ptr::null(),
        lpszClassName: class_name.as_ptr(),
    };

    unsafe {
        RegisterClassW(&wnd_class);

        let tray_hwnd = CreateWindowExW(
            0,
            class_name.as_ptr(),
            std::ptr::null(),
            WS_OVERLAPPEDWINDOW,
            0,
            0,
            0,
            0,
            HWND_MESSAGE,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        );

        if tray_hwnd.is_null() {
            return;
        }

        let mut icon_data: NOTIFYICONDATAW = std::mem::zeroed();
        icon_data.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        icon_data.hWnd = tray_hwnd;
        icon_data.uID = 1;
        icon_data.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
        icon_data.uCallbackMessage = WM_TRAYICON;
        icon_data.hIcon = create_win32_icon();

        let tip: Vec<u16> = "CMD Runner v0.7.0\0".encode_utf16().collect();
        for (i, &c) in tip.iter().enumerate() {
            if i < 128 {
                icon_data.szTip[i] = c;
            }
        }

        Shell_NotifyIconW(NIM_ADD, &icon_data);

        let mut msg = MSG {
            hwnd: std::ptr::null_mut(),
            message: 0,
            wParam: 0,
            lParam: 0,
            time: 0,
            pt: POINT { x: 0, y: 0 },
        };
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        Shell_NotifyIconW(NIM_DELETE, &icon_data);
    }
}

fn create_win32_icon() -> *mut std::ffi::c_void {
    use windows_sys::Win32::Graphics::Gdi::*;
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    unsafe {
        let hdc = GetDC(std::ptr::null_mut());
        let mem_dc = CreateCompatibleDC(hdc);
        let bmp = CreateCompatibleBitmap(hdc, 32, 32);
        let mask_bmp = CreateBitmap(32, 32, 1, 1, std::ptr::null());
        let old_bmp = SelectObject(mem_dc, bmp);

        let brush = CreateSolidBrush(0x00CC6633);
        let old_brush = SelectObject(mem_dc, brush);
        let pen = GetStockObject(NULL_PEN);
        let old_pen = SelectObject(mem_dc, pen);
        Rectangle(mem_dc, 0, 0, 32, 32);
        SelectObject(mem_dc, old_brush);
        DeleteObject(brush);
        SelectObject(mem_dc, old_pen);

        let white = CreateSolidBrush(0x00FFFFFF);
        let old_brush2 = SelectObject(mem_dc, white);
        let null_pen = GetStockObject(NULL_PEN);
        let old_pen2 = SelectObject(mem_dc, null_pen);

        let c_pixels: &[(i32, i32)] = &[
            (8,4),(9,4),(10,4),(11,4),(12,4),(13,4),
            (6,5),(7,5),
            (5,6),(6,6),
            (5,7),(6,7),
            (5,8),(6,8),
            (5,9),(6,9),
            (5,10),(6,10),
            (5,11),(6,11),
            (5,12),(6,12),
            (5,13),(6,13),
            (5,14),(6,14),
            (5,15),(6,15),
            (5,16),(6,16),
            (6,17),(7,17),
            (8,18),(9,18),(10,18),(11,18),(12,18),(13,18),
        ];

        for &(x, y) in c_pixels {
            Rectangle(mem_dc, x, y, x + 2, y + 2);
        }

        SelectObject(mem_dc, old_brush2);
        DeleteObject(white);
        SelectObject(mem_dc, old_pen2);

        SelectObject(mem_dc, old_bmp);

        let mut icon_info = ICONINFO {
            fIcon: 1,
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: mask_bmp,
            hbmColor: bmp,
        };
        let icon = CreateIconIndirect(&mut icon_info);

        DeleteObject(mask_bmp as _);
        DeleteObject(bmp as _);
        DeleteDC(mem_dc);
        ReleaseDC(std::ptr::null_mut(), hdc);

        icon
    }
}
