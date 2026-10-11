//! The plot every section draws: each plane as a line over its area, the
//! pointer's rule, the band along its foot and the mark at white — and the
//! arithmetic under it: where a bin stands and how tall it is.

use std::ops::Range;

use egui::{Align2, Color32, FontId, pos2};

use super::*;
use crate::image::stats::COLOR;
use crate::ui::outline;

/// The ground the plot is drawn on. Not quite black, so that the panel's own
/// edge is still an edge rather than a hole in it. The one surface with a
/// job that outranks matching the desktop: the planes' lines and their
/// faint areas need a dark ground under them to read as three colors, so it
/// is the same near-black in every theme.
pub(super) const PLOT_BACKGROUND: Color = Color::rgb(8, 8, 10);
/// The luminance plane. Neutral, and no theme's
/// business: it is the one plane that stands for a pixel's value rather
/// than for a channel, and a value with a hue on it would read as a fourth
/// color.
pub(super) const HISTOGRAM_LUMA: Color = Color::rgba(170, 170, 170, 200);
/// The color planes: a red, a green and a blue, each taken some way back
/// from its primary, in that order. Not the theme's own red, green and
/// blue, which no theme chose for this: each leads in its own channel, so
/// three lines crossing on [`PLOT_BACKGROUND`] stay three, and read the
/// same in every theme, the ground being the same in every theme. Held
/// short of the primaries because the full ones, at a pixel to the bin,
/// come out as a hedge of pure red, green and blue spikes that the eye
/// cannot leave alone; these read as a plot.
pub(super) const HISTOGRAM_PLANES: [Color; COLOR] = [
    Color::rgb(232, 76, 70),
    Color::rgb(92, 200, 108),
    Color::rgb(84, 132, 236),
];

/// How the response curve is scaled up the plot: on the plot's own axis, as
/// the bins across are, from `floor` at the axis to `ceiling` at the top.
///
/// The floor is black — 0 on a curve, and on an axis in stops, where black
/// is below every floor, the plot's own. The ceiling is white — on the
/// axis, so that a display doing nothing draws the diagonal — except where
/// the curve runs past it, which it does on a surface with room above white
/// and no curve on: the top of the plot is then wherever the response gets
/// to, and white is a line drawn across it. `white` is where that line
/// goes, as a fraction of the plot's height, and `None` where white is the
/// top and the line would be the plot's own edge.
pub(super) struct Scale {
    floor: f32,
    ceiling: f32,
    pub(super) white: Option<f32>,
}

impl Scale {
    pub(super) fn new(floor: f32, placed_white: f32, highest: f32) -> Self {
        let ceiling = highest.max(placed_white).max(floor + f32::MIN_POSITIVE);
        let white = (placed_white - floor) / (ceiling - floor);
        Self {
            floor,
            ceiling,
            white: (white < 1.0 - 1e-3).then_some(white),
        }
    }

    /// A response on the plot's axis as a fraction of the plot's height.
    pub(super) fn up(&self, placed: f32) -> f32 {
        ((placed - self.floor) / (self.ceiling - self.floor)).clamp(0.0, 1.0)
    }
}

/// Where a bin's bar is drawn across the plot, from 0 at the left edge to 1
/// at the right. The two end bins are carried out to the edges so that the
/// shape fills the plot's width; the rest stand at their centers.
pub(super) fn bin_across(index: usize) -> f32 {
    match index {
        0 => 0.0,
        last if last == BINS - 1 => 1.0,
        _ => (index as f32 + 0.5) / BINS as f32,
    }
}

/// The bins under the `column`th of `columns` device columns across the
/// plot: every bin any part of it covers, so that a column wider than a bin
/// takes all of them and one narrower than a bin still takes the one it is
/// in. Never empty, never past the last bin, and between them the columns
/// take every bin.
pub(super) fn bins_under(column: usize, columns: usize) -> Range<usize> {
    let columns = columns.max(1);
    let column = column.min(columns - 1);
    let first = column * BINS / columns;
    let end = ((column + 1) * BINS).div_ceil(columns).min(BINS);
    first..end.max(first + 1)
}

/// The fullest of `bins` in `counts`: what a column over more than one bin
/// stands as tall as, so that a spike in one of them is not averaged away
/// by the columns beside it.
pub(super) fn tallest(counts: &[u32; BINS], bins: Range<usize>) -> u32 {
    counts[bins].iter().copied().max().unwrap_or(0)
}

/// The share of the bins a column's scale lets past its top: of the bins
/// holding anything inside the ends of the reference plot, the fullest
/// fiftieth stand cut off, so that a spike or two — a flat sky, a masked
/// sea, the surround of a scan — cannot flatten the rest into the axis.
const CUT: f32 = 0.02;

/// How thick the cap along the top of a bar cut off there is.
const CUT_CAP: f32 = 2.0;

/// The count axis a plot stands on: a bar's height is the share of its
/// plane's pixels in its bin, and `top` is the share a bar reaching the top
/// of a plot holds. The input's two plots — the Display's and the Image's —
/// share one, so that a bar of the same height is the same share of the
/// picture in each, and what the gain map's lift and the working space do
/// to the file read as bars growing or shrinking between them. The Output's
/// stands on its own, set afresh from what goes out: the display spreads
/// and piles the picture up as it is set, and a scale held to the input's
/// would leave a stretched Output a sliver along the axis. The gain map's,
/// of another image in other units, stands on its own too.
///
/// Linear, as a photo editor's is: heights compare as the counts do. A bar
/// fuller than the top runs to it and is capped there, and its share is
/// what the header says with the pointer on it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Counts {
    top: f32,
}

impl Counts {
    /// The scale `reference` sets: the fullest [`CUT`] of its bins holding
    /// anything, across every plane and leaving out the two end bins — what
    /// the window throws away piles up there — stand past the top. A plot with nothing inside its
    /// ends — a picture of black and white alone — is scaled to its fullest
    /// bin, and an empty one to the whole of the picture.
    pub(super) fn of(reference: &Plot) -> Self {
        let planes = || std::iter::once(&reference.luma).chain(reference.color.iter().flatten());
        let shares = |inside: bool| -> Vec<f32> {
            planes()
                .flat_map(|counts| {
                    let total = total(counts);
                    let bins = if inside { 1..BINS - 1 } else { 0..BINS };
                    counts[bins]
                        .iter()
                        .filter(move |&&count| count > 0)
                        .map(move |&count| share_of(count, total))
                })
                .collect()
        };
        let mut held = shares(true);
        if held.is_empty() {
            held = shares(false);
        }
        held.sort_by(|a, b| b.total_cmp(a));
        let top = held
            .get((held.len() as f32 * CUT) as usize)
            .copied()
            .unwrap_or(1.0);
        Self { top }
    }

    /// The scale of the two that cuts fewer bars off: the one two plots on
    /// different axes share, set by whichever of them piles its pixels up
    /// the most.
    pub(super) fn wider(self, other: Self) -> Self {
        Self {
            top: self.top.max(other.top),
        }
    }

    /// How tall a bin holding `count` of a plane's `total` stands, from 0
    /// on the axis to 1 at the top of the plot, and past 1 where it is cut
    /// off there.
    pub(super) fn up(self, count: u32, total: u64) -> f32 {
        share_of(count, total) / self.top
    }

    /// The fullest share a plane of `plot` holds at `bin`, where it stands
    /// past the top: what the header says of a bar cut off.
    pub(super) fn cut_at(self, plot: &Plot, bin: usize) -> Option<f32> {
        std::iter::once(&plot.luma)
            .chain(plot.color.iter().flatten())
            .map(|counts| share_of(counts[bin], total(counts)))
            .filter(|&held| held > self.top)
            .max_by(f32::total_cmp)
    }
}

/// Every count in a plane: the pixels it was counted from.
pub(super) fn total(counts: &[u32; BINS]) -> u64 {
    counts.iter().map(|&count| u64::from(count)).sum()
}

/// `count` as a share of `total`, and none of nothing.
fn share_of(count: u32, total: u64) -> f32 {
    if total == 0 {
        0.0
    } else {
        (f64::from(count) / total as f64) as f32
    }
}

/// How thick a plane's line is, in device pixels; the
/// gray its area is laid down in, each plane's over the others'; how far
/// that gray is taken toward the plane's own ink, so that each area wears a
/// cast of its line's color; and how opaque it is.
const LINE_PIXELS: f32 = 1.5;
const LINE_AREA: [u8; 3] = [150, 150, 150];
const LINE_CAST: f32 = 0.3;
const LINE_ALPHA: u8 = 40;

/// The area under a plane's line: [`LINE_AREA`] taken
/// [`LINE_CAST`] of the way toward `ink`.
fn line_area(ink: Color32) -> Color32 {
    let [r, g, b] = [(0, ink.r()), (1, ink.g()), (2, ink.b())].map(|(at, to)| {
        (LINE_AREA[at] as f32 + (to as f32 - LINE_AREA[at] as f32) * LINE_CAST).round() as u8
    });
    Color32::from_rgba_unmultiplied(r, g, b, LINE_ALPHA)
}

/// The bins of `plotted` across `bars` on the column's scale `counts`, on
/// the plot's ground, as [`lines`] draws them, with `rules` over them, each
/// a bin and its ink, gliding to it as [`glided`] has them in `section`.
/// Drawn alike in every section, so that a bar reads the same down the
/// column.
pub(super) fn bars(
    pass: &Pass,
    ui: &egui::Ui,
    section: Section,
    plotted: &Plot,
    counts: Counts,
    bars: Rect,
    rules: &[(usize, Color32)],
) {
    let painter = ui.painter();
    painter.rect_filled(
        egui::Rect::from(bars.inset(-PLOT_INSET, -PLOT_INSET)),
        PLOT_RADIUS,
        PLOT_BACKGROUND,
    );

    let color: &[[u32; BINS]] = plotted.color.as_ref().map_or(&[], |planes| planes);
    lines(pass, ui, &plotted.luma, color, counts, bars);
    rule_over(pass, ui, section, bars, rules);
}

/// How long a rule takes to follow the pointer to its next bin, in seconds:
/// long enough to be seen moving rather than jumping, short enough never to
/// trail the hand.
const RULE_GLIDE: f64 = 0.1;

/// A rule on its way from one place on the axis to another, as fractions of
/// the way along it: where it set off from, where it is going, when it set
/// off, and the last frame it was drawn on.
#[derive(Clone, Copy)]
struct Glide {
    from: f32,
    to: f32,
    start: f64,
    frame: u64,
}

impl Glide {
    /// Where the rule is at `now`: easing out, quick to leave and slow to
    /// arrive, so that it settles rather than stops.
    fn at(self, now: f64) -> f32 {
        let t = ((now - self.start) / RULE_GLIDE).clamp(0.0, 1.0) as f32;
        let eased = 1.0 - (1.0 - t).powi(3);
        self.from + (self.to - self.from) * eased
    }
}

/// Where the rule `id` is drawn on its way to `target`, a fraction of the
/// way along the axis, with another frame asked for while it is still
/// moving. A rule that was not drawn on the frame before — the pointer
/// arriving on the picture or the plot, or coming back to it — stands at
/// once where it is going rather than gliding in from where it was last.
fn glided(ui: &egui::Ui, id: egui::Id, target: f32) -> f32 {
    let ctx = ui.ctx();
    let now = ui.input(|input| input.time);
    let frame = ctx.cumulative_frame_nr();
    let glide = match ctx.data(|data| data.get_temp::<Glide>(id)) {
        Some(held) if held.frame + 1 >= frame && held.to == target => Glide { frame, ..held },
        Some(held) if held.frame + 1 >= frame => Glide {
            from: held.at(now),
            to: target,
            start: now,
            frame,
        },
        _ => Glide {
            from: target,
            to: target,
            start: now,
            frame,
        },
    };
    ctx.data_mut(|data| data.insert_temp(id, glide));
    let shown = glide.at(now);
    if shown != glide.to {
        ctx.request_repaint();
    }
    shown
}

/// The pointer's rule across `bars` in `section`, each a bin and its ink.
fn rule_over(pass: &Pass, ui: &egui::Ui, section: Section, bars: Rect, rules: &[(usize, Color32)]) {
    let grid = pass.grid;
    let painter = ui.painter();
    // The pointer's rule: over the bins it is picking one of, and under
    // anything drawn over them, which is half of what the readout says
    // where there is a curve and the line must not cover it.
    //
    // Full height, where the window's own marks are handles on the band. A
    // rule standing through the plot is what a pointer wants and what a
    // permanent annotation does not: this one is only there while it is
    // being aimed, and it has to be followed up from the axis to the curve.
    //
    // Each glides to its bin rather than jumping, so that a pointer swept
    // across the picture is followed by rules moving down the column rather
    // than by rules flickering from place to place. A rule is known by its
    // section and its ink, which is its plane, or the accent.
    for &(bin, ink) in rules {
        let across = glided(
            ui,
            egui::Id::new(("histogram rule", section, ink)),
            bin_across(bin),
        );
        painter.rect_filled(
            egui::Rect::from(grid.rect(Rect::new(
                bars.x + across * bars.width - CURSOR_WIDTH / 2.0,
                bars.y,
                CURSOR_WIDTH,
                bars.height,
            ))),
            0.0,
            ink,
        );
    }
}

/// The plot's planes, at one column to a device pixel
/// and as tall as the fullest bin under it, so that a spike is not averaged
/// away by its neighbors: each plane's area laid down in [`line_area`], each
/// over the others', then each plane's top as a line in its own ink, the
/// luminance plane's under the color planes'; and a cap in the accent along
/// the top of a column cut off there.
fn lines(
    pass: &Pass,
    ui: &egui::Ui,
    luma: &[u32; BINS],
    color: &[[u32; BINS]],
    counts: Counts,
    bars: Rect,
) {
    let grid = pass.grid;
    let painter = ui
        .painter()
        .with_clip_rect(egui::Rect::from(bars.inset(-PLOT_INSET, -PLOT_INSET)));
    // Luminance first, so that its line runs under the color planes'.
    let planes: Vec<(&[u32; BINS], Color32)> =
        std::iter::once((luma, Color32::from(HISTOGRAM_LUMA.with_alpha(255))))
            .chain(
                color
                    .iter()
                    .zip(HISTOGRAM_PLANES)
                    .map(|(plane, ink)| (plane, Color32::from(ink))),
            )
            .collect();
    let columns: Vec<_> = grid.columns(bars.x, bars.width).collect();
    let heights: Vec<Vec<f32>> = planes
        .iter()
        .map(|(plane, _)| {
            let total = total(plane);
            columns
                .iter()
                .map(|column| {
                    counts.up(
                        tallest(plane, bins_under(column.index, column.count)),
                        total,
                    )
                })
                .collect()
        })
        .collect();
    for (plane, (_, ink)) in heights.iter().zip(&planes) {
        let area = line_area(*ink);
        for (column, &height) in columns.iter().zip(plane) {
            let top = grid.snap(bars.bottom() - height.min(1.0) * bars.height);
            if top < bars.bottom() {
                painter.rect_filled(
                    egui::Rect::from_min_max(
                        pos2(column.left, top),
                        pos2(column.right, bars.bottom()),
                    ),
                    0.0,
                    area,
                );
            }
        }
    }
    let width = grid.device_pixels(LINE_PIXELS);
    for (plane, (_, ink)) in heights.iter().zip(&planes) {
        let points: Vec<_> = columns
            .iter()
            .zip(plane)
            .map(|(column, &height)| {
                pos2(
                    (column.left + column.right) / 2.0,
                    bars.bottom() - height.min(1.0) * bars.height,
                )
            })
            .collect();
        painter.line(points, egui::Stroke::new(width, *ink));
    }
    for (index, column) in columns.iter().enumerate() {
        if heights.iter().any(|plane| plane[index] > 1.0) {
            painter.rect_filled(
                egui::Rect::from_min_max(
                    pos2(column.left, grid.snap(bars.y)),
                    pos2(column.right, grid.snap(bars.y) + grid.line_width(CUT_CAP)),
                ),
                0.0,
                pass.theme.accent,
            );
        }
    }
}

/// How a band marks the stretch of it where the screen goes past white,
/// which the panel cannot show, since it cannot glow.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Past {
    /// The accent along its top.
    Rule,
    /// A run of chevrons pointing right, in the accent, over it: what goes
    /// out lifted above white, on the band that is nothing but what goes
    /// out.
    Chevrons,
}

/// The band under the plot `bars`: a device pixel to the cell, each the
/// color `shade` gives for its place along the axis, from 0 at the left to
/// 1 at the right, marked as `past` says where `shade` says the screen goes
/// past white there. Outlined outside the color rather than over it, so
/// that a band that comes out black is still a band on a dark panel.
pub(super) fn band(
    pass: &Pass,
    ui: &egui::Ui,
    bars: Rect,
    past: Past,
    shade: impl Fn(f32) -> Shade,
) {
    let theme = pass.theme;
    let painter = ui.painter();
    let grid = pass.grid;
    let band = ramp(bars);
    let (top, bottom) = (grid.snap(band.y), grid.snap(band.bottom()));
    let hair = grid.device_pixels(1.0);
    let cells: Vec<_> = grid.columns(bars.x, bars.width).collect();
    let mut lifted: Option<(f32, f32)> = None;
    for column in &cells {
        let (left, right) = (column.left, column.right);
        let (color, above) = shade(column.t_center());
        painter.rect_filled(
            egui::Rect::from_min_max(pos2(left, top), pos2(right, bottom)),
            0.0,
            color,
        );
        if above {
            match past {
                Past::Rule => {
                    painter.rect_filled(
                        egui::Rect::from_min_max(pos2(left, top), pos2(right, top + hair)),
                        0.0,
                        theme.accent,
                    );
                }
                Past::Chevrons => {
                    lifted = Some(lifted.map_or((left, right), |(from, _)| (from, right)));
                }
            }
        }
    }
    if let Some((from, to)) = lifted {
        chevrons(
            pass,
            ui,
            egui::Rect::from_min_max(pos2(from, top), pos2(to, bottom)),
        );
    }
    let (Some(first), Some(last)) = (cells.first(), cells.last()) else {
        return;
    };
    let (left, right) = (first.left, last.right);
    outline(
        painter,
        grid,
        Rect::new(
            left - hair,
            top - hair,
            right - left + 2.0 * hair,
            bottom - top + 2.0 * hair,
        ),
        hair,
        theme.border.into(),
    );
}

/// How far apart the chevrons over a band's lifted stretch stand, and how
/// far each one is kept off the band's top and bottom.
const CHEVRON_PITCH: f32 = 6.0;
const CHEVRON_INSET: f32 = 2.0;

/// Chevrons pointing right, one every [`CHEVRON_PITCH`] from the left of
/// `stretch` and cut off at its right, in the accent: the stretch of a band
/// that is lifted past white.
fn chevrons(pass: &Pass, ui: &egui::Ui, stretch: egui::Rect) {
    let grid = pass.grid;
    let painter = ui.painter().with_clip_rect(stretch);
    let stroke = egui::Stroke::new(grid.line_width(1.0), Color32::from(pass.theme.accent));
    let (high, low) = (
        stretch.top() + CHEVRON_INSET,
        stretch.bottom() - CHEVRON_INSET,
    );
    let middle = (high + low) / 2.0;
    let depth = (low - high) / 2.0;
    let mut x = stretch.left() + CHEVRON_INSET;
    while x < stretch.right() {
        painter.line(
            vec![pos2(x, high), pos2(x + depth, middle), pos2(x, low)],
            stroke,
        );
        x += CHEVRON_PITCH;
    }
}

/// The row of whole stops along a plot's axis, drawn in `row`, which spans
/// the plot's width: a tick at every stop from `stops` that `place` puts on
/// the axis — as a fraction of the way along it, or `None` off it — and the
/// stop's number under the ticks there is room to write it beside.
///
/// Worked out from 0 outward each way, so that 0 is always written and the
/// others give way to it and to their neighbors nearer 0; a direction stops
/// where its ticks come closer than [`EV_TICK_GAP`], which on an axis spaced
/// on a curve or in linear light is where the stops crowd into the shadows
/// past reading. Exact wherever it is drawn: a tick stands where the stop's
/// value falls on the axis, worked out from the value rather than from the
/// bins.
pub(super) fn ev_scale(
    pass: &Pass,
    ui: &egui::Ui,
    row: Rect,
    stops: std::ops::RangeInclusive<i32>,
    place: impl Fn(i32) -> Option<f32>,
) {
    let at = |t: f32| row.x + t * row.width;
    // The extents the labels written so far take, so that the next one out
    // is left off where it would touch.
    let mut taken: Vec<(f32, f32)> = Vec::new();
    let draw = |stop: i32, x: f32, taken: &mut Vec<(f32, f32)>| {
        let words = if stop == 0 {
            "0".to_string()
        } else {
            format!("{stop:+}")
        };
        let ink = if stop == 0 {
            pass.theme.text_primary
        } else {
            pass.theme.text_dim
        };
        tick(pass, ui, row, x, &words, ink.into(), taken);
    };
    let (low, high) = (*stops.start(), *stops.end());
    let walk = |from: i32, to: i32, step: i32, taken: &mut Vec<(f32, f32)>| {
        let mut last: Option<f32> = None;
        let mut stop = from;
        while (step > 0 && stop <= to) || (step < 0 && stop >= to) {
            if let Some(t) = place(stop).filter(|t| (0.0..=1.0).contains(t)) {
                let x = at(t);
                if last.is_some_and(|last| (x - last).abs() < EV_TICK_GAP) {
                    break;
                }
                last = Some(x);
                draw(stop, x, taken);
            }
            stop += step;
        }
    };
    let start = 0.clamp(low, high);
    walk(start, high, 1, &mut taken);
    walk(start - 1, low, -1, &mut taken);
}

/// A row of values along a plot's axis, drawn in `row`, which spans the
/// plot's width: a tick at each of `ticks`, a fraction of the way along the
/// axis and the words for the value there, and the words under the ticks
/// there is room to write them beside, the earlier in `ticks` first.
pub(super) fn value_scale(pass: &Pass, ui: &egui::Ui, row: Rect, ticks: &[(f32, String)]) {
    let mut taken: Vec<(f32, f32)> = Vec::new();
    for (t, words) in ticks {
        if (0.0..=1.0).contains(t) {
            let x = row.x + t * row.width;
            tick(
                pass,
                ui,
                row,
                x,
                words,
                pass.theme.text_dim.into(),
                &mut taken,
            );
        }
    }
}

/// One tick of a row along an axis, at `x` in `row`, and `words` under it
/// in `ink`, held inside the row and left off where they would touch any of
/// `taken`, the extents of the words written before them, which they join.
fn tick(
    pass: &Pass,
    ui: &egui::Ui,
    row: Rect,
    x: f32,
    words: &str,
    ink: Color32,
    taken: &mut Vec<(f32, f32)>,
) {
    let grid = pass.grid;
    let painter = ui.painter();
    let hair = grid.line_width(1.0);
    painter.rect_filled(
        egui::Rect::from(grid.rect(Rect::new(x - hair / 2.0, row.y, hair, EV_TICK))),
        0.0,
        pass.theme.text_dim,
    );
    let width = width_of(ui, words, CLIP_TEXT);
    let left = (x - width / 2.0).clamp(row.x, row.right() - width);
    let span = (left - EV_LABEL_GAP / 2.0, left + width + EV_LABEL_GAP / 2.0);
    if taken.iter().any(|(a, b)| span.0 < *b && *a < span.1) {
        return;
    }
    taken.push(span);
    painter.text(
        pos2(grid.snap(left), row.y + EV_TICK),
        Align2::LEFT_TOP,
        words,
        FontId::proportional(CLIP_TEXT),
        ink,
    );
}

/// The rule that marks white on the plot `bars`, where white is not the top
/// of the plot: `across` of the way along the axis, standing from the axis
/// `up` of the way to the top, and named at its head. On a surface with
/// room above white, the room past it can then be seen as the room it is
/// rather than as a clip that is not happening. Upright, as the axis's own
/// marks are: a rule across the plot would cross the bins as though it
/// measured them.
pub(super) fn white_mark(pass: &Pass, ui: &egui::Ui, bars: Rect, across: f32, up: f32) {
    let theme = pass.theme;
    let grid = pass.grid;
    let painter = ui.painter();
    let hair = grid.device_pixels(1.0);
    let x = grid.snap(bars.x + across * bars.width);
    let top = grid.snap(bars.bottom() - up * bars.height);
    painter.rect_filled(
        egui::Rect::from(grid.rect(Rect::new(x - hair / 2.0, top, hair, bars.bottom() - top))),
        0.0,
        theme.text_dim,
    );
    // Named at its head. A rule that runs the plot's height is named beside
    // its top, after it where the word fits and before it where it does
    // not; one that stops at a curve is named above its head and before it,
    // which is the side a curve rising to the right leaves clear.
    let size = TEXT_SIZE * 0.75;
    let width = width_of(ui, WHITE_LABEL, size);
    let (at, align) = if up < 1.0 {
        (pos2(x - 3.0, top - 3.0), Align2::RIGHT_BOTTOM)
    } else if x + 3.0 + width <= bars.right() - CLIP_INSET {
        (pos2(x + 3.0, bars.y + CLIP_INSET), Align2::LEFT_TOP)
    } else {
        (pos2(x - 3.0, bars.y + CLIP_INSET), Align2::RIGHT_TOP)
    };
    painter.text(
        at,
        align,
        WHITE_LABEL,
        FontId::proportional(size),
        theme.text_dim.into(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::icon::Grid;

    /// A plot of luminance alone, `counts` across its bins.
    fn plot_of(counts: [u32; BINS]) -> Plot {
        Plot {
            min: 0.0,
            max: 1.0,
            luma: counts,
            color: None,
        }
    }

    /// A spike cannot set the scale: a picture spread evenly but for one
    /// bin holding most of it stands on a scale the even bins nearly fill,
    /// and the spike runs past the top. The pile at either end is left out of
    /// the scale too.
    #[test]
    fn a_spike_is_cut_off_rather_than_flattening_the_rest() {
        let mut counts = [100; BINS];
        counts[BINS / 2] = 1_000_000;
        counts[BINS - 1] = 5_000_000;
        let plot = plot_of(counts);
        let scale = Counts::of(&plot);
        let total = total(&plot.luma);
        let even = scale.up(100, total);
        assert!((0.9..=1.0).contains(&even), "{even}");
        assert!(scale.up(1_000_000, total) > 1.0);
        assert!(scale.up(5_000_000, total) > 1.0);
        assert_eq!(scale.up(0, total), 0.0, "an empty bin is flat");

        // And the header says how much a cut bar holds, where the plane is
        // on screen, and nothing of a bar that is not cut.
        let held = scale.cut_at(&plot, BINS / 2).expect("cut");
        assert!((held - 1_000_000.0 / total as f32).abs() < 1e-6, "{held}");
        assert_eq!(scale.cut_at(&plot, 3), None);
    }

    /// The scale is a share of the picture, not a count: the same share
    /// stands as tall on a plot of a picture of any size, which is what lets
    /// the gain map's plot, of a smaller picture, share the column's.
    #[test]
    fn the_same_share_stands_as_tall_whatever_the_count() {
        let scale = Counts::of(&plot_of([100; BINS]));
        let small = scale.up(1, BINS as u64);
        let large = scale.up(1_000, 1_000 * BINS as u64);
        assert!((small - large).abs() < 1e-6, "{small} {large}");
        assert!((small - 1.0).abs() < 1e-6, "an even plot fills it: {small}");
    }

    /// A plot with nothing inside its ends is scaled to its fullest bin,
    /// and one with nothing at all draws nothing rather than a bar off the
    /// top.
    #[test]
    fn a_plot_with_nothing_inside_its_ends_still_has_a_scale() {
        let mut counts = [0; BINS];
        counts[0] = 300;
        counts[BINS - 1] = 100;
        let plot = plot_of(counts);
        let scale = Counts::of(&plot);
        assert!((scale.up(300, 400) - 1.0).abs() < 1e-6);
        assert!(scale.up(100, 400) < 1.0);

        let empty = Counts::of(&plot_of([0; BINS]));
        assert_eq!(empty.up(0, 0), 0.0);
    }

    /// Each plane leads in its own channel by a wide margin, so three lines
    /// crossing stay three — and none of them is the primary itself, which
    /// at a pixel to the bin is a spike the eye cannot leave alone.
    #[test]
    fn the_color_planes_are_one_channel_each() {
        for (channel, plane) in HISTOGRAM_PLANES.iter().enumerate() {
            let levels = [plane.r, plane.g, plane.b];
            for (other, level) in levels.iter().enumerate() {
                if other == channel {
                    assert!(*level >= 200, "{plane:?} leads in its own channel");
                    assert!(*level < 255, "{plane:?} is short of the primary");
                } else {
                    assert!(*level <= 140, "{plane:?} stays out of the others");
                }
            }
        }
    }

    /// The luminance plane stands for a pixel's value, not for one of its
    /// channels, so it carries no hue; and the ground the planes are drawn
    /// on is near black, or their faint areas would not read.
    #[test]
    fn the_luminance_plane_is_neutral_and_the_ground_is_dark() {
        let luma = HISTOGRAM_LUMA;
        assert!(luma.r == luma.g && luma.g == luma.b, "{luma:?}");
        let ground = PLOT_BACKGROUND;
        assert!(ground.r.max(ground.g).max(ground.b) < 16, "{ground:?}");
    }

    /// The device column counts a 256-point plot comes to across the
    /// interface's scales and the monitor's.
    const COLUMN_COUNTS: [usize; 6] = [192, 256, 320, 384, 512, 640];

    /// However many columns the plot is cut into, each takes at least one
    /// bin, they march along the bins without going back, and between them
    /// they take every one.
    #[test]
    fn every_bin_is_under_a_column_however_many_there_are() {
        for columns in COLUMN_COUNTS {
            let mut covered = [false; BINS];
            let mut last = 0..1;
            for column in 0..columns {
                let bins = bins_under(column, columns);
                assert!(!bins.is_empty(), "{columns}: {column}");
                assert!(bins.end <= BINS, "{columns}: {column}");
                assert!(bins.start >= last.start, "{columns}: {column}");
                assert!(bins.end >= last.end, "{columns}: {column}");
                for bin in bins.clone() {
                    covered[bin] = true;
                }
                last = bins;
            }
            assert!(covered.iter().all(|&bin| bin), "{columns}");
        }
        // At 1.25 a column is four fifths of a bin: some straddle two.
        assert_eq!(bins_under(1, 320), 0..2);
        assert_eq!(bins_under(4, 320), 3..4);
    }

    /// At a device pixel to the point, a column is its own bin.
    #[test]
    fn at_one_to_one_a_column_is_its_bin() {
        for column in 0..BINS {
            assert_eq!(bins_under(column, BINS), column..column + 1);
        }
    }

    /// One full bin among empty ones still stands at its height whether the
    /// columns are wider than a bin, as wide, or narrower.
    #[test]
    fn a_spike_survives_a_column_wider_than_a_bin() {
        let mut counts = [0; BINS];
        counts[101] = 1000;
        for columns in [192, 256, 320] {
            let peak = (0..columns)
                .map(|column| tallest(&counts, bins_under(column, columns)))
                .max();
            assert_eq!(peak, Some(1000), "{columns}");
        }
    }

    /// At 1.25 every device pixel of the plot is one column, and every bin
    /// stands in at least one of them.
    #[test]
    fn at_one_and_a_quarter_every_column_is_one_device_pixel_and_every_bin_is_drawn() {
        let grid = Grid::new(1.25);
        let columns: Vec<_> = grid.columns(20.0, BINS as f32).collect();
        assert_eq!(columns.len(), 320);
        for column in &columns {
            assert!(((column.right - column.left) * 1.25 - 1.0).abs() < 1e-3);
        }
        let mut drawn = [false; BINS];
        for column in &columns {
            for bin in bins_under(column.index, column.count) {
                drawn[bin] = true;
            }
        }
        assert!(drawn.iter().all(|&bin| bin));
    }

    /// At three quarters there are fewer columns than bins, and none of the
    /// bins is left out of them.
    #[test]
    fn at_three_quarters_no_bin_is_dropped() {
        let columns: Vec<_> = Grid::new(0.75).columns(20.0, BINS as f32).collect();
        assert_eq!(columns.len(), 192);
        let mut counts = [0; BINS];
        for bin in 0..BINS {
            counts.fill(0);
            counts[bin] = 7;
            assert!(
                columns
                    .iter()
                    .any(|column| tallest(&counts, bins_under(column.index, column.count)) == 7),
                "{bin}"
            );
        }
    }

    /// A rule stands at once where it first appears and where it comes back
    /// after a frame without it, and glides between bins otherwise, easing
    /// out and arriving on time.
    #[test]
    fn rules_glide_between_bins_and_stand_where_they_appear() {
        let ctx = egui::Context::default();
        let id = egui::Id::new("rule");
        let shown = |time: f64, target: Option<f32>| {
            let input = egui::RawInput {
                time: Some(time),
                ..Default::default()
            };
            let mut at = None;
            let mut output = ctx.run_ui(input, |ui| {
                at = target.map(|target| glided(ui, id, target));
            });
            output.textures_delta.clear();
            at
        };
        assert_eq!(shown(0.0, Some(0.2)), Some(0.2), "appears in place");
        assert_eq!(shown(0.01, Some(0.6)), Some(0.2), "sets off from there");
        let early = shown(0.01 + RULE_GLIDE / 4.0, Some(0.6)).unwrap();
        assert!(early > 0.2 + 0.4 / 4.0 && early < 0.6, "{early}");
        assert_eq!(shown(0.01 + RULE_GLIDE, Some(0.6)), Some(0.6), "arrives");
        assert_eq!(shown(0.1, None), None);
        assert_eq!(shown(0.11, Some(0.9)), Some(0.9), "comes back in place");
    }
}
