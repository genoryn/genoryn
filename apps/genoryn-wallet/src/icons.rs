//! Monochrome line icons drawn as vectors, so they stay sharp at every display
//! scale and need no font glyphs or bundled image files.
use eframe::egui::{Color32, Painter, Rect, Shape, Stroke, StrokeKind, pos2, vec2};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Icon {
    Overview,
    Send,
    Receive,
    Transactions,
    Mining,
    Node,
    Network,
    Settings,
    Wallets,
    Verify,
    Security,
}

/// Indexed like `SECTIONS`.
pub(super) const SECTION_ICONS: [Icon; 11] = [
    Icon::Overview,
    Icon::Send,
    Icon::Receive,
    Icon::Transactions,
    Icon::Mining,
    Icon::Node,
    Icon::Network,
    Icon::Settings,
    Icon::Wallets,
    Icon::Verify,
    Icon::Security,
];

/// Paints `icon` inside `rect` (drawn on a unit grid, scaled to the rect).
pub(super) fn paint(painter: &Painter, rect: Rect, icon: Icon, color: Color32) {
    let stroke = Stroke::new((rect.width() / 11.0).max(1.4), color);
    let at = |x: f32, y: f32| rect.min + vec2(x * rect.width(), y * rect.height());
    let line = |points: &[(f32, f32)]| {
        painter.add(Shape::line(
            points.iter().map(|&(x, y)| at(x, y)).collect(),
            stroke,
        ));
    };
    let curve = |cx: f32, cy: f32, rx: f32, ry: f32, from: f32, to: f32| {
        let steps = 32;
        let points = (0..=steps)
            .map(|step| {
                let angle = (from + (to - from) * step as f32 / steps as f32).to_radians();
                at(cx + rx * angle.cos(), cy + ry * angle.sin())
            })
            .collect();
        painter.add(Shape::line(points, stroke));
    };
    let scale = rect.width();
    match icon {
        Icon::Overview => {
            for (x, y) in [(0.1, 0.1), (0.56, 0.1), (0.1, 0.56), (0.56, 0.56)] {
                painter.rect_stroke(
                    Rect::from_min_max(at(x, y), at(x + 0.34, y + 0.34)),
                    scale * 0.07,
                    stroke,
                    StrokeKind::Middle,
                );
            }
        }
        Icon::Send => {
            line(&[(0.2, 0.8), (0.8, 0.2)]);
            line(&[(0.36, 0.2), (0.8, 0.2), (0.8, 0.64)]);
        }
        Icon::Receive => {
            line(&[(0.8, 0.2), (0.2, 0.8)]);
            line(&[(0.2, 0.36), (0.2, 0.8), (0.64, 0.8)]);
        }
        Icon::Transactions => {
            line(&[(0.1, 0.32), (0.88, 0.32)]);
            line(&[(0.7, 0.14), (0.88, 0.32), (0.7, 0.5)]);
            line(&[(0.9, 0.7), (0.12, 0.7)]);
            line(&[(0.3, 0.52), (0.12, 0.7), (0.3, 0.88)]);
        }
        Icon::Mining => {
            let hexagon = [
                (0.5, 0.08),
                (0.88, 0.29),
                (0.88, 0.71),
                (0.5, 0.92),
                (0.12, 0.71),
                (0.12, 0.29),
            ];
            painter.add(Shape::closed_line(
                hexagon.iter().map(|&(x, y)| at(x, y)).collect(),
                stroke,
            ));
            line(&[(0.12, 0.29), (0.5, 0.5), (0.88, 0.29)]);
            line(&[(0.5, 0.5), (0.5, 0.92)]);
        }
        Icon::Node => {
            for top in [0.12, 0.56] {
                painter.rect_stroke(
                    Rect::from_min_max(at(0.1, top), at(0.9, top + 0.32)),
                    scale * 0.06,
                    stroke,
                    StrokeKind::Middle,
                );
                painter.circle_filled(at(0.72, top + 0.16), stroke.width * 0.9, color);
                line(&[(0.24, top + 0.16), (0.48, top + 0.16)]);
            }
        }
        Icon::Network => {
            painter.circle_stroke(at(0.5, 0.5), scale * 0.4, stroke);
            curve(0.5, 0.5, 0.17, 0.4, 0.0, 360.0);
            line(&[(0.1, 0.5), (0.9, 0.5)]);
        }
        Icon::Settings => {
            let center = at(0.5, 0.5);
            let teeth = Stroke::new(stroke.width * 1.9, color);
            for tooth in 0..8 {
                let angle = (tooth as f32 * 45.0).to_radians();
                let direction = vec2(angle.cos(), angle.sin()) * scale;
                painter.line_segment([center + direction * 0.3, center + direction * 0.44], teeth);
            }
            painter.circle_stroke(center, scale * 0.3, stroke);
            painter.circle_stroke(center, scale * 0.11, stroke);
        }
        Icon::Security => {
            curve(0.5, 0.42, 0.22, 0.26, 180.0, 360.0);
            line(&[(0.28, 0.42), (0.28, 0.46)]);
            line(&[(0.72, 0.42), (0.72, 0.46)]);
            painter.rect_stroke(
                Rect::from_min_max(at(0.14, 0.46), at(0.86, 0.9)),
                scale * 0.08,
                stroke,
                StrokeKind::Middle,
            );
            painter.circle_filled(at(0.5, 0.63), stroke.width * 1.1, color);
            line(&[(0.5, 0.66), (0.5, 0.76)]);
        }
        Icon::Wallets => {
            line(&[(0.18, 0.3), (0.7, 0.12), (0.76, 0.3)]);
            painter.rect_stroke(
                Rect::from_min_max(at(0.08, 0.3), at(0.92, 0.88)),
                scale * 0.08,
                stroke,
                StrokeKind::Middle,
            );
            painter.rect_stroke(
                Rect::from_min_max(at(0.62, 0.49), at(0.92, 0.69)),
                scale * 0.05,
                stroke,
                StrokeKind::Middle,
            );
            painter.circle_filled(at(0.73, 0.59), stroke.width * 0.8, color);
        }
        Icon::Verify => {
            painter.add(Shape::closed_line(
                [
                    (0.5, 0.06),
                    (0.88, 0.2),
                    (0.84, 0.56),
                    (0.5, 0.94),
                    (0.16, 0.56),
                    (0.12, 0.2),
                ]
                .iter()
                .map(|&(x, y)| at(x, y))
                .collect(),
                stroke,
            ));
            line(&[(0.32, 0.5), (0.45, 0.63), (0.69, 0.37)]);
        }
    }
}

// The GENORYN sign on a 256 grid, mirrored from scripts/brand/render_logo.py.
const GRID: f32 = 256.0;
const RADIUS: f32 = 58.0;
const STROKE: f32 = 30.0;
const OPENING_END: f32 = 318.0;
const BAR_START: f32 = 134.0;
const TICK_WIDTH: f32 = 14.0;
const TICK_OFFSET: f32 = 15.0;
const TICK_OVERHANG: f32 = 24.0;

/// Paints the GNR currency sign, a G crossed by two short vertical strokes, filling
/// `rect` as the logo's 256-unit grid does.
pub(super) fn paint_sign(painter: &Painter, rect: Rect, color: Color32) {
    let unit = rect.width().min(rect.height()) / GRID;
    let origin = rect.center() - vec2(GRID / 2.0, GRID / 2.0) * unit;
    let at = |x: f32, y: f32| origin + vec2(x, y) * unit;
    let c = GRID / 2.0;
    let steps = 48;
    let ring = (0..=steps)
        .map(|step| {
            let angle = (OPENING_END * step as f32 / steps as f32).to_radians();
            at(c + RADIUS * angle.cos(), c + RADIUS * angle.sin())
        })
        .collect();
    painter.add(Shape::line(ring, Stroke::new(STROKE * unit, color)));
    let outer = RADIUS + STROKE / 2.0;
    let inner = RADIUS - STROKE / 2.0;
    painter.rect_filled(
        Rect::from_min_max(
            at(BAR_START, c - STROKE / 2.0),
            at(c + outer, c + STROKE / 2.0),
        ),
        0.0,
        color,
    );
    for center in [c - TICK_OFFSET, c + TICK_OFFSET] {
        let (left, right) = (center - TICK_WIDTH / 2.0, center + TICK_WIDTH / 2.0);
        for (top, bottom) in [
            (c - outer - TICK_OVERHANG, c - inner),
            (c + inner, c + outer + TICK_OVERHANG),
        ] {
            painter.rect_filled(
                Rect::from_min_max(at(left, top), at(right, bottom)),
                2.0 * unit,
                color,
            );
        }
    }
}

/// The app mark: the sign on a rounded tile, as in the window and file icon.
pub(super) fn paint_logo(painter: &Painter, rect: Rect, dark: bool) {
    paint_logo_faded(painter, rect, dark, 1.0);
}
pub(super) fn paint_logo_faded(painter: &Painter, rect: Rect, dark: bool, opacity: f32) {
    let corner = rect.width() * 56.0 / GRID;
    painter.rect_filled(rect, corner, Color32::from_gray(11).gamma_multiply(opacity));
    if dark {
        painter.rect_stroke(
            rect,
            corner,
            Stroke::new(1.0, Color32::from_gray(48).gamma_multiply(opacity)),
            StrokeKind::Inside,
        );
    }
    paint_sign(
        painter,
        rect,
        Color32::from_gray(242).gamma_multiply(opacity),
    );
}

/// The appearance toggle: a sun in light mode, a crescent moon in dark mode.
pub(super) fn paint_theme(
    painter: &Painter,
    rect: Rect,
    light: bool,
    ink: Color32,
    background: Color32,
) {
    let center = rect.center();
    let radius = rect.width() * 0.22;
    let stroke = Stroke::new((rect.width() / 11.0).max(1.4), ink);
    if light {
        painter.circle_stroke(center, radius, stroke);
        for ray in 0..8 {
            let angle = (ray as f32 * 45.0).to_radians();
            let direction = vec2(angle.cos(), angle.sin());
            painter.line_segment(
                [
                    center + direction * rect.width() * 0.34,
                    center + direction * rect.width() * 0.46,
                ],
                stroke,
            );
        }
    } else {
        let moon = rect.width() * 0.34;
        painter.circle_filled(center, moon, ink);
        painter.circle_filled(
            center + vec2(moon * 0.55, -moon * 0.45),
            moon * 0.9,
            background,
        );
    }
}

/// A small helper for callers that position icons by center.
pub(super) fn square(center_x: f32, center_y: f32, size: f32) -> Rect {
    Rect::from_center_size(pos2(center_x, center_y), vec2(size, size))
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::{Context, Id, LayerId, Order};

    #[test]
    fn every_section_has_a_distinct_icon_that_paints() {
        let unique = SECTION_ICONS
            .iter()
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(unique.len(), crate::SECTIONS.len());
        let ctx = Context::default();
        let _ = ctx.run(Default::default(), |ctx| {
            let painter = ctx.layer_painter(LayerId::new(Order::Foreground, Id::new("icons")));
            for icon in SECTION_ICONS {
                paint(&painter, square(20.0, 20.0, 18.0), icon, Color32::WHITE);
            }
            paint_sign(&painter, square(20.0, 20.0, 14.0), Color32::WHITE);
            paint_logo(&painter, square(40.0, 40.0, 44.0), true);
        });
    }
}
