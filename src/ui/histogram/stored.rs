//! The Image section: the picture as the file stores it — no lift, and in
//! its own primaries — at the foot of the column, where everything above it
//! starts from. Named for the picture rather than for the file, since the
//! gain map above it is the file's too.
//!
//! A plot to the stored channel, one under the other in the order the file
//! keeps them, and nothing the file does not store: no luminance, which is
//! a weighting of the channels the display works out, and not one the file
//! holds. The pixel under the pointer is marked a channel at a time: a rule
//! on each plane's plot, and each channel's value in the header, in the
//! plane's own color and in the units the file keeps it in, as the bottom
//! bar's Decimal readout writes them.
//!
//! The planes share one axis where the channels are compared against one
//! another — a photograph's, over the whole range integer samples can hold,
//! a raw's from black to the sensor's saturation, so that how far a picture
//! stands short of clipping is the room to the right of it; or light stored
//! as floats, in stops, with a row of stops from the file's own 1 under the
//! last of them ([`Axis::Stops`]) — and each its own where the file is read
//! as data, whose channels may be different quantities: each plane then has
//! its own row of values under it.

use std::sync::Arc;

use egui::FontId;

use super::plot::{self, Counts, plane_bars};
use super::*;
use crate::image::Samples;
use crate::image::stats::{self, Axis, Stored};
use crate::ui::pixel::component;

/// The room between one of the pixel's values and the next in the header.
const VALUE_GAP: f32 = 8.0;

/// How far either way of 1 the row of stops under an axis in stops is
/// looked for over: past either end of any such axis, whose reach down from
/// its top is bounded and whose top is a float's.
const STOP_REACH: std::ops::RangeInclusive<i32> = -64..=64;

/// How tall each channel's plot stands where there are three of them: read
/// for their shape against one another and against the plots above, so
/// shorter than the one plot a gray file has, which is [`SHORT_PLOT_HEIGHT`].
pub(super) const CHANNEL_PLOT_HEIGHT: f32 = 40.0;

/// The room between one channel's plot and the next: their grounds are
/// drawn out [`PLOT_INSET`] past the plots each way, and a gutter as wide
/// again between the grounds keeps them two plots rather than one striped
/// one.
const PLANE_GAP: f32 = 3.0 * PLOT_INSET;

/// How the section is laid out for a file: how many channels it plots, and
/// whether they share one axis, with one row of values under the last of
/// them, or each has its own range and its own row.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Layout {
    pub planes: usize,
    pub shared: bool,
}

impl Layout {
    /// A gray file's: the one plot, on the one axis.
    pub const ONE: Layout = Layout {
        planes: 1,
        shared: true,
    };

    pub fn of(current: &Current) -> Self {
        Self {
            planes: current.stored.planes.len().max(1),
            shared: current.stored.shared,
        }
    }

    /// How tall each plot stands.
    pub const fn plot_height(self) -> f32 {
        if self.planes == 1 {
            SHORT_PLOT_HEIGHT
        } else {
            CHANNEL_PLOT_HEIGHT
        }
    }

    /// What the section holds under its header: the plots, the gaps between
    /// them, and the rows of values — one under the last plot where the
    /// planes share an axis, one under each where they do not.
    pub const fn body_height(self) -> f32 {
        let planes = self.planes as f32;
        let rows = if self.shared { 1.0 } else { planes };
        planes * self.plot_height() + (planes - 1.0) * PLANE_GAP + rows * (PLOT_INSET + EV_ROW)
    }
}

/// Where each channel's plot goes in the section laid out at `rect`: one
/// under the other from under the header, a row of values under each where
/// the planes have their own ranges.
pub(super) fn plots(rect: Rect, layout: Layout) -> Vec<Rect> {
    let x = rect.x + PLOT_INSET;
    let width = rect.width - 2.0 * PLOT_INSET;
    let mut y = rect.y + LABEL_HEIGHT + HEAD_GAP + PLOT_INSET;
    let height = layout.plot_height();
    (0..layout.planes)
        .map(|_| {
            let plot = Rect::new(x, y, width, height);
            y += height + PLANE_GAP;
            if !layout.shared {
                y += PLOT_INSET + EV_ROW;
            }
            plot
        })
        .collect()
}

/// The first channel's plot in the section laid out at `rect`: where the
/// section's plots begin, for what asks a section for one plot.
pub(super) fn first_plot(rect: Rect, layout: Layout) -> Rect {
    plots(rect, layout)[0]
}

/// Which plane's plot the pointer is over in the section laid out at
/// `rect`, and which bin of it, or `None` off every plot.
pub(super) fn hovered(
    rect: Rect,
    layout: Layout,
    cursor: Option<[f32; 2]>,
) -> Option<(usize, usize)> {
    plots(rect, layout)
        .into_iter()
        .enumerate()
        .find_map(|(plane, bars)| Some((plane, hovered_bin(bars, cursor)?)))
}

/// The section's plots, poured from the stored tallies once an arrival and
/// held between passes for as long as the stored scan is the same one: a
/// plot per plane, and the count axis they share.
pub(super) struct Plotted {
    stored: Arc<Stored>,
    pub planes: Vec<[u32; BINS]>,
    pub counts: Counts,
}

impl Plotted {
    fn of(stored: &Arc<Stored>) -> Self {
        let planes: Vec<[u32; BINS]> = stored
            .planes
            .iter()
            .map(|plane| plane.plot(stored.axis))
            .collect();
        Self {
            stored: Arc::clone(stored),
            counts: Counts::over(planes.iter()),
            planes,
        }
    }
}

/// The plots of `current`'s file as stored, from the pass before where the
/// stored scan is the one they were poured from, and poured again where it
/// is not.
pub(super) fn plotted(ui: &egui::Ui, current: &Current) -> Arc<Plotted> {
    let id = egui::Id::new("histogram image");
    if let Some(held) = ui.data(|data| data.get_temp::<Arc<Plotted>>(id))
        && Arc::ptr_eq(&held.stored, &current.stored)
    {
        return held;
    }
    let made = Arc::new(Plotted::of(&current.stored));
    ui.data_mut(|data| data.insert_temp(id, made.clone()));
    made
}

/// The plots of `current`'s file as stored, worked out afresh: what a test
/// holds the trace against, with no pass to keep them in.
#[cfg(test)]
pub(super) fn plotted_for(current: &Current) -> Plotted {
    Plotted::of(&current.stored)
}

/// Whether the file's samples are floats, which are written as numbers
/// rather than as counts.
fn float(current: &Current) -> bool {
    matches!(current.image.samples, Samples::F32 { .. })
}

/// The plane the `index`th stored channel is drawn as: a gray file's one
/// channel in the neutral, and a color file's in its channel's own.
fn plane_of(current: &Current, index: usize) -> Plane {
    if current.image.is_gray() {
        Plane::Luma
    } else {
        Plane::COLOR[index.min(Plane::COLOR.len() - 1)]
    }
}

/// A value on a plane's axis, in the units the file keeps its samples in:
/// counts for integer samples, the number itself for floats, as
/// [`component`] writes them. On an axis in stops the value is stops of
/// the file's own 1, and the number is what stands there.
fn stored_words(current: &Current, place: f32) -> String {
    component(
        stored_at(current, place) * current.image.samples.full_scale(),
        float(current),
    )
}

/// The value as stored standing at `place` on a plane's axis: a fraction
/// of full scale for integer samples, the number itself for floats.
fn stored_at(current: &Current, place: f32) -> f32 {
    match current.stored.axis {
        Axis::Stops => place.exp2(),
        Axis::Curve(_) => place,
    }
}

/// The value at the middle of `bin` on the axis `range` spans.
fn bin_value_in(range: [f32; 2], bin: usize) -> f32 {
    range[0] + bin_across(bin) * (range[1] - range[0])
}

/// The values written along `plane`'s axis, each where it falls on the
/// axis and in the units the file keeps its samples in. A photograph's
/// integer samples have their whole range in quarters — 0, 64, 128, 192 and
/// 255 for eight bits, the quarters of a power of two, the last held to the
/// top — and any other plane its two ends. None on an axis in stops, which
/// has a row of stops instead: see [`stop_across`].
pub(super) fn ticks(current: &Current, plane: &stats::Plane) -> Vec<(f32, String)> {
    let [low, high] = plane.range;
    let span = high - low;
    if span.is_nan() || span <= 0.0 || current.stored.axis == Axis::Stops {
        return Vec::new();
    }
    let along = |axis: f32| (axis - low) / span;
    let full = current.image.samples.full_scale();
    let whole = !float(current) && current.stored.shared && plane.range == [0.0, 1.0];
    if !whole {
        return [low, high]
            .into_iter()
            .map(|axis| (along(axis), ends_words(axis * full, float(current))))
            .collect();
    }
    (0..=4)
        .map(|quarter| {
            let stored = ((full + 1.0) * quarter as f32 / 4.0).min(full);
            (along(stored / full), ends_words(stored, false))
        })
        .collect()
}

/// Where `stop` stops from the file's own 1 falls along an axis in stops
/// spanning `range`, as a fraction of the way along it: the row of stops
/// under the plot of light stored as floats, which has no top to write and
/// whose values across twenty stops would be a column of zeros at three
/// places. Counted from 1 rather than from the window's white, which the
/// Display section's row counts from: this is the file's own scale, and
/// holds still while the window and the exposure move.
pub(super) fn stop_across(range: [f32; 2], stop: i32) -> Option<f32> {
    let span = range[1] - range[0];
    (span > 0.0).then(|| (stop as f32 - range[0]) / span)
}

/// A value along the axis as the row under the plot writes it: a count for
/// integer samples, and a float to three places with the zeros it does not
/// need taken off.
fn ends_words(stored: f32, float: bool) -> String {
    if float {
        trimmed(format!("{stored:.3}"))
    } else {
        format!("{stored:.0}")
    }
}

/// What the section says with the pointer on the `plane`th plot at `bin`:
/// the value the bin stands for, in the units the file keeps its samples
/// in, in the plane's own color.
pub(super) fn bin_mark(current: &Current, plane: usize, bin: usize) -> Mark {
    let stored = &current.stored.planes[plane];
    let place = bin_value_in(stored.range, bin);
    let words = stored_words(current, place);
    let rank = stored.rank(stored_at(current, place));
    Mark {
        bin: Some(bin),
        words: words.clone(),
        channels: vec![Channel {
            plane: plane_of(current, plane),
            bin: Some(bin),
            words,
            rank,
        }],
    }
}

/// What the section says with the pointer on the picture's pixel `(x, y)`:
/// the pixel as the file stores it, a channel at a time — red, green and
/// blue as the file holds them, or a gray file's one channel — each with
/// the bin of its own plane's plot.
pub(super) fn pixel_mark(current: &Current, x: u32, y: u32) -> Option<Mark> {
    let sample = current.sample_as_stored(x, y)?;
    let image = &current.image;
    let stored = &current.stored;
    let scale = 1.0 / image.samples.full_scale();
    let channels = stored
        .planes
        .iter()
        .zip(sample.stored())
        .enumerate()
        .map(|(index, (plane, &value))| Channel {
            plane: plane_of(current, index),
            bin: plane.bin_at(stored.axis.place_stored(value * scale)),
            words: component(value, float(current)),
            rank: plane.rank(value * scale),
        })
        .collect();
    Some(Mark {
        channels,
        ..Mark::default()
    })
}

/// The Image section, laid out at `rect`: the header, with the pixel's
/// values in it, and a plot of each channel as stored, with a rule in each
/// plane's color for the pixel's channel.
pub(super) fn show(
    pass: &mut Pass,
    ui: &mut egui::Ui,
    current: &Current,
    rect: Rect,
    plotted: &Plotted,
    mark: Option<&Mark>,
) {
    let stored = &current.stored;
    let layout = Layout::of(current);
    let rects = plots(rect, layout);
    for (index, (bars, counts)) in rects.iter().zip(&plotted.planes).enumerate() {
        let plane = plane_of(current, index);
        // The pixel's rule on this plane, in the plane's color, with where
        // its value stands among the plane's pixels beside it.
        let marked: Vec<&Channel> = match mark {
            Some(mark) => mark
                .channels
                .iter()
                .filter(|channel| channel.plane == plane && channel.bin.is_some())
                .collect(),
            None => Vec::new(),
        };
        let rules: Vec<(usize, Color32)> = marked
            .iter()
            .filter_map(|channel| Some((channel.bin?, plane.ink())))
            .collect();
        let stands = plane_bars(
            pass,
            ui,
            Section::Image,
            counts,
            plane.ink(),
            plotted.counts,
            *bars,
            &rules,
        );
        for (channel, across) in marked.iter().zip(stands) {
            if let Some(rank) = channel.rank {
                plot::rule_label(ui, *bars, across, 0, &percentile_words(rank), plane.ink());
            }
        }
        // The row of values under this plot, where each plane has its own
        // axis.
        if !layout.shared
            && let Some(plane) = stored.planes.get(index)
        {
            let row = ev_row(*bars, bars.bottom() + PLOT_INSET);
            plot::value_scale(pass, ui, row, &ticks(current, plane));
        }
    }
    // The one row under the last plot, where the planes share an axis.
    if let (Some(bars), Some(range), Some(plane)) =
        (rects.last(), stored.shared_range(), stored.planes.first())
    {
        let row = ev_row(*bars, bars.bottom() + PLOT_INSET);
        match stored.axis {
            Axis::Stops => {
                plot::ev_scale(pass, ui, row, STOP_REACH, |stop| stop_across(range, stop))
            }
            Axis::Curve(_) => plot::value_scale(pass, ui, row, &ticks(current, plane)),
        }
    }
    match mark {
        Some(mark) if !mark.channels.is_empty() => {
            section::header(pass, ui, Section::Image, rect, None);
            values(pass, ui, current, rect, mark);
        }
        _ => section::header(
            pass,
            ui,
            Section::Image,
            rect,
            mark.map(|mark| mark.words.as_str()),
        ),
    }
}

/// The pixel's values at the end of the header, each in its plane's ink:
/// red, green and blue, in the order the file keeps them. Each is set right
/// in a cell as wide as the widest value the file's samples can be written
/// as, so that the numbers hold still as the pointer crosses the picture.
fn values(pass: &Pass, ui: &egui::Ui, current: &Current, rect: Rect, mark: &Mark) {
    let painter = ui.painter();
    let font = FontId::proportional(ROW_TEXT);
    let line = section::header_line(rect);
    let middle = line.y + line.height / 2.0;
    let widest = match current.image.samples {
        Samples::U8 { .. } => "255",
        Samples::U16 { .. } => "65535",
        Samples::F32 { .. } => "0.0000",
    };
    let width_of = |words: &str| {
        painter
            .layout_no_wrap(words.to_owned(), font.clone(), Color32::PLACEHOLDER)
            .size()
            .x
    };
    let cell = width_of(widest);
    let mut end = section::readout_end(Section::Image, line);
    for channel in mark.channels.iter().rev() {
        let galley =
            painter.layout_no_wrap(channel.words.clone(), font.clone(), Color32::PLACEHOLDER);
        let width = galley.size().x;
        let at = pos2(pass.grid.snap(end - width), middle - galley.size().y / 2.0);
        painter.galley(at, galley, channel.plane.ink());
        end -= width.max(cell) + VALUE_GAP;
    }
}
