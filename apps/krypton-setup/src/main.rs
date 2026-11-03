//! Genoryn Krypton Setup: an animated, per-user installer.
#![cfg_attr(windows, windows_subsystem = "windows")]

mod install;

use eframe::egui::{
    self, Align2, Color32, FontData, FontDefinitions, FontFamily, FontId, Pos2, Rect, Rounding,
    Sense, Shape, Stroke, Vec2, pos2, vec2,
};
use install::{Options, Progress};
use std::f32::consts::TAU;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;

const W: f32 = 960.0;
const H: f32 = 600.0;
const MARK_PNG: &[u8] = include_bytes!("../assets/mark.png");

#[derive(PartialEq, Clone, Copy)]
enum Screen {
    Welcome,
    Working,
    Done,
    Failed,
}

struct App {
    screen: Screen,
    uninstall: bool,
    opts: Options,
    path_text: String,
    launch: bool,
    progress: Arc<Progress>,
    result: Arc<std::sync::Mutex<Option<Result<(), String>>>>,
    shown: f32,
    started: f64,
    done_at: f64,
    logo: Option<egui::TextureHandle>,
    error: String,
}

fn decode_png(bytes: &[u8]) -> Option<(Vec<u8>, u32, u32)> {
    let mut dec = png::Decoder::new(bytes);
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::ALPHA);
    let mut r = dec.read_info().ok()?;
    let mut buf = vec![0; r.output_buffer_size()];
    let info = r.next_frame(&mut buf).ok()?;
    buf.truncate(info.buffer_size());
    let rgba = match info.color_type {
        png::ColorType::Rgba => buf,
        png::ColorType::Rgb => buf
            .chunks(3)
            .flat_map(|c| [c[0], c[1], c[2], 255])
            .collect(),
        png::ColorType::GrayscaleAlpha => buf
            .chunks(2)
            .flat_map(|c| [c[0], c[0], c[0], c[1]])
            .collect(),
        png::ColorType::Grayscale => buf.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        _ => return None,
    };
    Some((rgba, info.width, info.height))
}

fn white(a: f32) -> Color32 {
    Color32::from_rgba_unmultiplied(255, 255, 255, (a.clamp(0.0, 1.0) * 255.0) as u8)
}
fn st(w: f32, c: Color32) -> Stroke {
    Stroke::new(w, c)
}
fn grey(v: u8) -> Color32 {
    Color32::from_rgb(v, v, v)
}
fn ease(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

impl App {
    fn new(cc: &eframe::CreationContext, uninstall: bool, dir: PathBuf) -> Self {
        let mut fonts = FontDefinitions::default();
        fonts.font_data.insert(
            "inter".into(),
            FontData::from_static(include_bytes!("../assets/Inter-Regular.ttf")),
        );
        fonts.font_data.insert(
            "inter-sb".into(),
            FontData::from_static(include_bytes!("../assets/Inter-SemiBold.ttf")),
        );
        fonts
            .families
            .get_mut(&FontFamily::Proportional)
            .unwrap()
            .insert(0, "inter".into());
        fonts.families.insert(
            FontFamily::Name("sb".into()),
            vec!["inter-sb".into(), "inter".into()],
        );
        cc.egui_ctx.set_fonts(fonts);
        let logo = decode_png(MARK_PNG).map(|(rgba, w, h)| {
            cc.egui_ctx.load_texture(
                "mark",
                egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &rgba),
                egui::TextureOptions::LINEAR,
            )
        });
        let mut visuals = egui::Visuals::dark();
        visuals.panel_fill = Color32::BLACK;
        visuals.extreme_bg_color = grey(10);
        visuals.selection.bg_fill = grey(80);
        visuals.widgets.inactive.bg_stroke = st(1.0, white(0.12));
        cc.egui_ctx.set_visuals(visuals);
        Self {
            screen: Screen::Welcome,
            uninstall,
            path_text: dir.display().to_string(),
            opts: Options {
                dir,
                desktop: true,
                start_menu: true,
            },
            launch: true,
            progress: Arc::new(Progress::default()),
            result: Arc::new(std::sync::Mutex::new(None)),
            shown: 0.0,
            started: 0.0,
            done_at: 0.0,
            logo,
            error: String::new(),
        }
    }

    fn start(&mut self, now: f64) {
        self.opts.dir = PathBuf::from(self.path_text.trim());
        self.screen = Screen::Working;
        self.started = now;
        let (opts, p, res, un) = (
            self.opts.clone(),
            self.progress.clone(),
            self.result.clone(),
            self.uninstall,
        );
        std::thread::spawn(move || {
            let r = if un {
                install::uninstall(&opts.dir, &p)
            } else {
                install::install(&opts, &p)
            };
            *res.lock().unwrap() = Some(r.map_err(|e| format!("{e:#}")));
        });
    }
}

fn button(ui: &mut egui::Ui, rect: Rect, label: &str, primary: bool, t: f32) -> bool {
    let r = ui.allocate_rect(rect, Sense::click());
    let hover = r.hovered();
    let p = ui.painter();
    if primary {
        if hover {
            for k in 1..=4 {
                p.rect_stroke(
                    rect.expand(k as f32 * 2.5),
                    Rounding::same(12.0 + k as f32 * 2.0),
                    st(2.0, white(0.10 / k as f32 + 0.02)),
                );
            }
        }
        let pulse = 0.5 + 0.5 * (t * 2.0).sin();
        p.rect_filled(
            rect.expand(1.0 + pulse * 1.5),
            Rounding::same(13.0),
            white(0.05 + 0.04 * pulse),
        );
        p.rect_filled(
            rect,
            Rounding::same(11.0),
            white(if hover { 1.0 } else { 0.94 }),
        );
        p.text(
            rect.center(),
            Align2::CENTER_CENTER,
            label,
            FontId::new(16.0, FontFamily::Name("sb".into())),
            Color32::BLACK,
        );
    } else {
        p.rect_filled(
            rect,
            Rounding::same(11.0),
            white(if hover { 0.10 } else { 0.06 }),
        );
        p.rect_stroke(rect, Rounding::same(11.0), st(1.0, white(0.17)));
        p.text(
            rect.center(),
            Align2::CENTER_CENTER,
            label,
            FontId::new(15.0, FontFamily::Name("sb".into())),
            white(0.92),
        );
    }
    if hover {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    r.clicked()
}

fn checkbox(ui: &mut egui::Ui, pos: Pos2, label: &str, on: &mut bool) {
    let rect = Rect::from_min_size(pos, vec2(320.0, 26.0));
    let r = ui.allocate_rect(rect, Sense::click());
    if r.clicked() {
        *on = !*on;
    }
    let p = ui.painter();
    let b = Rect::from_min_size(pos + vec2(0.0, 3.0), vec2(20.0, 20.0));
    p.rect_filled(
        b,
        Rounding::same(6.0),
        if *on { white(0.95) } else { white(0.05) },
    );
    p.rect_stroke(
        b,
        Rounding::same(6.0),
        st(1.0, white(if r.hovered() { 0.4 } else { 0.2 })),
    );
    if *on {
        let c = b.center();
        p.add(Shape::line(
            vec![
                c + vec2(-5.0, 0.0),
                c + vec2(-1.5, 3.5),
                c + vec2(5.0, -4.0),
            ],
            st(2.2, Color32::BLACK),
        ));
    }
    p.text(
        pos + vec2(32.0, 13.0),
        Align2::LEFT_CENTER,
        label,
        FontId::proportional(15.0),
        white(0.85),
    );
}

fn draw_background(p: &egui::Painter, rect: Rect, t: f32) {
    p.rect_filled(rect, Rounding::ZERO, Color32::BLACK);
    // Slowly drifting soft light pools (layered translucent discs).
    let pools = [
        (0.18, 0.30, 260.0, 0.0),
        (0.82, 0.22, 220.0, 1.7),
        (0.62, 0.86, 300.0, 3.1),
    ];
    for (x, y, r, ph) in pools {
        let c = pos2(
            rect.left() + rect.width() * x + 30.0 * (t * 0.07 + ph).sin(),
            rect.top() + rect.height() * y + 22.0 * (t * 0.05 + ph).cos(),
        );
        for k in 0..14 {
            let f = k as f32 / 14.0;
            p.circle_filled(c, r * (1.0 - f), white(0.006 + 0.004 * f));
        }
    }
    // Faint grid hairlines.
    for i in 1..12 {
        let x = rect.left() + rect.width() * i as f32 / 12.0;
        p.line_segment(
            [pos2(x, rect.top()), pos2(x, rect.bottom())],
            st(1.0, white(0.012)),
        );
    }
}

fn draw_logo(
    p: &egui::Painter,
    logo: &Option<egui::TextureHandle>,
    c: Pos2,
    size: f32,
    t: f32,
    energy: f32,
) {
    // Halo.
    let breathe = 0.5 + 0.5 * (t * 1.3).sin();
    for k in 0..18 {
        let f = k as f32 / 18.0;
        p.circle_filled(
            c,
            size * (0.62 + 0.55 * (1.0 - f)),
            white((0.004 + 0.006 * f) * (0.6 + 0.4 * breathe) * (1.0 + energy)),
        );
    }
    // Orbit ring with travelling lights.
    let ring = size * 0.78;
    p.circle_stroke(c, ring, st(1.0, white(0.10)));
    p.circle_stroke(c, ring * 1.18, st(1.0, white(0.05)));
    let n = 36;
    for i in 0..n {
        let a = i as f32 / n as f32 * TAU + t * (0.6 + energy * 1.6);
        let head = (i as f32 / n as f32 * 3.0 + t * 0.9).fract();
        let alpha = (head.powi(6) * 0.9 + 0.08) * (0.6 + 0.4 * energy.max(0.3));
        p.circle_filled(
            c + vec2(a.cos(), a.sin()) * ring,
            1.6 + head.powi(6) * 2.4,
            white(alpha),
        );
    }
    for i in 0..3 {
        let a = -t * 0.45 + i as f32 * TAU / 3.0;
        let q = c + vec2(a.cos(), a.sin()) * ring * 1.18;
        p.circle_filled(q, 3.2, white(0.75));
        p.circle_filled(q, 8.0, white(0.08));
    }
    if let Some(tex) = logo {
        let r = Rect::from_center_size(c, Vec2::splat(size));
        p.image(
            tex.id(),
            r,
            Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
            Color32::WHITE,
        );
    }
}

impl eframe::App for App {
    fn clear_color(&self, _: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 1.0]
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let now = ctx.input(|i| i.time);
        let t = now as f32;
        ctx.request_repaint();
        if self.screen == Screen::Working
            && let Some(r) = self.result.lock().unwrap().take()
        {
            match r {
                Ok(()) => {
                    self.screen = Screen::Done;
                    self.done_at = now;
                }
                Err(e) => {
                    self.error = e;
                    self.screen = Screen::Failed;
                }
            }
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::none())
            .show(ctx, |ui| {
                let full = ui.max_rect();
                let p = ui.painter().clone();
                draw_background(&p, full, t);
                // Title bar: drag to move, close button.
                let bar = Rect::from_min_size(full.min, vec2(W, 44.0));
                let drag = ui.allocate_rect(
                    Rect::from_min_size(full.min, vec2(W - 60.0, 44.0)),
                    Sense::click_and_drag(),
                );
                if drag.drag_started() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
                }
                p.text(
                    bar.left_center() + vec2(20.0, 0.0),
                    Align2::LEFT_CENTER,
                    if self.uninstall {
                        "Genoryn Krypton · Uninstall"
                    } else {
                        "Genoryn Krypton · Setup"
                    },
                    FontId::proportional(13.0),
                    white(0.55),
                );
                let close = Rect::from_center_size(
                    pos2(full.right() - 28.0, full.top() + 22.0),
                    vec2(34.0, 28.0),
                );
                let cr = ui.allocate_rect(close, Sense::click());
                if cr.hovered() {
                    p.rect_filled(close, Rounding::same(7.0), white(0.10));
                }
                p.text(
                    close.center(),
                    Align2::CENTER_CENTER,
                    "✕",
                    FontId::proportional(14.0),
                    white(0.8),
                );
                if cr.clicked() && self.screen != Screen::Working {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }

                let pm = self.progress.permille.load(Ordering::Relaxed) as f32 / 1000.0;
                self.shown += (pm - self.shown) * 0.08;
                let energy = if self.screen == Screen::Working {
                    1.0
                } else {
                    0.0
                };
                let logo_c = pos2(full.left() + 250.0, full.top() + 300.0);
                draw_logo(&p, &self.logo, logo_c, 150.0, t, energy);

                let x = full.left() + 500.0;
                let fade = ease((t - 0.2) / 0.8);
                p.text(
                    pos2(x, full.top() + 150.0),
                    Align2::LEFT_TOP,
                    "GENORYN",
                    FontId::new(14.0, FontFamily::Name("sb".into())),
                    white(0.5 * fade),
                );
                p.text(
                    pos2(x, full.top() + 172.0),
                    Align2::LEFT_TOP,
                    "Krypton",
                    FontId::new(52.0, FontFamily::Name("sb".into())),
                    white(fade),
                );
                p.text(
                    pos2(x, full.top() + 240.0),
                    Align2::LEFT_TOP,
                    format!("Quantum-resistant, private money · v{}", install::VERSION),
                    FontId::proportional(16.0),
                    white(0.6 * fade),
                );

                match self.screen {
                    Screen::Welcome => {
                        let y = full.top() + 290.0;
                        if self.uninstall {
                            p.text(
                                pos2(x, y),
                                Align2::LEFT_TOP,
                                "Remove the program files and shortcuts.",
                                FontId::proportional(15.0),
                                white(0.8),
                            );
                            p.text(
                                pos2(x, y + 24.0),
                                Align2::LEFT_TOP,
                                "Your wallets and recovery data are kept.",
                                FontId::proportional(15.0),
                                white(0.55),
                            );
                        } else {
                            p.text(
                                pos2(x, y),
                                Align2::LEFT_TOP,
                                "Install location",
                                FontId::proportional(13.0),
                                white(0.55),
                            );
                            let field = Rect::from_min_size(pos2(x, y + 20.0), vec2(400.0, 36.0));
                            p.rect_filled(
                                field,
                                Rounding::same(9.0),
                                Color32::from_rgba_unmultiplied(0, 0, 0, 120),
                            );
                            p.rect_stroke(field, Rounding::same(9.0), st(1.0, white(0.12)));
                            ui.put(
                                field.shrink2(vec2(10.0, 6.0)),
                                egui::TextEdit::singleline(&mut self.path_text)
                                    .frame(false)
                                    .font(FontId::proportional(14.0)),
                            );
                            checkbox(
                                ui,
                                pos2(x, y + 70.0),
                                "Desktop shortcut",
                                &mut self.opts.desktop,
                            );
                            checkbox(
                                ui,
                                pos2(x, y + 100.0),
                                "Start menu entry",
                                &mut self.opts.start_menu,
                            );
                            checkbox(
                                ui,
                                pos2(x, y + 130.0),
                                "Launch the wallet when finished",
                                &mut self.launch,
                            );
                        }
                        let b = Rect::from_min_size(pos2(x, full.top() + 470.0), vec2(190.0, 48.0));
                        if button(
                            ui,
                            b,
                            if self.uninstall {
                                "Uninstall"
                            } else {
                                "Install"
                            },
                            true,
                            t,
                        ) {
                            self.start(now);
                        }
                        p.text(
                            pos2(x, full.top() + 540.0),
                            Align2::LEFT_TOP,
                            "Open source · MIT or Apache-2.0 · no account, no telemetry",
                            FontId::proportional(12.5),
                            white(0.4),
                        );
                    }
                    Screen::Working | Screen::Done => {
                        let y = full.top() + 300.0;
                        let bar = Rect::from_min_size(pos2(x, y + 40.0), vec2(400.0, 10.0));
                        p.rect_filled(bar, Rounding::same(5.0), white(0.07));
                        let fill = Rect::from_min_size(
                            bar.min,
                            vec2(bar.width() * self.shown.clamp(0.0, 1.0), bar.height()),
                        );
                        p.rect_filled(fill, Rounding::same(5.0), white(0.92));
                        // Shimmer and sparks at the leading edge.
                        let sx = fill.left() + ((t * 0.9).fract()) * fill.width();
                        p.rect_filled(
                            Rect::from_center_size(pos2(sx, bar.center().y), vec2(40.0, 10.0))
                                .intersect(fill),
                            Rounding::same(5.0),
                            white(0.5),
                        );
                        let edge = pos2(fill.right(), bar.center().y);
                        for k in 0..10 {
                            let ph = (t * 1.7 + k as f32 * 0.37).fract();
                            let ang = (k as f32 * 2.399) % TAU;
                            let q = edge + vec2(-ph * 26.0, ang.sin() * ph * 12.0);
                            p.circle_filled(q, 1.6 * (1.0 - ph), white(0.8 * (1.0 - ph)));
                        }
                        p.circle_filled(edge, 9.0, white(0.07));
                        p.text(
                            pos2(x, y),
                            Align2::LEFT_TOP,
                            if self.screen == Screen::Done {
                                if self.uninstall {
                                    "Removed".to_string()
                                } else {
                                    "Installed".to_string()
                                }
                            } else {
                                self.progress.step.get()
                            },
                            FontId::new(17.0, FontFamily::Name("sb".into())),
                            white(0.95),
                        );
                        p.text(
                            pos2(bar.right(), y + 2.0),
                            Align2::RIGHT_TOP,
                            format!("{:>3.0} %", self.shown * 100.0),
                            FontId::monospace(15.0),
                            white(0.7),
                        );
                        let steps = if self.uninstall {
                            [
                                "Stopping running programs",
                                "Removing shortcuts",
                                "Removing program files",
                                "Cleaning up",
                            ]
                        } else {
                            [
                                "Unpacking the quantum-resistant core",
                                "Installing node, miner and wallet",
                                "Creating shortcuts",
                                "Registering with Windows",
                            ]
                        };
                        for (i, s) in steps.iter().enumerate() {
                            let th = (i as f32 + 1.0) / steps.len() as f32;
                            let ok = self.shown >= th - 0.02;
                            let sy = y + 74.0 + i as f32 * 28.0;
                            let c = pos2(x + 9.0, sy + 9.0);
                            if ok {
                                p.circle_filled(c, 9.0, white(0.92));
                                p.add(Shape::line(
                                    vec![
                                        c + vec2(-4.0, 0.0),
                                        c + vec2(-1.0, 3.0),
                                        c + vec2(4.5, -3.5),
                                    ],
                                    st(2.0, Color32::BLACK),
                                ));
                            } else {
                                p.circle_stroke(c, 8.0, st(1.2, white(0.25)));
                                let a = t * 4.0;
                                p.circle_filled(c + vec2(a.cos(), a.sin()) * 8.0, 2.0, white(0.7));
                            }
                            p.text(
                                pos2(x + 28.0, sy + 9.0),
                                Align2::LEFT_CENTER,
                                *s,
                                FontId::proportional(14.5),
                                white(if ok { 0.85 } else { 0.45 }),
                            );
                        }
                        if self.screen == Screen::Done {
                            // Animated checkmark drawn on the logo ring.
                            let k = ease(((now - self.done_at) as f32) / 0.9);
                            let c = logo_c + vec2(78.0, 78.0);
                            p.circle_filled(c, 26.0 * k, white(0.95));
                            let pts = [
                                c + vec2(-11.0, 0.0),
                                c + vec2(-3.0, 8.0),
                                c + vec2(12.0, -9.0),
                            ];
                            let seg = k * 2.0;
                            let mut line = vec![pts[0]];
                            line.push(pts[0] + (pts[1] - pts[0]) * seg.min(1.0));
                            if seg > 1.0 {
                                line.push(pts[1] + (pts[2] - pts[1]) * (seg - 1.0));
                            }
                            p.add(Shape::line(line, st(3.5, Color32::BLACK)));
                            let label = if self.uninstall {
                                "Close"
                            } else if self.launch {
                                "Launch Genoryn Krypton"
                            } else {
                                "Finish"
                            };
                            let b = Rect::from_min_size(
                                pos2(x, full.top() + 470.0),
                                vec2(
                                    if self.launch && !self.uninstall {
                                        260.0
                                    } else {
                                        170.0
                                    },
                                    48.0,
                                ),
                            );
                            if button(ui, b, label, true, t) {
                                if self.launch && !self.uninstall {
                                    install::launch(&self.opts.dir);
                                }
                                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                            }
                        }
                    }
                    Screen::Failed => {
                        let y = full.top() + 300.0;
                        p.text(
                            pos2(x, y),
                            Align2::LEFT_TOP,
                            "Something went wrong",
                            FontId::new(18.0, FontFamily::Name("sb".into())),
                            white(0.95),
                        );
                        let galley = ui.fonts(|f| {
                            f.layout(
                                self.error.clone(),
                                FontId::proportional(13.5),
                                white(0.65),
                                400.0,
                            )
                        });
                        p.galley(pos2(x, y + 32.0), galley, white(0.65));
                        let b = Rect::from_min_size(pos2(x, full.top() + 470.0), vec2(150.0, 48.0));
                        if button(ui, b, "Close", false, t) {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                    }
                }
                p.rect_stroke(full.shrink(0.5), Rounding::same(0.0), st(1.0, white(0.10)));
            });
    }
}

fn main() -> eframe::Result<()> {
    let uninstall = std::env::args().any(|a| a == "--uninstall");
    // Unattended mode for automation and in-app updates:
    // --silent [--dir PATH] [--no-shortcuts] [--launch]
    if std::env::args().any(|a| a == "--silent") {
        let args: Vec<String> = std::env::args().collect();
        let dir = args
            .iter()
            .position(|a| a == "--dir")
            .and_then(|i| args.get(i + 1))
            .map(PathBuf::from)
            .unwrap_or_else(install::default_dir);
        let shortcuts = !args.iter().any(|a| a == "--no-shortcuts");
        let p = Arc::new(Progress::default());
        let r = if uninstall {
            install::uninstall(&dir, &p)
        } else {
            install::install(
                &Options {
                    dir: dir.clone(),
                    desktop: shortcuts,
                    start_menu: shortcuts,
                },
                &p,
            )
        };
        let log = std::env::temp_dir().join("genoryn-krypton-setup.log");
        let _ = std::fs::write(
            &log,
            match &r {
                Ok(()) => format!("ok {}\n", dir.display()),
                Err(e) => format!("error {e:#}\n"),
            },
        );
        if r.is_ok() && !uninstall && args.iter().any(|a| a == "--launch") {
            install::launch(&dir);
        }
        std::process::exit(if r.is_ok() { 0 } else { 1 });
    }
    let dir = if uninstall {
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|p| p.to_path_buf()))
            .unwrap_or_else(install::default_dir)
    } else {
        install::default_dir()
    };
    let icon = decode_png(MARK_PNG).map(|(rgba, width, height)| egui::IconData {
        rgba,
        width,
        height,
    });
    let mut vp = egui::ViewportBuilder::default()
        .with_inner_size([W, H])
        .with_resizable(false)
        .with_decorations(false)
        .with_title("Genoryn Krypton Setup");
    if let Some(i) = icon {
        vp = vp.with_icon(Arc::new(i));
    }
    let opts = eframe::NativeOptions {
        viewport: vp,
        centered: true,
        ..Default::default()
    };
    eframe::run_native(
        "Genoryn Krypton Setup",
        opts,
        Box::new(move |cc| Ok(Box::new(App::new(cc, uninstall, dir)))),
    )
}
