//! The plot every section draws: the bins as columns, the pointer's rule,
//! the words in its top corners, the band along its foot and the line that
//! marks white — and the arithmetic under it: where a bin stands, how tall
//! its bar is, and what a stretch of a column comes out as with the planes
//! standing over it.

use std::ops::Range;

use egui::{Align2, Color32, FontId, pos2};

use super::*;
use crate::image::stats::COLOR;
use crate::ui::outline;

/// The ground the plot is drawn on. Not quite black, so that the panel's own
/// edge is still an edge rather than a hole in it. The one surface with a
/// job that outranks matching the desktop: a screened plot has to have a
/// dark ground under it or its planes stop being three colors, so it is
/// the same near-black in every theme.
pub(super) const PLOT_BACKGROUND: Color = Color::rgb(8, 8, 10);
/// The luminance plane, under the color ones. Neutral, and no theme's
/// business: it is the one plane that stands for a pixel's value rather
/// than for a channel, and a value with a hue on it would read as a fourth
/// color.
pub(super) const HISTOGRAM_LUMA: Color = Color::rgba(170, 170, 170, 200);
/// The color planes: a red, a green and a blue, each taken some way back
/// from its primary, in that order. Not the theme's own red, green and
/// blue, which no theme chose for this: screened over one another on
/// [`PLOT_BACKGROUND`] these still give a yellow, a cyan and a magenta
/// where two overlap and a near white where all three do, which is the
/// reading a channel histogram is looked at for — and is the same reading
/// in every theme, the ground being the same in every theme. Held short of
/// the primaries because the full ones, at a pixel to the bin, come out as
/// a hedge of pure red, green and blue spikes that the eye cannot leave
/// alone; these read as a plot.
pub(super) const HISTOGRAM_PLANES: [Color; COLOR] = [
    Color::rgb(232, 76, 70),
    Color::rgb(92, 200, 108),
    Color::rgb(84, 132, 236),
];

/// The planes a column of the plot is drawn from: which of them stand this
/// high, as a set. What the color of a stretch of the column is a function of.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) struct Cover {
    pub luma: bool,
    pub planes: [bool; 3],
}

/// What a stretch of a column comes out as, with `cover` standing over it:
/// the plot's ground, the luminance plane laid over that, and the color
/// planes screened over the lot.
///
/// egui has one blend, so the screening is done here, per stretch of
/// column: the planes are the primaries on a near-black ground, so two of
/// them give the secondary between and all three give white, which is the
/// reading a channel histogram is looked at for.
pub(super) fn screened(luma_ink: Color, cover: Cover) -> Color32 {
    let over = |ground: [f32; 3], ink: Color| -> [f32; 3] {
        let alpha = ink.a as f32 / 255.0;
        let ink = [ink.r, ink.g, ink.b].map(|channel| channel as f32 / 255.0);
        [0, 1, 2].map(|c| ground[c] * (1.0 - alpha) + ink[c] * alpha)
    };
    let screen = |ground: [f32; 3], ink: Color| -> [f32; 3] {
        let ink = [ink.r, ink.g, ink.b].map(|channel| channel as f32 / 255.0);
        [0, 1, 2].map(|c| 1.0 - (1.0 - ground[c]) * (1.0 - ink[c]))
    };
    let ground = PLOT_BACKGROUND;
    let mut color = [ground.r, ground.g, ground.b].map(|channel| channel as f32 / 255.0);
    if cover.luma {
        color = over(color, luma_ink);
    }
    for (plane, ink) in cover.planes.into_iter().zip(HISTOGRAM_PLANES) {
        if plane {
            color = screen(color, ink);
        }
    }
    let [r, g, b] = color.map(|channel| (channel.clamp(0.0, 1.0) * 255.0).round() as u8);
    Color32::from_rgb(r, g, b)
}

/// How the response curve is scaled up the plot: in the file's own encoding,
/// as the bins across are, from 0 at the axis to `ceiling` at the top.
///
/// The ceiling is white — encoded, so that a display doing nothing draws the
/// diagonal — except where the curve runs past it, which it does on a surface
/// with room above white and no curve on: the top of the plot is then
/// wherever the response gets to, and white is a line drawn across it.
/// `white` is where that line goes, as a fraction of the plot's height, and
/// `None` where white is the top and the line would be the plot's own edge.
pub(super) struct Scale {
    ceiling: f32,
    pub(super) white: Option<f32>,
}

impl Scale {
    pub(super) fn new(encoded_white: f32, highest: f32) -> Self {
        let ceiling = highest.max(encoded_white).max(f32::MIN_POSITIVE);
        let white = encoded_white / ceiling;
        Self {
            ceiling,
            white: (white < 1.0 - 1e-3).then_some(white),
        }
    }

    /// An encoded response as a fraction of the plot's height.
    pub(super) fn up(&self, encoded: f32) -> f32 {
        (encoded / self.ceiling).clamp(0.0, 1.0)
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
/// of a plot holds. One for every plot of the picture down the column —
/// the Output, the Display and the Image — so that a bar of the same height
/// is the same share of the picture in each, and the curve piling the
/// highlights up or spreading the shadows out reads as bars growing or
/// shrinking between the Display and the Output plots. The gain map's plot,
/// of another image in other units, stands on one of its own.
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
    /// the window throws away piles up there, and the Output plot's corners
    /// say how much — stand past the top. A plot with nothing inside its
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

    /// The scale of the two that cuts fewer bars off: the one plots on
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

    /// The fullest share of a plane `planes` holds at `bin` on `plot`, where
    /// it stands past the top: what the header says of a bar cut off.
    pub(super) fn cut_at(
        self,
        plot: &Plot,
        bin: usize,
        planes: impl Fn(Plane) -> bool,
    ) -> Option<f32> {
        let luma = planes(Plane::Luma).then_some(&plot.luma);
        let color = plot
            .color
            .iter()
            .flat_map(|color| Plane::COLOR.into_iter().zip(color))
            .filter(|(plane, _)| planes(*plane))
            .map(|(_, counts)| counts);
        luma.into_iter()
            .chain(color)
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

/// The bins of `plotted` across `bars` on the column's scale `counts`,
/// standing on the plot's ground, with `rules` over them, each a bin and its
/// ink: the luminance plane under the color ones, each where the panel's
/// toggles have it, at one column to a device pixel, and a cap in the accent
/// along the top of a column cut off there. Drawn alike in every section, so
/// that a bar reads the same down the column.
pub(super) fn bars(
    pass: &Pass,
    ui: &egui::Ui,
    plotted: &Plot,
    counts: Counts,
    bars: Rect,
    rules: &[(usize, Color32)],
) {
    let panels = pass.panels;
    let painter = ui.painter();
    painter.rect_filled(
        egui::Rect::from(bars.inset(-PLOT_INSET, -PLOT_INSET)),
        PLOT_RADIUS,
        PLOT_BACKGROUND,
    );

    // Luminance always goes down first, underneath the color planes: it
    // runs as tall as the tallest of them about as often as not, and painting
    // it on top swamps the color the panel exists to show.
    let luma: Option<&[u32; BINS]> = panels.show_luma.then_some(&plotted.luma);
    let color: &[[u32; BINS]] = match plotted.color.as_ref() {
        Some(planes) if panels.show_planes => planes,
        _ => &[],
    };

    // Each plane against its own pixels, on the column's one scale.
    let luma_total = luma.map(total);
    let color_totals: Vec<u64> = color.iter().map(total).collect();

    // Held back under the color planes; with them off — or on a gray
    // image, which has none — it is the plot.
    let luma_ink = if color.is_empty() {
        HISTOGRAM_LUMA
    } else {
        HISTOGRAM_LUMA.with_alpha(HISTOGRAM_LUMA_UNDER)
    };

    // One column to a device pixel — the bins are a point each, which is
    // what the panel's width was fixed for, and a point is a device pixel
    // only at one to one — standing as tall as the fullest bin under it, cut
    // into stretches by the heights of the planes standing in it, each
    // stretch filled with what the planes over it come to. On the device's
    // grid, like every other mark here.
    let grid = pass.grid;
    let snap = |value: f32| grid.snap(value);
    for column in grid.columns(bars.x, bars.width) {
        let (left, right) = (column.left, column.right);
        let bins = bins_under(column.index, column.count);
        let mut heights: Vec<(f32, usize)> = Vec::with_capacity(4);
        if let (Some(plane), Some(total)) = (luma, luma_total) {
            heights.push((counts.up(tallest(plane, bins.clone()), total), 3));
        }
        for (index, (plane, &total)) in color.iter().zip(&color_totals).enumerate() {
            heights.push((counts.up(tallest(plane, bins.clone()), total), index));
        }
        heights.sort_by(|a, b| a.0.total_cmp(&b.0));
        let cut = heights.last().is_some_and(|&(height, _)| height > 1.0);
        let heights = heights
            .into_iter()
            .map(|(height, plane)| (height.min(1.0) * bars.height, plane));
        let mut cover = Cover {
            luma: luma.is_some(),
            planes: [!color.is_empty(); 3],
        };
        let mut from = 0.0;
        for (height, plane) in heights {
            let (bottom, top) = (snap(bars.bottom() - from), snap(bars.bottom() - height));
            if top < bottom {
                painter.rect_filled(
                    egui::Rect::from_min_max(pos2(left, top), pos2(right, bottom)),
                    0.0,
                    screened(luma_ink, cover),
                );
            }
            from = height;
            match plane {
                3 => cover.luma = false,
                plane => cover.planes[plane] = false,
            }
        }
        if cut {
            painter.rect_filled(
                egui::Rect::from_min_max(
                    pos2(left, snap(bars.y)),
                    pos2(right, snap(bars.y) + grid.line_width(CUT_CAP)),
                ),
                0.0,
                pass.theme.accent,
            );
        }
    }

    // The pointer's rule: over the bins it is picking one of, and under
    // anything drawn over them, which is half of what the readout says
    // where there is a curve and the line must not cover it.
    //
    // Full height, where the window's own marks are handles on the band. A
    // rule standing through the plot is what a pointer wants and what a
    // permanent annotation does not: this one is only there while it is
    // being aimed, and it has to be followed up from the axis to the curve.
    for &(bin, ink) in rules {
        let across = bin_across(bin);
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

/// Words in the two top corners of the plot `bars`, each in its own ink:
/// the shares the screen clips at either end, or the ends of the axis.
/// Backed with the plot's own ground, laid over whatever bar has climbed
/// into the corner, so that the number stays a number on a picture that has
/// piled up at one end. Pinned to the ends of the axis they name rather
/// than set together in one corner: each is about the edge of the plot it
/// stands at.
pub(super) fn corners(
    ui: &egui::Ui,
    bars: Rect,
    left: Option<(String, Color32)>,
    right: Option<(String, Color32)>,
) {
    let painter = ui.painter();
    let font = FontId::proportional(CLIP_TEXT);
    for (words, at_right) in [(left, false), (right, true)] {
        let Some((words, ink)) = words else {
            continue;
        };
        let width = width_of(ui, &words, CLIP_TEXT);
        let x = if at_right {
            bars.right() - CLIP_INSET - width
        } else {
            bars.x + CLIP_INSET
        };
        let text = Rect::new(x, bars.y + CLIP_INSET, width, CLIP_TEXT);
        painter.rect_filled(
            egui::Rect::from(text.inset(-CLIP_PAD, -CLIP_PAD)),
            CLIP_PAD,
            PLOT_BACKGROUND.with_alpha(CLIP_BACKING_ALPHA),
        );
        painter.text(
            pos2(text.x, text.y),
            Align2::LEFT_TOP,
            words,
            font.clone(),
            ink,
        );
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
    let theme = pass.theme;
    let grid = pass.grid;
    let painter = ui.painter();
    let font = FontId::proportional(CLIP_TEXT);
    let hair = grid.line_width(1.0);
    let at = |t: f32| row.x + t * row.width;
    // The extents the labels written so far take, so that the next one out
    // is left off where it would touch.
    let mut taken: Vec<(f32, f32)> = Vec::new();
    let draw = |stop: i32, x: f32, taken: &mut Vec<(f32, f32)>| {
        painter.rect_filled(
            egui::Rect::from(grid.rect(Rect::new(x - hair / 2.0, row.y, hair, EV_TICK))),
            0.0,
            theme.text_dim,
        );
        let words = if stop == 0 {
            "0".to_string()
        } else {
            format!("{stop:+}")
        };
        let width = width_of(ui, &words, CLIP_TEXT);
        let left = (x - width / 2.0).clamp(row.x, row.right() - width);
        let span = (left - EV_LABEL_GAP / 2.0, left + width + EV_LABEL_GAP / 2.0);
        if taken.iter().any(|(a, b)| span.0 < *b && *a < span.1) {
            return;
        }
        taken.push(span);
        let ink: Color32 = if stop == 0 {
            theme.text_primary.into()
        } else {
            theme.text_dim.into()
        };
        painter.text(
            pos2(grid.snap(left), row.y + EV_TICK),
            Align2::LEFT_TOP,
            words,
            font.clone(),
            ink,
        );
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

/// The line across the plot `bars` that marks white, `white` of the way up
/// it, where white is not the top of the plot: on a surface with room above
/// white, the room above it can then be seen as the room it is rather than
/// as a clip that is not happening.
pub(super) fn white_line(pass: &Pass, ui: &egui::Ui, bars: Rect, white: f32) {
    let theme = pass.theme;
    let grid = pass.grid;
    let painter = ui.painter();
    let y = grid.snap(bars.bottom() - white * bars.height);
    painter.rect_filled(
        egui::Rect::from(grid.rect(Rect::new(bars.x, y, bars.width, grid.device_pixels(1.0)))),
        0.0,
        theme.text_dim,
    );
    let size = TEXT_SIZE * 0.75;
    painter.text(
        pos2(bars.right() - 2.0, y - size * 0.3),
        Align2::RIGHT_BOTTOM,
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
    /// and the spike runs past the top. The pile at either end, which the
    /// corners say, is left out of the scale too.
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
        let held = scale.cut_at(&plot, BINS / 2, |_| true).expect("cut");
        assert!((held - 1_000_000.0 / total as f32).abs() < 1e-6, "{held}");
        assert_eq!(scale.cut_at(&plot, 3, |_| true), None);
        assert_eq!(scale.cut_at(&plot, BINS / 2, |_| false), None);
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

    /// What the three color planes come to where they overlap, screened in
    /// the encoded values as [`screened`] screens them, the panel's painter
    /// having one blend and that one on encoded colors.
    fn overlapped(planes: [Color; COLOR]) -> [f32; 3] {
        let mut out = [0.0f32; 3];
        for (channel, value) in out.iter_mut().enumerate() {
            *value = 1.0
                - planes
                    .iter()
                    .map(|plane| 1.0 - [plane.r, plane.g, plane.b][channel] as f32 / 255.0)
                    .product::<f32>();
        }
        out
    }

    /// Each plane leads in its own channel by a wide margin, so two
    /// overlapping give a secondary and all three give a near white — and
    /// none of them is the primary itself, which at a pixel to the bin is
    /// a spike the eye cannot leave alone.
    #[test]
    fn the_color_planes_are_one_channel_each() {
        let white = overlapped(HISTOGRAM_PLANES);
        assert!(white.iter().all(|channel| *channel > 0.9), "{white:?}");
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

    /// The plane under them stands for a pixel's value, not for one of its
    /// channels, so it carries no hue; and the ground they are screened on
    /// is near black, or screening would not read.
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
}
