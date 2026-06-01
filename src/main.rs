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
use tray_icon::menu::{Menu as TrayMenu, MenuEvent, PredefinedMenuItem};
use tray_icon::{Icon, TrayIconBuilder};

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

fn main() -> eframe::Result {
    std::thread::spawn(setup_tray);

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("CMD Runner v0.6.0")
            .with_inner_size([900.0, 650.0])
            .with_min_inner_size([600.0, 400.0]),
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
}

impl App {
    fn new() -> Self {
        let config = Config::load();
        let mut app = Self {
            tabs: Vec::new(),
            active_tab: None,
            edit_buffers: std::collections::HashMap::new(),
            confirm_delete: None,
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

                                                let short_path = std::path::Path::new(&tab.path)
                                                    .file_name()
                                                    .unwrap_or_default()
                                                    .to_string_lossy()
                                                    .to_string();
                                                ui.label(
                                                    egui::RichText::new(&short_path)
                                                        .size(9.0)
                                                        .color(egui::Color32::from_rgb(120, 120, 130)),
                                                );
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
                        let mut style = (*ctx.style()).clone();
                        style.visuals.override_text_color = Some(egui::Color32::from_rgb(220, 220, 220));
                        style.visuals.extreme_bg_color = egui::Color32::from_rgb(35, 35, 40);
                        style.visuals.faint_bg_color = egui::Color32::from_rgb(40, 40, 45);
                        ui.style_mut().visuals.override_text_color = Some(egui::Color32::from_rgb(220, 220, 220));
                        ui.style_mut().visuals.extreme_bg_color = egui::Color32::from_rgb(35, 35, 40);
                        egui::ScrollArea::vertical()
                            .stick_to_bottom(true)
                            .show(ui, |ui| {
                                ui.add(
                                    egui::TextEdit::multiline(&mut output.as_str())
                                        .font(egui::TextStyle::Monospace)
                                        .text_color(egui::Color32::from_rgb(220, 220, 220))
                                        .desired_width(f32::INFINITY),
                                );
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
    use windows_sys::Win32::UI::WindowsAndMessaging::{EnumWindows, GetWindowThreadProcessId};

    let our_pid = std::process::id();

    unsafe extern "system" fn enum_cb(
        hwnd: windows_sys::Win32::Foundation::HWND,
        lparam: isize,
    ) -> i32 {
        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, &mut pid);
        if pid == lparam as u32 {
            MAIN_HWND.store(hwnd as isize, Ordering::SeqCst);
            TRAY_READY.store(true, Ordering::SeqCst);
            0
        } else {
            1
        }
    }

    unsafe {
        EnumWindows(Some(enum_cb), our_pid as isize);
    }
}

fn setup_tray() {
    use windows_sys::Win32::UI::WindowsAndMessaging::{SetForegroundWindow, ShowWindow, SW_SHOW};

    std::thread::sleep(Duration::from_millis(500));

    for _ in 0..40 {
        if TRAY_READY.load(Ordering::SeqCst) {
            break;
        }
        find_my_hwnd();
        std::thread::sleep(Duration::from_millis(200));
    }

    let menu = TrayMenu::new();
    let show_item = tray_icon::menu::MenuItem::new("显示窗口", true, None);
    let exit_item = tray_icon::menu::MenuItem::new("退出", true, None);
    let show_id = show_item.id().clone();
    let exit_id = exit_item.id().clone();
    menu.append(&show_item).unwrap();
    menu.append(&PredefinedMenuItem::separator()).unwrap();
    menu.append(&exit_item).unwrap();

    let icon = create_tray_icon();
    let _tray_icon = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("CMD Runner v0.6.0")
        .with_icon(icon)
        .build()
        .expect("Failed to create tray icon");

    let event_rx = MenuEvent::receiver();
    loop {
        if let Ok(event) = event_rx.recv() {
            let hwnd = MAIN_HWND.load(Ordering::SeqCst) as *mut std::ffi::c_void;
            if event.id == show_id && !hwnd.is_null() {
                unsafe {
                    ShowWindow(hwnd, SW_SHOW);
                    SetForegroundWindow(hwnd);
                }
            } else if event.id == exit_id {
                std::process::exit(0);
            }
        }
    }
}

fn create_tray_icon() -> Icon {
    let size = 32;
    let mut rgba = vec![0u8; size * size * 4];
    for y in 0..size {
        for x in 0..size {
            let idx = (y * size + x) * 4;
            let cx = x as i32 - 16;
            let cy = y as i32 - 16;
            let dist = ((cx * cx + cy * cy) as f64).sqrt();
            if dist < 12.0 {
                rgba[idx] = 60;
                rgba[idx + 1] = 180;
                rgba[idx + 2] = 100;
                rgba[idx + 3] = 255;
                if dist > 10.0 {
                    rgba[idx] = 40;
                    rgba[idx + 1] = 140;
                    rgba[idx + 2] = 80;
                }
            } else if dist < 14.0 {
                rgba[idx] = 40;
                rgba[idx + 1] = 140;
                rgba[idx + 2] = 80;
                rgba[idx + 3] = 255;
            }
        }
    }
    Icon::from_rgba(rgba, size as u32, size as u32).unwrap()
}
