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
/// the level line drawn across the picture.
const EDGE: f32 = 1.0;
const LEVEL_LINE: f32 = 1.5;
/// The dashes the level line and the line of sight are drawn in.
const DASH: f32 = 6.0;
const GAP: f32 = 4.0;

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
            // Facing away from the eye, which looks along +z.
            if seen[2] >= 0.0 {
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
        painter.add(egui::Shape::convex_polygon(points, fill, edge));
        if let Some(screen) = screen {
            let glass = mix(theme.bar_background, theme.accent, 0.05);
            painter.add(egui::Shape::convex_polygon(
                screen,
                glass,
                Stroke::new(edge.width, dim),
            ));
        }
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

/// Draws the level line across the picture with `painter`: through its
/// middle, at the angle `shown` says the horizon runs, from one edge of the
/// picture to the other, with a short square mark at the middle pointing
/// up. Nothing where the lens pointed too steeply for a horizon.
pub(super) fn level_line(pass: &Pass, painter: &egui::Painter, current: &Current, shown: &Shown) {
    let Some(horizon) = shown.horizon else {
        return;
    };
    let size = current.size();
    let placement = pass.view.placement(size, pass.input.viewport);
    let scale = pass.input.scale;
    let screen = |[x, y]: [f32; 2]| {
        let [x, y] = placement.screen_point([x, y]);
        pos2(x / scale, y / scale)
    };
    let middle = [size[0] / 2.0, size[1] / 2.0];
    let (sin, cos) = (horizon as f32).to_radians().sin_cos();
    // How far either way the line runs before it leaves the picture.
    let reach = [cos, sin]
        .iter()
        .zip(middle)
        .filter(|(along, _)| along.abs() > 1e-6)
        .map(|(along, half)| half / along.abs())
        .fold(f32::INFINITY, f32::min);
    if !reach.is_finite() {
        return;
    }
    let end = |sign: f32| {
        screen([
            middle[0] + sign * reach * cos,
            middle[1] + sign * reach * sin,
        ])
    };
    let ends = [end(-1.0), end(1.0)];
    let grid = pass.grid;
    let theme = pass.theme;
    let accent: Color32 = theme.accent.into();
    let width = grid.line_width(LEVEL_LINE);
    // A dark band under the dashes, so they are seen on a bright sky too.
    let under = Stroke::new(width + 2.0, Color32::from_black_alpha(90));
    painter.line_segment(ends, under);
    painter.extend(egui::Shape::dashed_line(
        &ends,
        Stroke::new(width, accent),
        DASH,
        GAP,
    ));
    // The middle, and which side of the line is up.
    let at = screen(middle);
    let up = vec2(sin, -cos) * 10.0;
    painter.line_segment([at, at + up], under);
    painter.line_segment([at, at + up], Stroke::new(width, accent));
    painter.circle_filled(at, width * 1.5, accent);
}
