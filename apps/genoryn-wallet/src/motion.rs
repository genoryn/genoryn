//! Typography and motion shared with genoryn.io: Inter for text, Unbounded for
//! display figures, JetBrains Mono for labels and digits; a launch splash, a ring
//! loader and a short fade when the page changes.
use eframe::egui::{self, Color32, FontFamily, Stroke};
use std::{sync::Arc, time::Instant};

pub(super) const DISPLAY: &str = "display";
pub(super) const SEMIBOLD: &str = "semibold";
const SPLASH_SECONDS: f32 = 1.25;
const PAGE_FADE_SECONDS: f32 = 0.22;

pub(super) fn display() -> FontFamily {
    FontFamily::Name(DISPLAY.into())
}
pub(super) fn semibold() -> FontFamily {
    FontFamily::Name(SEMIBOLD.into())
}

/// Installs the bundled fonts in front of egui's defaults, which stay as fallback
/// for symbols outside the Latin subsets.
pub(super) fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    for (name, bytes) in [
        (
            "Inter",
            &include_bytes!("../assets/fonts/Inter-Regular.ttf")[..],
        ),
        (
            "InterSemiBold",
            &include_bytes!("../assets/fonts/Inter-SemiBold.ttf")[..],
        ),
        (
            "Unbounded",
            &include_bytes!("../assets/fonts/Unbounded-Bold.ttf")[..],
        ),
        (
            "JetBrainsMono",
            &include_bytes!("../assets/fonts/JetBrainsMono-Regular.ttf")[..],
        ),
    ] {
        fonts
            .font_data
            .insert(name.into(), Arc::new(egui::FontData::from_static(bytes)));
    }
    let fallback = fonts
        .families
        .get(&FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    let with = |first: &[&str]| -> Vec<String> {
        first
            .iter()
            .map(|name| (*name).to_owned())
            .chain(fallback.iter().cloned())
            .collect()
    };
    fonts
        .families
        .insert(FontFamily::Proportional, with(&["Inter"]));
    fonts
        .families
        .insert(display(), with(&["Unbounded", "InterSemiBold"]));
    fonts.families.insert(semibold(), with(&["InterSemiBold"]));
    fonts
        .families
        .entry(FontFamily::Monospace)
        .or_default()
        .insert(0, "JetBrainsMono".into());
    ctx.set_fonts(fonts);
}

fn ease_out(t: f32) -> f32 {
    1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(3)
}

/// A rotating ring segment; the wallet's only progress indicator.
pub(super) fn loader(ui: &mut egui::Ui, size: f32) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
    let time = ui.input(|input| input.time) as f32;
    let ink = ui.visuals().strong_text_color();
    let track = ui.visuals().widgets.noninteractive.bg_stroke.color;
    let center = rect.center();
    let radius = size * 0.42;
    let width = (size / 9.0).max(1.6);
    ui.painter()
        .circle_stroke(center, radius, Stroke::new(width, track));
    let start = time * 5.2;
    let sweep = 1.6 + (time * 2.4).sin() * 0.9;
    let points: Vec<egui::Pos2> = (0..=28)
        .map(|step| {
            let angle = start + sweep * step as f32 / 28.0;
            center + radius * egui::vec2(angle.cos(), angle.sin())
        })
        .collect();
    ui.painter()
        .add(egui::Shape::line(points, Stroke::new(width, ink)));
    ui.ctx().request_repaint();
    response
}

/// The launch splash: the mark scales in, the wordmark follows, then everything
/// fades into the wallet. Returns false once it is over.
pub(super) fn splash(ctx: &egui::Context, started: Instant, light: bool) -> bool {
    let t = started.elapsed().as_secs_f32();
    if t >= SPLASH_SECONDS {
        return false;
    }
    let screen = ctx.screen_rect();
    let fade = 1.0 - ease_out(((t - 0.85) / 0.4).max(0.0));
    let background = Color32::from_gray(if light { 246 } else { 5 });
    let ink = if light {
        Color32::BLACK
    } else {
        Color32::WHITE
    };
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new("splash"),
    ));
    painter.rect_filled(screen, 0.0, background.gamma_multiply(fade));
    let appear = ease_out(t / 0.55);
    let tile = 76.0 * (0.86 + 0.14 * appear);
    let center = screen.center() - egui::vec2(0.0, 26.0);
    let logo = egui::Rect::from_center_size(center, egui::vec2(tile, tile));
    super::icons::paint_logo_faded(&painter, logo, !light, appear * fade);
    let word = ease_out((t - 0.18) / 0.5);
    painter.text(
        center + egui::vec2(0.0, 66.0 + 8.0 * (1.0 - word)),
        egui::Align2::CENTER_CENTER,
        "G E N O R Y N",
        egui::FontId::new(20.0, semibold()),
        ink.gamma_multiply(word * fade),
    );
    let bar = egui::Rect::from_center_size(center + egui::vec2(0.0, 104.0), egui::vec2(140.0, 2.0));
    painter.rect_filled(bar, 1.0, ink.gamma_multiply(0.12 * fade));
    let progress = ease_out(t / 0.95);
    painter.rect_filled(
        egui::Rect::from_min_size(bar.min, egui::vec2(bar.width() * progress, 2.0)),
        1.0,
        ink.gamma_multiply(0.85 * fade),
    );
    ctx.request_repaint();
    true
}

/// Opacity and lift for content that just appeared; 1.0 when settled.
pub(super) fn page_fade(ctx: &egui::Context, since: Instant) -> f32 {
    let t = since.elapsed().as_secs_f32() / PAGE_FADE_SECONDS;
    if t < 1.0 {
        ctx.request_repaint();
    }
    ease_out(t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_fonts_cover_every_family_the_wallet_uses() {
        let ctx = egui::Context::default();
        install_fonts(&ctx);
        let _ = ctx.run(Default::default(), |ctx| {
            for family in [
                FontFamily::Proportional,
                FontFamily::Monospace,
                display(),
                semibold(),
            ] {
                let width = ctx.fonts(|fonts| {
                    fonts
                        .layout_no_wrap(
                            "GENORYN 1,234.5 GNR · ₿→".into(),
                            egui::FontId::new(16.0, family.clone()),
                            Color32::WHITE,
                        )
                        .size()
                        .x
                });
                assert!(width > 50.0, "{family:?}");
            }
        });
    }

    #[test]
    fn motion_settles() {
        assert_eq!(ease_out(0.0), 0.0);
        assert_eq!(ease_out(1.0), 1.0);
        assert_eq!(ease_out(7.0), 1.0, "clamped");
        assert!(ease_out(0.5) > 0.5, "decelerates");
    }
}
