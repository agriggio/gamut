//! Which way the camera faced, drawn: a camera in three dimensions over a
//! level floor, pitched and rolled as the file says it was, for the info
//! panel's Orientation section; and the level line drawn across the picture
//! while the pointer is on that section.
//!
//! The camera is the picture as shown given a body: its back is as wide
//! and as tall as the picture, so a portrait is drawn as a camera held on
//! its side, and the lens comes out of its front. It is drawn by hand with
//! the painter, each face a flat polygon shaded by how squarely it faces
//! the light, the faces turned away from the eye left out and the rest
//! painted from the farthest in — enough for a few convex boxes.

use egui::{Color32, Pos2, Stroke, pos2, vec2};

use crate::image::attitude::Shown;
use crate::render::Color;

use super::Current;
use super::chrome::Pass;
use super::icon;
use crate::theme::Theme;

/// The diagram's height in the column.
pub(super) const HEIGHT: f32 = 132.0;

/// The world is the camera's own axes held level: x across to the right,
/// y down and z ahead. The eye looks at it from behind the camera and to its
/// right, and from above: turned this far about the vertical, so the lens
/// points away to the right of the eye ...
const YAW: f32 = 0.95;
/// ... and tipped this far, so the floor is seen from above.
const ELEVATION: f32 = 0.38;

/// How far the floor is below the camera's center, how far out it runs
/// either way, and how many lines cross it, in the units the body's longer
/// side is two of.
const FLOOR_DROP: f32 = 1.6;
const FLOOR_REACH: f32 = 2.2;
const FLOOR_LINES: usize = 5;

/// The body's depth, and the lens's length and radius against the body's
/// shorter side; the button on top's size and where it stands along it.
const BODY_DEPTH: f32 = 0.55;
const LENS_LENGTH: f32 = 0.7;
const LENS_RADIUS: f32 = 0.36;
const LENS_SIDES: usize = 24;
const BUTTON: [f32; 3] = [0.28, 0.14, 0.24];
const BUTTON_ALONG: f32 = 0.55;

/// The camera's back, which looks out along -z.
const BACK: Point = [0.0, 0.0, -1.0];
/// The screen on the back, as a share of the back's size.
const SCREEN: f32 = 0.72;

/// How far ahead of the lens the line it looks along runs.
const SIGHT: f32 = 1.6;

/// The most a picture's sides are drawn apart, so a panorama is still a
/// camera rather than a plank.
const MAX_ASPECT: f32 = 2.0;

/// How far across the diagram the scene runs, in the units above: what
/// sets its scale, kept the same whichever way the camera points so that
/// one picture's camera and the next one's are drawn the same size.
const SCENE_SPAN: f32 = 3.8;

/// The weight of the camera's edges and of the line it looks along, and of
/// the horizon drawn across the picture.
const EDGE: f32 = 1.0;
const HORIZON_LINE: f32 = 1.5;
/// The dashes the horizon and the line of sight are drawn in.
const DASH: f32 = 6.0;
const GAP: f32 = 4.0;
/// The lighter line through the middle that says only which way is level,
/// where the horizon is not on the picture or has no place: as heavy as the
/// horizon, in shorter dashes, and this share of the accent.
const LEVEL_DASH: f32 = 3.0;
const LEVEL_GAP: f32 = 3.0;
const LEVEL_INK: f32 = 0.7;

/// How squarely a face must face the eye to be drawn: the cosine of the
/// angle between its outward way and the way back to the eye.
const EDGE_ON: f32 = 0.03;

/// A point of the scene.
type Point = [f32; 3];

/// One flat face of a solid, in the camera's axes: its corners in order
/// round it, and the way it looks out.
struct Face {
    corners: Vec<Point>,
    outward: Point,
}

/// Draws the camera as `shown` says it faced into `rect`, its body the
/// shape of a picture `size` pixels across and down.
pub(super) fn diagram(
    theme: &Theme,
    grid: icon::Grid,
    painter: &egui::Painter,
    rect: egui::Rect,
    shown: &Shown,
    size: [f32; 2],
) {
    let axes = camera_axes(shown.gravity);
    let scale = rect.width().min(rect.height() * 1.4) / SCENE_SPAN;
    let center = rect.center() + vec2(-0.3 * scale, -0.35 * scale);
    let eye = |point: Point| view(point);
    let screen = |point: Point| {
        let [x, y, _] = eye(point);
        pos2(center.x + x * scale, center.y + y * scale)
    };

    // The floor: lines across it each way, dim, and its edge.
    let dim: Color32 = theme.text_dim.into();
    let floor_stroke = Stroke::new(grid.line_width(EDGE), dim.gamma_multiply(0.45));
    for line in 0..FLOOR_LINES {
        let at = -FLOOR_REACH + 2.0 * FLOOR_REACH * line as f32 / (FLOOR_LINES - 1) as f32;
        for ends in [
            [
                [at, FLOOR_DROP, -FLOOR_REACH],
                [at, FLOOR_DROP, FLOOR_REACH],
            ],
            [
                [-FLOOR_REACH, FLOOR_DROP, at],
                [FLOOR_REACH, FLOOR_DROP, at],
            ],
        ] {
            painter.line_segment([screen(ends[0]), screen(ends[1])], floor_stroke);
        }
    }
    // Gravity, straight down from the camera to the floor.
    painter.extend(egui::Shape::dashed_line(
        &[screen([0.0, 0.0, 0.0]), screen([0.0, FLOOR_DROP, 0.0])],
        Stroke::new(grid.line_width(EDGE), dim),
        DASH / 2.0,
        GAP / 2.0,
    ));

    // The solids, in the camera's axes and then carried into the world's.
    let aspect = (size[0] / size[1].max(1.0)).clamp(1.0 / MAX_ASPECT, MAX_ASPECT);
    let half = if aspect >= 1.0 {
        [1.0, 1.0 / aspect]
    } else {
        [aspect, 1.0]
    };
    let short = half[0].min(half[1]);
    let mut faces = cuboid([0.0, 0.0, 0.0], [half[0], half[1], BODY_DEPTH / 2.0]);
    // The button on top says which way is up: the camera's top is its -y.
    faces.extend(cuboid(
        [BUTTON_ALONG * half[0], -half[1] - BUTTON[1] / 2.0, 0.0],
        [BUTTON[0] / 2.0 * short, BUTTON[1] / 2.0, BUTTON[2] / 2.0],
    ));
    faces.extend(cylinder(
        BODY_DEPTH / 2.0,
        BODY_DEPTH / 2.0 + LENS_LENGTH * short,
        LENS_RADIUS * short,
    ));
    let world = |point: Point| carry(axes, point);

    let accent: Color32 = theme.accent.into();
    let edge = Stroke::new(grid.line_width(EDGE), accent);
    let mut drawn: Vec<(f32, Vec<Pos2>, Color32, bool)> = faces
        .iter()
        .filter_map(|face| {
            let corners: Vec<Point> = face.corners.iter().map(|&point| world(point)).collect();
            let normal = world(face.outward);
            let seen = view(normal);
            // Facing away from the eye, which looks along +z, or so nearly
            // edge-on that it is a sliver, whose sharp corners would be
            // stroked as spikes.
            if seen[2] >= -EDGE_ON {
                return None;
            }
            let depth =
                corners.iter().map(|&point| eye(point)[2]).sum::<f32>() / corners.len() as f32;
            let light = dot(normal, LIGHT).max(0.0);
            let fill = mix(theme.bar_background, theme.accent, 0.12 + 0.5 * light);
            let mut points: Vec<Pos2> = corners.into_iter().map(screen).collect();
            // Clockwise on screen, which is the way round the painter
            // feathers a polygon's edge outward.
            if signed_area(&points) < 0.0 {
                points.reverse();
            }
            Some((depth, points, fill, face.outward == BACK))
        })
        .collect();
    drawn.sort_by(|a, b| b.0.total_cmp(&a.0));
    // The line the lens looks along, behind the camera where it is behind
    // it and in front where it is in front: drawn first, and the solids
    // over it.
    let lens_front = BODY_DEPTH / 2.0 + LENS_LENGTH * short;
    let sight = [
        screen(world([0.0, 0.0, lens_front])),
        screen(world([0.0, 0.0, lens_front + SIGHT])),
    ];
    painter.extend(egui::Shape::dashed_line(&sight, edge, DASH, GAP));
    for (_, points, fill, back) in drawn {
        // The back wears a screen, which says which side of the box the
        // eye is looking at.
        let screen = back.then(|| {
            let middle = points
                .iter()
                .fold(vec2(0.0, 0.0), |sum, point| sum + point.to_vec2())
                / points.len() as f32;
            points
                .iter()
                .map(|point| (middle + (point.to_vec2() - middle) * SCREEN).to_pos2())
                .collect::<Vec<Pos2>>()
        });
        painter.add(egui::Shape::convex_polygon(
            points.clone(),
            fill,
            Stroke::NONE,
        ));
        outline(painter, &points, edge);
        if let Some(screen) = screen {
            let glass = mix(theme.bar_background, theme.accent, 0.05);
            painter.add(egui::Shape::convex_polygon(
                screen.clone(),
                glass,
                Stroke::NONE,
            ));
            outline(painter, &screen, Stroke::new(edge.width, dim));
        }
    }
}

/// Strokes round `points` edge by edge, each corner rounded. A closed
/// polygon's own stroke joins its corners with miters that egui does not
/// limit, so a sharp corner seen in perspective grows a spike.
fn outline(painter: &egui::Painter, points: &[Pos2], stroke: Stroke) {
    for (&a, &b) in points.iter().zip(points.iter().cycle().skip(1)) {
        painter.line_segment([a, b], stroke);
    }
    for &point in points {
        painter.circle_filled(point, stroke.width / 2.0, stroke.color);
    }
}

/// Where the light comes from: above, to the left and behind the eye.
const LIGHT: Point = [-0.36, -0.8, -0.48];

/// The camera's three axes — across, down and ahead — in the world's,
/// from gravity in the camera's, a unit vector. Gravity in the world pulls
/// straight down +y, so each axis's y is how far it lies along gravity; the
/// lens is pointed straight ahead, +z, as far as gravity leaves it free to
/// be, and across is square to it and level with it as far as the roll
/// allows, on whichever side keeps down agreeing with gravity.
fn camera_axes(gravity: [f64; 3]) -> [Point; 3] {
    let [gx, gy, gz] = gravity.map(|value| value as f32);
    let level = (1.0 - gz * gz).max(0.0).sqrt();
    let ahead = [0.0, gz, level];
    // With the lens pointed straight up or down, anything level is square
    // to it.
    let (z, side) = if level > 1e-4 {
        (-gx * gz / level, gy)
    } else {
        (0.0, 1.0)
    };
    let x = (1.0 - gx * gx - z * z).max(0.0).sqrt().copysign(side);
    let across = [x, gx, z];
    // Right-handed, as the camera's own axes are: across by down is ahead.
    let down = cross(ahead, across);
    [across, down, ahead]
}

/// A point in the camera's axes, in the world's.
fn carry([across, down, ahead]: [Point; 3], [x, y, z]: Point) -> Point {
    std::array::from_fn(|axis| across[axis] * x + down[axis] * y + ahead[axis] * z)
}

/// A point of the world as the eye sees it: x across the diagram, y down it
/// and z away from the eye.
fn view([x, y, z]: Point) -> Point {
    let (sin, cos) = YAW.sin_cos();
    let (x, z) = (x * cos + z * sin, z * cos - x * sin);
    let (sin, cos) = ELEVATION.sin_cos();
    let (y, z) = (y * cos - z * sin, y * sin + z * cos);
    [x, y, z]
}

/// A box about `center`, `half` its size each way, as its six faces.
fn cuboid(center: Point, half: Point) -> Vec<Face> {
    let mut faces = Vec::with_capacity(6);
    for axis in 0..3 {
        let [b, c] = [(axis + 1) % 3, (axis + 2) % 3];
        for side in [-1.0f32, 1.0] {
            let corners = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)]
                .into_iter()
                .map(|(along_b, along_c)| {
                    let mut point = center;
                    point[axis] += side * half[axis];
                    point[b] += along_b * half[b];
                    point[c] += along_c * half[c];
                    point
                })
                .collect();
            let mut outward = [0.0; 3];
            outward[axis] = side;
            faces.push(Face { corners, outward });
        }
    }
    faces
}

/// A cylinder along the camera's z from `back` to `front`, as its sides and
/// its front face.
fn cylinder(back: f32, front: f32, radius: f32) -> Vec<Face> {
    let angle = |step: f32| std::f32::consts::TAU * step / LENS_SIDES as f32;
    let at = |step: usize, z: f32| {
        let (sin, cos) = angle(step as f32).sin_cos();
        [radius * cos, radius * sin, z]
    };
    let mut faces: Vec<Face> = (0..LENS_SIDES)
        .map(|step| {
            let (sin, cos) = angle(step as f32 + 0.5).sin_cos();
            Face {
                corners: vec![
                    at(step, back),
                    at(step, front),
                    at(step + 1, front),
                    at(step + 1, back),
                ],
                outward: [cos, sin, 0.0],
            }
        })
        .collect();
    faces.push(Face {
        corners: (0..LENS_SIDES).map(|step| at(step, front)).collect(),
        outward: [0.0, 0.0, 1.0],
    });
    faces
}

/// Twice the area `points` enclose, positive where they go round clockwise
/// on screen, whose y runs down.
fn signed_area(points: &[Pos2]) -> f32 {
    points
        .iter()
        .zip(points.iter().cycle().skip(1))
        .map(|(a, b)| a.x * b.y - b.x * a.y)
        .sum()
}

fn cross(a: Point, b: Point) -> Point {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn dot(a: Point, b: Point) -> f32 {
    let length = (b[0] * b[0] + b[1] * b[1] + b[2] * b[2]).sqrt();
    (a[0] * b[0] + a[1] * b[1] + a[2] * b[2]) / length
}

/// `from` taken `share` of the way to `to`.
fn mix(from: Color, to: Color, share: f32) -> Color32 {
    let from: Color32 = from.into();
    let to: Color32 = to.into();
    let channel =
        |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * share).round() as u8;
    Color32::from_rgb(
        channel(from.r(), to.r()),
        channel(from.g(), to.g()),
        channel(from.b(), to.b()),
    )
}

/// Draws the line across the picture with `painter` at the angle `shown`
/// says the horizon runs, from one edge of the picture to the other, with a
/// short square mark pointing up. Where the file says what focal length it
/// was taken at and the true horizon falls on the picture, that is the line
/// drawn, in full, with a dot where it passes nearest the middle. Otherwise
/// the line through the middle is drawn, lighter, saying only which way is
/// level — and where the horizon falls outside the picture, a chevron at the
/// edge points the way it lies. Nothing where the lens pointed too steeply
/// for a horizon. Which of the two lines was drawn, if either.
pub(super) fn level_line(
    pass: &Pass,
    painter: &egui::Painter,
    current: &Current,
    shown: &Shown,
) -> Option<Line> {
    let horizon = shown.horizon?;
    let size = current.size();
    let placement = pass.view.placement(size, pass.input.viewport);
    let scale = pass.input.scale;
    let screen = |[x, y]: [f32; 2]| {
        let [x, y] = placement.screen_point([x, y]);
        pos2(x / scale, y / scale)
    };
    let middle = [size[0] / 2.0, size[1] / 2.0];
    let (sin, cos) = (horizon as f32).to_radians().sin_cos();
    let accent: Color32 = pass.theme.accent.into();
    let grid = pass.grid;
    let line = |width: f32, ink: Color32, dash: f32, gap: f32, from: [f32; 2], to: [f32; 2]| {
        let ends = [screen(from), screen(to)];
        // A solid band under the dashes, so they stand out from whatever
        // the picture is behind them.
        let under = Stroke::new(width + 2.0, UNDER);
        let ink = Stroke::new(width, ink);
        painter.line_segment(ends, under);
        painter.extend(egui::Shape::dashed_line(&ends, ink, dash, gap));
        (under, ink)
    };
    // Which side of the line is up, from the point on it at `at`.
    let tick = |at: Pos2, (under, ink): (Stroke, Stroke)| {
        let up = vec2(sin, -cos) * 10.0;
        painter.line_segment([at, at + up], under);
        painter.line_segment([at, at + up], ink);
    };

    let offset = shown
        .horizon_through(size.map(f64::from), current.exif.focal_equivalent)
        .map(|offset| offset.map(|value| value as f32));
    if let Some(offset) = offset {
        let foot = [middle[0] + offset[0], middle[1] + offset[1]];
        if let Some([from, to]) = across(foot, [cos, sin], size) {
            let width = grid.line_width(HORIZON_LINE);
            // The uprights first, under the horizon: spread along the
            // level line through the middle, each running toward the point
            // they all meet at, or square to the horizon where the lens was
            // level and they never meet.
            let vanishing = shown
                .vanishing(size.map(f64::from), current.exif.focal_equivalent)
                .map(|[x, y]| [middle[0] + x as f32, middle[1] + y as f32]);
            if let Some([start, end]) = across(middle, [cos, sin], size) {
                for step in 0..UPRIGHTS {
                    let share = (step as f32 + 0.5) / UPRIGHTS as f32;
                    let at = [
                        start[0] + (end[0] - start[0]) * share,
                        start[1] + (end[1] - start[1]) * share,
                    ];
                    let direction = match vanishing {
                        Some(point) => [point[0] - at[0], point[1] - at[1]],
                        None => [-sin, cos],
                    };
                    let length = direction[0].hypot(direction[1]);
                    if length < 1e-3 {
                        continue;
                    }
                    let direction = direction.map(|value| value / length);
                    if let Some([from, to]) = across(at, direction, size) {
                        line(
                            width,
                            accent.gamma_multiply(LEVEL_INK),
                            LEVEL_DASH,
                            LEVEL_GAP,
                            from,
                            to,
                        );
                    }
                }
            }
            let strokes = line(width, accent, DASH, GAP, from, to);
            let at = screen(foot);
            tick(at, strokes);
            painter.circle_filled(at, width * 1.5, accent);
            return Some(Line::Horizon);
        }
    }
    let [from, to] = across(middle, [cos, sin], size)?;
    let strokes = line(
        grid.line_width(HORIZON_LINE),
        accent.gamma_multiply(LEVEL_INK),
        LEVEL_DASH,
        LEVEL_GAP,
        from,
        to,
    );
    tick(screen(middle), strokes);
    // The horizon off the picture: a chevron at the edge, on the way from
    // the middle to it, pointing out toward it.
    if let Some(offset) = offset {
        let reach = edge_reach(offset, middle);
        let at = screen([middle[0] + offset[0] * reach, middle[1] + offset[1] * reach]);
        let out =
            (screen([middle[0] + offset[0], middle[1] + offset[1]]) - screen(middle)).normalized();
        let side = out.rot90() * CHEVRON;
        let tip = at - out * 2.0;
        let back = tip - out * CHEVRON;
        let width = grid.line_width(HORIZON_LINE);
        for stroke in [Stroke::new(width + 2.0, UNDER), Stroke::new(width, accent)] {
            painter.line_segment([back + side, tip], stroke);
            painter.line_segment([tip, back - side], stroke);
        }
    }
    Some(Line::Level)
}

/// Which line [`level_line`] drew across the picture.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Line {
    /// The true horizon, where it falls on the picture.
    Horizon,
    /// The line through the middle, which says only which way is level.
    Level,
}

impl Line {
    /// What the pill at the section's heading calls it.
    pub(super) fn word(self) -> &'static str {
        match self {
            Line::Horizon => "Horizon",
            Line::Level => "Level",
        }
    }
}

/// The band under the lines and the chevron drawn across the picture.
const UNDER: Color32 = Color32::WHITE;

/// How many upright lines are drawn across the picture with the horizon.
const UPRIGHTS: usize = 5;

/// How far the chevron marking a horizon off the picture runs back and
/// out to either side.
const CHEVRON: f32 = 7.0;

/// Where the line through `point` along `direction` enters and leaves a
/// picture `size` across and down: `None` where it misses it.
fn across(point: [f32; 2], direction: [f32; 2], size: [f32; 2]) -> Option<[[f32; 2]; 2]> {
    let (mut enter, mut leave) = (f32::NEG_INFINITY, f32::INFINITY);
    for axis in 0..2 {
        if direction[axis].abs() < 1e-6 {
            if !(0.0..=size[axis]).contains(&point[axis]) {
                return None;
            }
            continue;
        }
        let a = -point[axis] / direction[axis];
        let b = (size[axis] - point[axis]) / direction[axis];
        enter = enter.max(a.min(b));
        leave = leave.min(a.max(b));
    }
    let at = |t: f32| [point[0] + t * direction[0], point[1] + t * direction[1]];
    (enter < leave).then(|| [at(enter), at(leave)])
}

/// The share of `offset` from the middle that reaches the picture's edge,
/// `middle` being half the picture each way.
fn edge_reach(offset: [f32; 2], middle: [f32; 2]) -> f32 {
    offset
        .iter()
        .zip(middle)
        .filter(|(along, _)| along.abs() > 1e-6)
        .map(|(along, half)| half / along.abs())
        .fold(1.0, f32::min)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A line through the picture is cut at its edges; one that passes it
    /// by, level or slanted, is not drawn at all.
    #[test]
    fn a_line_is_cut_to_the_picture() {
        let size = [400.0, 300.0];
        assert_eq!(
            across([200.0, 100.0], [1.0, 0.0], size),
            Some([[0.0, 100.0], [400.0, 100.0]])
        );
        assert_eq!(across([200.0, -10.0], [1.0, 0.0], size), None);
        let (sin, cos) = 0.1f32.sin_cos();
        let [from, to] = across([200.0, 150.0], [cos, sin], size).unwrap();
        assert!(from[0].abs() < 1e-3 && (to[0] - 400.0).abs() < 1e-3);
        assert_eq!(across([200.0, -400.0], [cos, sin], size), None);
    }

    /// The edge is reached along the way to a horizon off the picture.
    #[test]
    fn a_horizon_off_the_picture_is_marked_at_its_edge() {
        assert_eq!(edge_reach([0.0, -600.0], [200.0, 150.0]), 0.25);
        assert_eq!(edge_reach([0.0, -100.0], [200.0, 150.0]), 1.0);
    }
}
