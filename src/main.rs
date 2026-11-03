#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
#![allow(deprecated)] // egui 0.37 把 Id::new 标了废弃，这里仍用它做稳定的持久 id

mod fonts;
mod gdi;

use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::Duration;

use eframe::egui;
use egui::{
    Align2, Color32, CursorIcon, FontId, Id, Rect, RichText, Sense, Stroke, StrokeKind,
    TextureHandle, TextureOptions,
};

use fonts::Catalog;
use gdi::TextRaster;
use windows::Win32::UI::Shell::{ILCreateFromPathW, ILFree, SHOpenFolderAndSelectItems};

const BG: Color32 = Color32::from_rgb(0x0a, 0x0c, 0x11);
const PANEL: Color32 = Color32::from_rgb(0x0d, 0x10, 0x17);
const CARD: Color32 = Color32::from_rgb(0x14, 0x18, 0x22);
const CARD_HOVER: Color32 = Color32::from_rgb(0x19, 0x1f, 0x2c);
const LINE: Color32 = Color32::from_rgb(0x22, 0x28, 0x36);
const ACCENT: Color32 = Color32::from_rgb(0x6f, 0x9b, 0xff);
const TEXT: Color32 = Color32::from_rgb(0xe9, 0xed, 0xf6);
const NAME: Color32 = Color32::from_rgb(0xc8, 0xd1, 0xe3);
const DIM: Color32 = Color32::from_rgb(0x8b, 0x93, 0xa8);
const FAINT: Color32 = Color32::from_rgb(0x5d, 0x65, 0x79);
const SAMPLE_TINT: Color32 = Color32::from_rgb(0xf2, 0xf5, 0xfb);

fn main() -> eframe::Result {
    unsafe {
        let _ = windows::Win32::System::Com::CoInitializeEx(
            None,
            windows::Win32::System::Com::COINIT_APARTMENTTHREADED,
        );
    }

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_maximized(true)
            .with_min_inner_size([880.0, 540.0])
            .with_title("本机字体预览"),
        ..Default::default()
    };

    eframe::run_native(
        "本机字体预览",
        options,
        Box::new(|cc| Ok(Box::new(App::new(cc)))),
    )
}

struct App {
    cat: Catalog,
    raster: TextRaster,
    text: String,
    search: String,
    size: f32,
    results: Vec<usize>,
    last_text: String,
    last_search: String,
    textures: HashMap<u64, (TextureHandle, u64)>,
    clock: u64,
    toast: Option<(String, f64)>,
    focused_once: bool,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        install_fonts(&cc.egui_ctx);
        apply_style(&cc.egui_ctx);

        let cat = Catalog::new().expect("读取系统字体失败");
        let mut app = Self {
            cat,
            raster: TextRaster::new(),
            text: String::new(),
            search: String::new(),
            size: 34.0,
            results: Vec::new(),
            last_text: String::new(),
            last_search: String::new(),
            textures: HashMap::new(),
            clock: 0,
            toast: None,
            focused_once: false,
        };
        app.results = app.cat.query("", "");
        app
    }

    fn sample_height(&self) -> f32 {
        // 留出完整两行的高度，避免第二行被切掉半个字
        self.size * 2.82 + 6.0
    }

    /// 输入变了才重新筛选，避免每帧都查字形。
    fn refresh(&mut self) {
        if self.text == self.last_text && self.search == self.last_search {
            return;
        }
        self.last_text.clone_from(&self.text);
        self.last_search.clone_from(&self.search);
        self.results = self.cat.query(&self.text, &self.search);
        self.textures.clear();
    }

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        let frame = egui::Frame::new()
            .fill(PANEL)
            .inner_margin(egui::Margin {
                left: 22,
                right: 22,
                top: 14,
                bottom: 13,
            })
            .stroke(Stroke::new(1.0, LINE));

        frame.show(ui, |ui| {
            ui.horizontal(|ui| {
                let (dot, _) = ui.allocate_exact_size(egui::vec2(7.0, 7.0), Sense::hover());
                ui.painter().circle_filled(dot.center(), 3.5, ACCENT);
                ui.add_space(5.0);
                ui.label(RichText::new("本机字体预览").size(14.0).strong().color(TEXT));
                ui.add_space(12.0);
                ui.label(
                    RichText::new(format!(
                        "{} / {} 款字体",
                        self.results.len(),
                        self.cat.total()
                    ))
                    .size(12.0)
                    .color(DIM),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        RichText::new("点击卡片复制字体名")
                            .size(11.5)
                            .color(FAINT),
                    );
                });
            });

            ui.add_space(10.0);
            let edit = egui::TextEdit::multiline(&mut self.text)
                .id(Id::new("main_text"))
                .hint_text("输入文字，只显示能完整渲染这些字的字体…")
                .font(FontId::proportional(17.0))
                .desired_rows(1)
                .desired_width(f32::INFINITY);
            let resp = ui.add(edit);
            if !self.focused_once {
                resp.request_focus();
                self.focused_once = true;
            }

            ui.add_space(9.0);
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.search)
                        .id(Id::new("search"))
                        .hint_text("搜索字体名 / 拼音 / 首字母：weiruan、wryh")
                        .font(FontId::proportional(13.0))
                        .desired_width(400.0),
                );
                if !self.search.is_empty() && ui.small_button("×").clicked() {
                    self.search.clear();
                }
                ui.add_space(20.0);
                ui.label(RichText::new("字号").size(12.0).color(FAINT));
                ui.add(
                    egui::Slider::new(&mut self.size, 20.0..=72.0)
                        .fixed_decimals(0)
                        .suffix(" px"),
                );
            });
        });
    }

    fn grid(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let n = self.results.len();
        if n == 0 {
            ui.vertical_centered(|ui| {
                ui.add_space(130.0);
                ui.label(RichText::new("没有匹配的字体").size(15.0).color(DIM));
                ui.add_space(6.0);
                let hint = if self.search.trim().is_empty() {
                    format!("没有一款字体能完整渲染「{}」", self.text.trim())
                } else {
                    format!("搜索「{}」没有结果", self.search.trim())
                };
                ui.label(RichText::new(hint).size(12.0).color(FAINT));
            });
            return;
        }

        let gap = 14.0;
        let avail = ui.available_width();
        let cols = (((avail + gap) / (296.0 + gap)).floor() as usize).max(1);
        let card_w = ((avail - gap * (cols as f32 - 1.0)) / cols as f32).floor();
        let card_h = self.sample_height() + 80.0;
        let row_h = card_h + gap;

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .max_height(ui.available_height())
            .show_viewport(ui, |ui, viewport| {
                let rows = n.div_ceil(cols);
                let origin = ui.min_rect().left_top();
                ui.set_min_height(rows as f32 * row_h);
                ui.set_min_width(avail);

                let first = (viewport.min.y / row_h).floor().max(0.0) as usize;
                let last = (((viewport.max.y / row_h).ceil() as usize) + 1).min(rows);

                for row in first..last {
                    for col in 0..cols {
                        let k = row * cols + col;
                        if k >= n {
                            break;
                        }
                        let idx = self.results[k];
                        let rect = Rect::from_min_size(
                            origin
                                + egui::vec2(col as f32 * (card_w + gap), row as f32 * row_h),
                            egui::vec2(card_w, card_h),
                        );
                        let resp = ui
                            .interact(rect, Id::new(("card", idx)), Sense::click())
                            .on_hover_cursor(CursorIcon::PointingHand);
                        self.paint_card(ui, ctx, rect, idx, resp.hovered());

                        // 右键菜单：在资源管理器里定位字体文件
                        let (name, file) = {
                            let item = self.cat.item(idx);
                            (item.name.clone(), item.file.clone())
                        };
                        resp.context_menu(|ui| {
                            ui.set_min_width(200.0);
                            match &file {
                                Some(path) => {
                                    if ui.button("打开文件位置").clicked() {
                                        reveal_in_explorer(path);
                                        egui::Popup::close_all(ui.ctx());
                                    }
                                    ui.label(
                                        RichText::new(file_name(path))
                                            .size(10.5)
                                            .color(FAINT),
                                    );
                                }
                                None => {
                                    ui.add_enabled(false, egui::Button::new("打开文件位置"))
                                        .on_disabled_hover_text("没有拿到该字体的文件路径");
                                }
                            }
                            ui.separator();
                            if ui.button("复制字体族名").clicked() {
                                ctx.copy_text(name.clone());
                                self.toast =
                                    Some((format!("已复制 {name}"), ctx.input(|i| i.time)));
                                egui::Popup::close_all(ui.ctx());
                            }
                        });

                        if resp.clicked() {
                            let name = self.cat.item(idx).name.clone();
                            ctx.copy_text(name.clone());
                            self.toast = Some((format!("已复制 {name}"), ctx.input(|i| i.time)));
                        }
                    }
                }
            });
    }

    fn paint_card(
        &mut self,
        ui: &egui::Ui,
        ctx: &egui::Context,
        rect: Rect,
        idx: usize,
        hovered: bool,
    ) {
        let t = ctx.animate_bool_with_time(Id::new(("hover", idx)), hovered, 0.12);
        let painter = ui.painter();
        painter.rect_filled(rect, 14.0, lerp(CARD, CARD_HOVER, t));
        painter.rect_stroke(
            rect,
            14.0,
            Stroke::new(1.0, lerp(LINE, ACCENT, t * 0.9)),
            StrokeKind::Inside,
        );

        let pad = 16.0;
        let sample = Rect::from_min_size(
            rect.min + egui::vec2(pad, 14.0),
            egui::vec2(rect.width() - pad * 2.0, self.sample_height()),
        );
        let tex = self.texture(ctx, idx, sample);
        painter.image(
            tex.id(),
            sample,
            Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            SAMPLE_TINT,
        );

        let item = self.cat.item(idx);
        let base = rect.bottom() - 42.0;

        // 先量右侧风格标签宽度，左侧字体名按剩余空间裁剪，避免压字
        let meta_font = FontId::proportional(10.5);
        let meta_w = painter
            .layout_no_wrap(item.styles.clone(), meta_font.clone(), FAINT)
            .size()
            .x;
        let name_clip = Rect::from_min_max(
            egui::pos2(rect.left() + pad, base - 2.0),
            egui::pos2(rect.right() - pad - meta_w - 12.0, rect.bottom()),
        );
        let clipped = painter.with_clip_rect(name_clip);
        clipped.text(
            egui::pos2(rect.left() + pad, base),
            Align2::LEFT_TOP,
            &item.label,
            FontId::proportional(12.5),
            NAME,
        );
        if !item.alias.is_empty() {
            clipped.text(
                egui::pos2(rect.left() + pad, base + 16.0),
                Align2::LEFT_TOP,
                &item.alias,
                FontId::proportional(10.5),
                FAINT,
            );
        }
        painter.text(
            egui::pos2(rect.right() - pad, base + 1.0),
            Align2::RIGHT_TOP,
            &item.styles,
            meta_font,
            FAINT,
        );
    }

    /// 每张卡片一张纹理，按 (字体, 文字, 尺寸) 缓存，滚动不会重复栅格化。
    fn texture(&mut self, ctx: &egui::Context, idx: usize, rect: Rect) -> TextureHandle {
        let ppp = ctx.pixels_per_point();
        let w = ((rect.width() * ppp).round() as u32).clamp(8, 2400);
        let h = ((rect.height() * ppp).round() as u32).clamp(8, 1200);
        let px = (self.size * ppp).round().max(6.0) as i32;
        let text = if self.text.trim().is_empty() {
            self.cat.item(idx).label.clone()
        } else {
            self.text.clone()
        };

        let mut hasher = DefaultHasher::new();
        idx.hash(&mut hasher);
        text.hash(&mut hasher);
        w.hash(&mut hasher);
        h.hash(&mut hasher);
        px.hash(&mut hasher);
        let key = hasher.finish();

        self.clock += 1;
        if let Some((tex, used)) = self.textures.get_mut(&key) {
            *used = self.clock;
            return tex.clone();
        }

        let (name, label) = {
            let item = self.cat.item(idx);
            (item.name.clone(), item.label.clone())
        };
        let face = self.raster.face_for(&name, &label);
        let image = self.raster.render(&face, &text, w, h, px);
        let tex = ctx.load_texture(format!("card-{key}"), image, TextureOptions::LINEAR);

        if self.textures.len() > 260 {
            if let Some(old) = self
                .textures
                .iter()
                .min_by_key(|(_, (_, used))| *used)
                .map(|(k, _)| *k)
            {
                self.textures.remove(&old);
            }
        }
        self.textures.insert(key, (tex.clone(), self.clock));
        tex
    }

    fn toast(&mut self, ctx: &egui::Context) {
        let Some((msg, at)) = self.toast.clone() else {
            return;
        };
        let now = ctx.input(|i| i.time);
        let age = now - at;
        if age > 1.6 {
            self.toast = None;
            return;
        }
        let alpha = if age < 1.2 {
            1.0
        } else {
            ((1.6 - age) / 0.4).clamp(0.0, 1.0) as f32
        };
        let a = |v: f32| (v * alpha) as u8;

        egui::Area::new(Id::new("toast"))
            .anchor(Align2::CENTER_BOTTOM, egui::vec2(0.0, -26.0))
            .order(egui::Order::Foreground)
            .interactable(false)
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(Color32::from_rgba_unmultiplied(28, 34, 48, a(246.0)))
                    .corner_radius(10)
                    .inner_margin(egui::Margin::symmetric(16, 9))
                    .stroke(Stroke::new(
                        1.0,
                        Color32::from_rgba_unmultiplied(122, 162, 255, a(90.0)),
                    ))
                    .show(ui, |ui| {
                        ui.label(
                            RichText::new(msg)
                                .size(12.5)
                                .color(Color32::from_rgba_unmultiplied(219, 228, 247, a(255.0))),
                        );
                    });
            });
        ctx.request_repaint_after(Duration::from_millis(40));
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.refresh();
        ui.painter().rect_filled(ui.max_rect(), 0.0, BG);

        self.top_bar(ui);

        let rest = ui.available_height();
        egui::Frame::new()
            .fill(BG)
            .inner_margin(egui::Margin {
                left: 22,
                right: 22,
                top: 16,
                bottom: 18,
            })
            .show(ui, |ui| {
                ui.set_min_height((rest - 36.0).max(120.0));
                ui.set_min_width(ui.available_width());
                self.grid(ui, &ctx);
            });

        self.toast(&ctx);

        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::F)) {
            ctx.memory_mut(|m| m.request_focus(Id::new("search")));
        }
    }
}

fn install_fonts(ctx: &egui::Context) {
    let mut defs = egui::FontDefinitions::default();
    for path in [
        "C:/Windows/Fonts/msyh.ttc",
        "C:/Windows/Fonts/Deng.ttf",
        "C:/Windows/Fonts/simhei.ttf",
        "C:/Windows/Fonts/simsun.ttc",
    ] {
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        defs.font_data
            .insert("cjk".to_owned(), Arc::new(egui::FontData::from_owned(bytes)));
        defs.families
            .entry(egui::FontFamily::Proportional)
            .or_default()
            .insert(0, "cjk".to_owned());
        defs.families
            .entry(egui::FontFamily::Monospace)
            .or_default()
            .push("cjk".to_owned());
        break;
    }
    ctx.set_fonts(defs);
}

fn apply_style(ctx: &egui::Context) {
    ctx.set_theme(egui::ThemePreference::Dark);
    ctx.all_styles_mut(|style| {
        style.visuals = egui::Visuals::dark();
        style.visuals.panel_fill = BG;
        style.visuals.window_fill = PANEL;
        style.visuals.extreme_bg_color = Color32::from_rgb(0x0d, 0x10, 0x17);
        style.visuals.selection.bg_fill = Color32::from_rgba_unmultiplied(111, 155, 255, 70);
        style.visuals.selection.stroke = Stroke::new(1.0, ACCENT);
        style.visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, LINE);
        style.visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, ACCENT);
        style.visuals.widgets.active.bg_stroke = Stroke::new(1.0, ACCENT);
        style.visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, DIM);
        style.spacing.item_spacing = egui::vec2(10.0, 8.0);
        style.spacing.button_padding = egui::vec2(10.0, 5.0);
        style.text_styles.insert(
            egui::TextStyle::Body,
            FontId::new(14.0, egui::FontFamily::Proportional),
        );
    });
}

/// 在资源管理器中打开字体所在目录并选中该文件。
fn reveal_in_explorer(path: &str) {
    unsafe {
        let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
        let pidl = ILCreateFromPathW(windows::core::PCWSTR(wide.as_ptr()));
        if pidl.is_null() {
            return;
        }
        let _ = SHOpenFolderAndSelectItems(pidl, None, 0);
        ILFree(Some(pidl as *const _));
    }
}

fn file_name(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or(path)
}

fn lerp(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let f = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(f(a.r(), b.r()), f(a.g(), b.g()), f(a.b(), b.b()))
}
