//! The loupe: a circle around the pointer, and a larger circle beside it
//! showing what is inside the first magnified.
//!
//! The magnified picture is the image layer's — a third draw of the same
//! texture, placed by [`glass`] and cut to the circle in the shader — so
//! everything that applies to the image applies to it for nothing. Only
//! the two rings and the reticle at the glass's middle are drawn here, and
//! where the circles go is worked out here, by [`place`], for both halves
//! to agree on.

use egui::Stroke;

use crate::render::{Glass, Placement};

use super::chrome::Pass;
use super::{Rect, grid, minimap};

/// The radius of the glass, in logical pixels: the same at every
/// magnification, since it is the size of the thing beside the pointer,
/// and what changes with the magnification is how much of the picture fits
/// in it — the eye's radius, which is [`eye_radius`].
pub const GLASS_RADIUS: f32 = 80.0;

/// The magnifications on offer, in the order the wheel steps through them.
pub const MAGNIFICATIONS: [f32; 4] = [2.0, 4.0, 8.0, 16.0];

/// The one the loupe starts at.
pub const DEFAULT_MAGNIFICATION: f32 = 4.0;

/// The radius of the circle around the pointer at `magnification`: what
/// the glass shows at that magnification, so the glass shows exactly what
/// the eye rings and nothing more.
pub fn eye_radius(magnification: f32) -> f32 {
    GLASS_RADIUS / magnification
}

/// The magnification after `current`, `up` being the larger way, and
/// `current` itself at either end: the wheel steps rather than wraps, so
/// a notch too many does not throw the loupe to the other end.
pub fn step(current: f32, up: bool) -> f32 {
    let at = MAGNIFICATIONS
        .iter()
        .position(|&candidate| candidate >= current)
        .unwrap_or(MAGNIFICATIONS.len() - 1);
    let next = if up {
        (at + 1).min(MAGNIFICATIONS.len() - 1)
    } else {
        at.saturating_sub(1)
    };
    MAGNIFICATIONS[next]
}

/// The magnification after `current` round the ones on offer, the largest
/// followed by the smallest: what the key steps, where the wheel stops at
/// either end.
pub fn cycle(current: f32) -> f32 {
    let at = MAGNIFICATIONS
        .iter()
        .position(|&candidate| candidate >= current)
        .unwrap_or(MAGNIFICATIONS.len() - 1);
    MAGNIFICATIONS[(at + 1) % MAGNIFICATIONS.len()]
}

/// The magnification as the button beside the grid's reads it out.
pub fn label(magnification: f32) -> String {
    format!("{magnification}\u{00d7}")
}

/// The gap between the eye's ring and the glass's, so that the two read as
/// two things rather than as one touching itself.
const GAP: f32 = 16.0;

/// The rings' weight, in logical pixels, and their ink: the minimap's
/// border's, both of them — an outline around a picture the image layer
/// has drawn inside the picture, which is what the glass is — and not the
/// accent, which says what is switched on.
const STROKE: f32 = minimap::INSET_STROKE;

/// Where the loupe is, in the logical pixels the interface is laid out in:
/// the center of the eye — the pointer — and the center of the glass; and
/// how much larger the glass shows what the eye rings, which is what the
/// eye's radius follows.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Loupe {
    pub eye: [f32; 2],
    pub glass: [f32; 2],
    pub magnification: f32,
}

/// Where the loupe goes for a pointer at `cursor` over a content area of
/// `content`, at `magnification`: the eye on the pointer, and the glass
/// diagonally away from it — up and to the right, where the hand is least
/// likely to be over what it shows, and the other way on whichever axis
/// that would run it off the content area. The glass is then held inside
/// the area, which in a window too small to hold it beside the eye puts it
/// over the eye rather than off screen: a loupe half under a panel shows
/// half of what it is for.
pub fn place(cursor: [f32; 2], content: Rect, magnification: f32) -> Loupe {
    let reach = (eye_radius(magnification) + GAP + GLASS_RADIUS) / std::f32::consts::SQRT_2;
    let fits_right = cursor[0] + reach + GLASS_RADIUS <= content.right();
    let fits_up = cursor[1] - reach - GLASS_RADIUS >= content.y;
    let dx = if fits_right { reach } else { -reach };
    let dy = if fits_up { -reach } else { reach };
    Loupe {
        eye: cursor,
        glass: [
            within(
                cursor[0] + dx,
                content.x + GLASS_RADIUS,
                content.right() - GLASS_RADIUS,
            ),
            within(
                cursor[1] + dy,
                content.y + GLASS_RADIUS,
                content.bottom() - GLASS_RADIUS,
            ),
        ],
        magnification,
    }
}

/// `value` held between `low` and `high`, and the middle of the two where
/// there is no room between them at all — a window narrower than the glass
/// — since a clamp with its bounds crossed is a panic rather than an answer.
fn within(value: f32, low: f32, high: f32) -> f32 {
    if low > high {
        (low + high) / 2.0
    } else {
        value.clamp(low, high)
    }
}

/// The point of the image the loupe is about, in image pixels: the middle
/// of the pixel under the eye — the one the bar's readout reads — rather
/// than the point under the pointer itself, so that the pixel sits square
/// in the glass's middle, where the reticle frames it, and the glass moves
/// a pixel at a time.
fn under(loupe: Loupe, placement: Placement, scale: f32) -> [f32; 2] {
    let point = placement.image_point([loupe.eye[0] * scale, loupe.eye[1] * scale]);
    [point[0].floor() + 0.5, point[1].floor() + 0.5]
}

/// The glass's draw, in physical pixels: the image placed so that the
/// middle of the pixel under the eye lands at the glass's center, the
/// loupe's magnification times larger than `placement` shows it, and cut
/// to the glass's circle.
pub fn glass(loupe: Loupe, placement: Placement, scale: f32) -> Glass {
    let zoom = placement.zoom * loupe.magnification;
    let under = under(loupe, placement, scale);
    let center = [loupe.glass[0] * scale, loupe.glass[1] * scale];
    Glass {
        placement: Placement {
            x: center[0] - under[0] * zoom,
            y: center[1] - under[1] * zoom,
            width: placement.width * loupe.magnification,
            height: placement.height * loupe.magnification,
            zoom,
            upscale: placement.upscale,
        },
        center,
        radius: GLASS_RADIUS * scale,
    }
}

/// How large the pixel under the eye has to be in the glass, in logical
/// pixels, before the reticle frames it: below it the reticle is four ticks
/// around a gap, since a box around a pixel that small would be most of the
/// pixel's own size again and cover what is beside it.
const FRAME_FROM: f32 = 8.0;

/// The clear middle of the reticle's ticks while it frames nothing, from
/// the glass's center out, in logical pixels.
const TICK_GAP: f32 = 4.0;

/// How long each tick is, in logical pixels.
const TICK_LENGTH: f32 = 8.0;

/// How far inside the glass's rim the reticle stops, in logical pixels: a
/// frame or a tick that would reach closer is left off.
const RIM_MARGIN: f32 = 4.0;

/// The reticle's two inks: a light line one device pixel wide, edged on
/// each side by a dark one, so that it reads on a bright picture, a dark
/// one and noise alike. Fixed rather than the theme's, since it is drawn
/// on the picture rather than on the interface.
const RETICLE_LIGHT: egui::Color32 = egui::Color32::WHITE;
const RETICLE_DARK: egui::Color32 = egui::Color32::BLACK;

/// A rectangle in whole device pixels: left, top, right, bottom.
type Span = [f32; 4];

/// The reticle at the glass's middle, in device pixels, every edge on a
/// whole one: what is drawn dark, and the light over it.
#[derive(Debug, Default, PartialEq)]
struct Reticle {
    dark: Vec<Span>,
    light: Vec<Span>,
}

/// The reticle for a glass drawn as `glass`, on a display of `scale`.
///
/// Four ticks pointing at the glass's center, a gap between them; and once
/// the pixel there is [`FRAME_FROM`] across, a box around it as well — the
/// light line on the device pixels just outside the pixel's edges, the dark
/// one outside that, so none of the pixel itself is covered — with the
/// ticks starting at the box. A pixel too large for its box to fit inside
/// the glass gets no reticle: it is all the glass shows.
fn reticle(glass: &Glass, scale: f32) -> Reticle {
    let placement = glass.placement;
    let point = placement.image_point(glass.center);
    let pixel = [point[0].floor(), point[1].floor()];
    let [left, top] = placement.screen_point(pixel);
    let [right, bottom] = placement.screen_point([pixel[0] + 1.0, pixel[1] + 1.0]);
    let [cx, cy] = glass.center;
    let reach = glass.radius - RIM_MARGIN * scale;

    let mut reticle = Reticle::default();
    // Where each tick starts, measured outward from the center: right,
    // down, left, up.
    let gap = (TICK_GAP * scale).round();
    let mut starts = [
        cx.ceil() + gap,
        cy.ceil() + gap,
        cx.floor() - gap,
        cy.floor() - gap,
    ];
    if placement.zoom >= FRAME_FROM * scale {
        // The light line's outer edge: the pixels just outside the
        // pixel's, the one a ramped edge half covers counted as the
        // pixel's own.
        let light = [
            left.floor() - 1.0,
            top.floor() - 1.0,
            right.ceil() + 1.0,
            bottom.ceil() + 1.0,
        ];
        let dark = grow(light, 1.0);
        let corner = (dark[0] - cx)
            .abs()
            .max((dark[2] - cx).abs())
            .hypot((dark[1] - cy).abs().max((dark[3] - cy).abs()));
        if corner > reach {
            return reticle;
        }
        reticle.dark.extend(ring(dark));
        reticle.light.extend(ring(light));
        starts = [
            starts[0].max(dark[2]),
            starts[1].max(dark[3]),
            starts[2].min(dark[0]),
            starts[3].min(dark[1]),
        ];
    }

    let length = (TICK_LENGTH * scale).round().max(1.0);
    let (column, row) = (cx.floor(), cy.floor());
    let ticks = [
        (
            starts[0],
            (starts[0] + length).min((cx + reach).floor()),
            true,
        ),
        (
            starts[1],
            (starts[1] + length).min((cy + reach).floor()),
            false,
        ),
        (
            (starts[2] - length).max((cx - reach).ceil()),
            starts[2],
            true,
        ),
        (
            (starts[3] - length).max((cy - reach).ceil()),
            starts[3],
            false,
        ),
    ];
    for (from, to, across) in ticks {
        if to <= from {
            continue;
        }
        let tick = if across {
            [from, row, to, row + 1.0]
        } else {
            [column, from, column + 1.0, to]
        };
        reticle.dark.push(grow(tick, 1.0));
        reticle.light.push(tick);
    }
    reticle
}

/// `span` with `by` more on every side.
fn grow(span: Span, by: f32) -> Span {
    [span[0] - by, span[1] - by, span[2] + by, span[3] + by]
}

/// The one-pixel ring just inside `outer`'s edges, as four rectangles.
fn ring(outer: Span) -> [Span; 4] {
    let [left, top, right, bottom] = outer;
    [
        [left, top, right, top + 1.0],
        [left, bottom - 1.0, right, bottom],
        [left, top + 1.0, left + 1.0, bottom - 1.0],
        [right - 1.0, top + 1.0, right, bottom - 1.0],
    ]
}

/// Draws the two rings over the picture, and over the glass the image layer
/// has already put down: the eye's around the pixel under the pointer, and
/// the glass's rim around the magnified picture; and the reticle at the
/// glass's middle. Painted straight on the picture's painter,
/// as the region is, so that the picture under the loupe keeps the pointer.
///
/// Not over the minimap: the image layer draws the thumbnail over the glass,
/// and the rings are clipped around the thumbnail's rectangle to match, so
/// the whole loupe goes under the map. Clipped rather than layered, since
/// the thumbnail is the image layer's and nothing egui stacks over the rings
/// covers it.
pub(super) fn show(pass: &Pass, ui: &mut egui::Ui) {
    let (Some(loupe), Some(current)) = (pass.input.loupe, pass.current) else {
        return;
    };
    // Through the placement the image layer draws the glass with, so that
    // the reticle lands on the pixels it frames, and the eye rings the
    // middle of the pixel the glass is about rather than the pointer.
    let scale = pass.input.scale;
    let placement = pass.view.placement(current.size(), pass.input.viewport);
    let glass = glass(loupe, placement, scale);
    let reticle = reticle(&glass, scale);
    let eye = placement.screen_point(under(loupe, placement, scale));
    let eye = egui::pos2(eye[0] / scale, eye[1] / scale);
    let logical = |span: Span| {
        egui::Rect::from_min_max(
            egui::pos2(span[0] / scale, span[1] / scale),
            egui::pos2(span[2] / scale, span[3] / scale),
        )
    };
    let thumbnail = pass
        .input
        .minimap_on_screen
        .then(|| minimap::thumbnail(pass.content, current.size()))
        .flatten();
    let grid = pass.grid;
    let ink: egui::Color32 = pass.theme.inset_edge.into();
    let stroke = Stroke::new(grid.line_width(STROKE), ink);
    // egui lays a circle's stroke outside its radius; each ring is drawn
    // half a stroke in so that it straddles its circle, the glass's edge
    // — the pixel the image layer feathers — running down its middle.
    let inset = stroke.width / 2.0;
    for piece in grid::pieces(pass.content, &[thumbnail]) {
        let painter = ui.painter().with_clip_rect(piece.into());
        painter.circle_stroke(eye, eye_radius(loupe.magnification) - inset, stroke);
        painter.circle_stroke(
            egui::pos2(loupe.glass[0], loupe.glass[1]),
            GLASS_RADIUS - inset,
            stroke,
        );
        for (spans, ink) in [
            (&reticle.dark, RETICLE_DARK),
            (&reticle.light, RETICLE_LIGHT),
        ] {
            for &span in spans {
                painter.rect_filled(logical(span), 0.0, ink);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::Upscale;

    fn content() -> Rect {
        Rect::new(30.0, 30.0, 940.0, 640.0)
    }

    /// How far apart the two centers are, when nothing is in the way.
    fn reach() -> f32 {
        eye_radius(DEFAULT_MAGNIFICATION) + GAP + GLASS_RADIUS
    }

    fn place(cursor: [f32; 2], content: Rect) -> Loupe {
        super::place(cursor, content, DEFAULT_MAGNIFICATION)
    }

    fn distance(a: [f32; 2], b: [f32; 2]) -> f32 {
        ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
    }

    /// The glass keeps its distance from the eye, up and to the right of
    /// it, and goes the other way on an axis where that would run it off
    /// the content area — never off the area itself.
    #[test]
    fn the_glass_sits_beside_the_eye_and_inside_the_content_area() {
        let content = content();
        let middle = place([500.0, 350.0], content);
        assert_eq!(middle.eye, [500.0, 350.0]);
        assert!((distance(middle.eye, middle.glass) - reach()).abs() < 1e-3);
        assert!(middle.glass[0] > middle.eye[0] && middle.glass[1] < middle.eye[1]);

        // Near the right edge it goes left; near the top it goes down;
        // in the corner, both.
        let right = place([content.right() - 40.0, 350.0], content);
        assert!(right.glass[0] < right.eye[0] && right.glass[1] < right.eye[1]);
        let top = place([500.0, content.y + 40.0], content);
        assert!(top.glass[0] > top.eye[0] && top.glass[1] > top.eye[1]);
        let corner = place([content.right() - 40.0, content.y + 40.0], content);
        assert!(corner.glass[0] < corner.eye[0] && corner.glass[1] > corner.eye[1]);
        for placed in [middle, right, top, corner] {
            assert!((distance(placed.eye, placed.glass) - reach()).abs() < 1e-3);
        }

        // Wherever the pointer is, the glass is whole inside the area.
        for x in [content.x, content.x + 1.0, 500.0, content.right() - 1.0] {
            for y in [content.y, 350.0, content.bottom() - 1.0] {
                let placed = place([x, y], content);
                let [gx, gy] = placed.glass;
                assert!(gx - GLASS_RADIUS >= content.x - 1e-3, "{placed:?}");
                assert!(gx + GLASS_RADIUS <= content.right() + 1e-3, "{placed:?}");
                assert!(gy - GLASS_RADIUS >= content.y - 1e-3, "{placed:?}");
                assert!(gy + GLASS_RADIUS <= content.bottom() + 1e-3, "{placed:?}");
            }
        }

        // A content area too small for the glass at all puts it in the
        // middle rather than panicking over a clamp with no room in it.
        let tiny = Rect::new(0.0, 0.0, 100.0, 100.0);
        assert_eq!(place([50.0, 50.0], tiny).glass, [50.0, 50.0]);
    }

    /// The glass shows what the eye rings: the middle of the pixel under the
    /// pointer is at the glass's center, at the magnification, and the
    /// circle is the glass's in physical pixels.
    #[test]
    fn the_glass_magnifies_the_point_under_the_eye() {
        let placement = Placement {
            x: 100.0,
            y: 60.0,
            width: 800.0,
            height: 600.0,
            zoom: 2.0,
            upscale: Upscale::Bicubic,
        };
        let scale = 2.0;
        let loupe = Loupe {
            eye: [200.0, 150.0],
            glass: [300.0, 80.0],
            magnification: 4.0,
        };
        let glass = glass(loupe, placement, scale);
        assert_eq!(glass.center, [600.0, 160.0]);
        assert_eq!(glass.radius, GLASS_RADIUS * scale);
        assert_eq!(glass.placement.zoom, placement.zoom * 4.0);
        assert_eq!(glass.placement.upscale, Upscale::Bicubic);
        assert_eq!(glass.placement.width, placement.width * 4.0);

        // The pointer is on pixel (150, 120), at its corner; the glass is
        // about the pixel's middle.
        assert_eq!(placement.image_point([400.0, 300.0]), [150.0, 120.0]);
        let under_glass = glass.placement.image_point(glass.center);
        assert!((under_glass[0] - 150.5).abs() < 1e-3);
        assert!((under_glass[1] - 120.5).abs() < 1e-3);

        // A device pixel from the eye is four from the center.
        let beside = glass
            .placement
            .image_point([glass.center[0] + 4.0, glass.center[1]]);
        assert!((beside[0] - (150.5 + 1.0 / placement.zoom)).abs() < 1e-3);

        // Anywhere on the pixel, the glass is the same.
        let elsewhere = super::glass(
            Loupe {
                eye: [200.7, 150.9],
                ..loupe
            },
            placement,
            scale,
        );
        assert_eq!(elsewhere.placement.x, glass.placement.x);
        assert_eq!(elsewhere.placement.y, glass.placement.y);
    }

    /// The glass is one size at every magnification, and the eye is the
    /// glass's radius over it: what the glass shows at the magnification.
    /// The wheel steps from one magnification to the next and stops at the
    /// ends; the button reads it out as a multiplier.
    #[test]
    fn the_magnification_sizes_the_eye_and_steps_between_the_offered_ones() {
        assert_eq!(eye_radius(2.0), 40.0);
        assert_eq!(eye_radius(4.0), 20.0);
        assert_eq!(eye_radius(16.0), 5.0);
        for magnification in MAGNIFICATIONS {
            let loupe = super::place([500.0, 350.0], content(), magnification);
            let glass = glass(
                loupe,
                Placement {
                    x: 0.0,
                    y: 0.0,
                    width: 1000.0,
                    height: 700.0,
                    zoom: 1.0,
                    upscale: Upscale::Nearest,
                },
                1.0,
            );
            assert_eq!(glass.radius, GLASS_RADIUS);
            assert_eq!(glass.placement.zoom, magnification);
            // A point an eye's radius from the pixel's middle lands at the
            // glass's rim.
            let middle = loupe.eye[0] + 0.5;
            let rim = glass
                .placement
                .screen_point([middle + eye_radius(magnification), loupe.eye[1]]);
            assert!((rim[0] - (glass.center[0] + GLASS_RADIUS)).abs() < 1e-3);
        }

        assert_eq!(step(2.0, true), 4.0);
        assert_eq!(step(4.0, true), 8.0);
        assert_eq!(step(16.0, true), 16.0);
        assert_eq!(step(16.0, false), 8.0);
        assert_eq!(step(2.0, false), 2.0);
        // A magnification between two offered lands on the next up or down.
        assert_eq!(step(3.0, true), 8.0);
        assert_eq!(step(3.0, false), 2.0);

        assert_eq!(cycle(2.0), 4.0);
        assert_eq!(cycle(8.0), 16.0);
        assert_eq!(cycle(16.0), 2.0);
        assert_eq!(cycle(3.0), 8.0);

        assert_eq!(label(4.0), "4\u{00d7}");
        assert_eq!(label(16.0), "16\u{00d7}");
    }

    fn glass_at(zoom: f32, magnification: f32, scale: f32, eye: [f32; 2]) -> Glass {
        let placement = Placement {
            x: 13.0,
            y: 7.0,
            width: 640.0 * zoom,
            height: 480.0 * zoom,
            zoom,
            upscale: Upscale::Nearest,
        };
        let loupe = Loupe {
            eye,
            glass: [eye[0] + 120.0, eye[1] - 120.0],
            magnification,
        };
        glass(loupe, placement, scale)
    }

    fn covers(span: Span, x: f32, y: f32) -> bool {
        span[0] <= x && x < span[2] && span[1] <= y && y < span[3]
    }

    /// Every edge of the reticle is on a whole device pixel.
    fn on_the_grid(reticle: &Reticle) {
        for span in reticle.dark.iter().chain(&reticle.light) {
            for edge in span {
                assert_eq!(edge.fract(), 0.0, "{reticle:?}");
            }
        }
    }

    /// With the pixel small in the glass, the reticle is four ticks around
    /// a clear middle, each one light line edged in dark, and nothing near
    /// the center.
    #[test]
    fn a_small_pixel_gets_four_ticks_around_a_gap() {
        for scale in [1.0, 1.25, 2.0] {
            let glass = glass_at(1.0, 4.0, scale, [301.3, 222.6]);
            let reticle = reticle(&glass, scale);
            on_the_grid(&reticle);
            assert_eq!(reticle.light.len(), 4);
            assert_eq!(reticle.dark.len(), 4);
            for (light, dark) in reticle.light.iter().zip(&reticle.dark) {
                assert_eq!(*dark, grow(*light, 1.0));
                assert!(light[2] - light[0] == 1.0 || light[3] - light[1] == 1.0);
            }
            let [cx, cy] = glass.center;
            let gap = TICK_GAP * scale - 1.0;
            for span in &reticle.light {
                for (x, y) in [(cx, cy), (cx + gap, cy), (cx, cy - gap)] {
                    assert!(!covers(*span, x, y), "{span:?} at {scale}");
                }
            }
        }
    }

    /// With the pixel large in the glass, a box rings it on the device
    /// pixels just outside its edges, covering none of it, and the ticks
    /// start outside the box.
    #[test]
    fn a_large_pixel_is_framed_without_covering_it() {
        for (zoom, scale) in [(4.0, 1.0), (3.3, 1.25), (2.0, 2.0)] {
            let glass = glass_at(zoom, 16.0, scale, [301.3, 222.6]);
            let reticle = reticle(&glass, scale);
            on_the_grid(&reticle);
            assert_eq!(reticle.light.len(), 8);
            let placement = glass.placement;
            let point = placement.image_point(glass.center);
            let pixel = [point[0].floor(), point[1].floor()];
            let [left, top] = placement.screen_point(pixel);
            let [right, bottom] = placement.screen_point([pixel[0] + 1.0, pixel[1] + 1.0]);
            // Nothing over the pixel, ramped edges included.
            for span in reticle.dark.iter().chain(&reticle.light) {
                let clear = span[2] <= left.floor()
                    || span[0] >= right.ceil()
                    || span[3] <= top.floor()
                    || span[1] >= bottom.ceil();
                assert!(clear, "{span:?} over the pixel at zoom {zoom}");
            }
            // The light line touches the pixel on all four sides.
            let [cx, cy] = glass.center;
            for (x, y) in [
                (left.floor() - 0.5, cy),
                (right.ceil() + 0.5, cy),
                (cx, top.floor() - 0.5),
                (cx, bottom.ceil() + 0.5),
            ] {
                assert!(reticle.light.iter().any(|span| covers(*span, x, y)));
            }
        }
    }

    /// A pixel whose box would not fit inside the glass is all the glass
    /// shows, and gets no reticle; nor does any tick reach the rim.
    #[test]
    fn the_reticle_stays_inside_the_glass() {
        let glass = glass_at(16.0, 16.0, 1.0, [301.3, 222.6]);
        assert_eq!(reticle(&glass, 1.0), Reticle::default());
        for zoom in [0.25, 1.0, 1.5, 2.0] {
            for magnification in MAGNIFICATIONS {
                let glass = glass_at(zoom, magnification, 1.0, [301.3, 222.6]);
                let reach = glass.radius - RIM_MARGIN;
                for span in reticle(&glass, 1.0).dark {
                    for x in [span[0], span[2]] {
                        for y in [span[1], span[3]] {
                            let distance = (x - glass.center[0]).hypot(y - glass.center[1]);
                            assert!(distance <= reach + 2.0, "{span:?}");
                        }
                    }
                }
            }
        }
    }
}
