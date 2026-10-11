//! The Image section: the picture as the file stores it — no lift, and in
//! its own primaries — at the foot of the column, where everything above it
//! starts from. Named for the picture rather than for the file, since the
//! gain map above it is the file's too.
//!
//! The pixel under the pointer is marked here a channel at a time: a rule
//! on each plane, and each channel's value in the header, in the plane's
//! own color and in the units the file keeps it in, as the bottom bar's
//! Decimal readout writes them.
//!
//! Its axis is the whole range the file's samples can hold where they are
//! integers — a raw's from black to the sensor's saturation, so that how
//! far a picture stands short of clipping is the room to the right of it —
//! and the range they were measured over where they are floats, which have
//! no top to run to: in stops where the floats are light
//! ([`Axis::Stops`]), with a row of stops from the file's own 1 under it.

use std::sync::Arc;

use egui::FontId;

use super::plot::{self, Counts, bars};
use super::*;
use crate::image::Samples;
use crate::image::stats::{Axis, Fine};
use crate::ui::pixel::component;

/// The room between one of the pixel's values and the next in the header.
const VALUE_GAP: f32 = 8.0;

/// How far either way of 1 the row of stops under an axis in stops is
/// looked for over: past either end of any such axis, whose reach down from
/// its top is bounded and whose top is a float's.
const STOP_REACH: std::ops::RangeInclusive<i32> = -64..=64;

/// The section's plot where it is not the stored scan's own, held between
/// passes for as long as the scan is the same one.
#[derive(Clone)]
struct Held {
    fine: Arc<Fine>,
    plot: Option<Arc<Plot>>,
}

/// The section's plot where the stored scan's own axis is not the one it
/// stands on — integer samples measured over less than their whole range,
/// as a linear file's are — or `None` where it is: see [`rebinned`].
pub(super) fn plotted(ui: &egui::Ui, current: &Current) -> Option<Arc<Plot>> {
    let id = egui::Id::new("histogram image");
    let fine = &current.stored.fine;
    if let Some(held) = ui.data(|data| data.get_temp::<Held>(id))
        && Arc::ptr_eq(&held.fine, fine)
    {
        return held.plot;
    }
    let plot = rebinned(current).map(Arc::new);
    ui.data_mut(|data| {
        data.insert_temp(
            id,
            Held {
                fine: Arc::clone(fine),
                plot: plot.clone(),
            },
        )
    });
    plot
}

/// The picture as stored on the whole range its integer samples can hold,
/// rebinned from the scan's fine bins, or `None` where the scan already
/// stands on it — a file on a curve, whose axis starts there — or where the
/// samples are floats, whose axis is what they were measured over.
pub(super) fn rebinned(current: &Current) -> Option<Plot> {
    let plot = &current.stored.plot;
    if float(current) || (plot.min == 0.0 && plot.max == 1.0) {
        return None;
    }
    Some(current.stored.fine.rebinned([0.0, 1.0], |value| value))
}

/// Whether the file's samples are floats, which are written as numbers
/// rather than as counts.
fn float(current: &Current) -> bool {
    matches!(current.image.samples, Samples::F32 { .. })
}

/// A value on the plot's axis — which is the file's samples as fractions of
/// full scale — in the units the file keeps them in: counts for integer
/// samples, the number itself for floats, as [`component`] writes them.
fn stored_words(current: &Current, axis: f32) -> String {
    component(axis * current.image.samples.full_scale(), float(current))
}

/// The values written along `plot`'s axis, each where it falls on the axis
/// and in the units the file keeps its samples in. Integer samples have
/// their whole range in quarters — 0, 64, 128, 192 and 255 for eight bits,
/// the quarters of a power of two, the last held to the top — and floats,
/// whose axis is only as wide as what they hold, its two ends. None on an
/// axis in stops, which has a row of stops instead: see [`stop_across`].
pub(super) fn ticks(current: &Current, plot: &Plot) -> Vec<(f32, String)> {
    let span = plot.max - plot.min;
    if span.is_nan() || span <= 0.0 || current.stored.axis == Axis::Stops {
        return Vec::new();
    }
    let along = |axis: f32| (axis - plot.min) / span;
    let full = current.image.samples.full_scale();
    if float(current) {
        return [plot.min, plot.max]
            .into_iter()
            .map(|axis| (along(axis), ends_words(axis * full, true)))
            .collect();
    }
    (0..=4)
        .map(|quarter| {
            let stored = ((full + 1.0) * quarter as f32 / 4.0).min(full);
            (along(stored / full), ends_words(stored, false))
        })
        .collect()
}

/// Where `stop` stops from the file's own 1 falls along `plot`, an axis in
/// stops, as a fraction of the way along it: the row of stops under the
/// plot of light stored as floats, which has no top to write and whose
/// values across twenty stops would be a column of zeros at three places.
/// Counted from 1 rather than from the window's white, which the Display
/// section's row counts from: this is the file's own scale, and holds
/// still while the window and the exposure move.
pub(super) fn stop_across(plot: &Plot, stop: i32) -> Option<f32> {
    let span = plot.max - plot.min;
    (span > 0.0).then(|| (stop as f32 - plot.min) / span)
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

/// What the section says with the pointer on its plot, `plot`, at `bin`:
/// the value the bin stands for, in the units the file keeps its samples in.
pub(super) fn bin_mark(current: &Current, plot: &Plot, bin: usize) -> Mark {
    Mark {
        bin: Some(bin),
        words: stored_words(current, bin_value(plot, bin)),
        ..Mark::default()
    }
}

/// What the section says with the pointer on the picture's pixel `(x, y)`:
/// the pixel as the file stores it, a channel at a time — red, green and
/// blue as the file holds them, and the luminance worked out from them as
/// the plot's luminance plane is, put back on the file's curve and scale —
/// each with the bin of its own plane of `plot`, the section's. A gray file
/// has the one channel.
pub(super) fn pixel_mark(current: &Current, plot: &Plot, x: u32, y: u32) -> Option<Mark> {
    let sample = current.sample_as_stored(x, y)?;
    let image = &current.image;
    let axis = current.stored.axis;
    let scale = 1.0 / image.samples.full_scale();
    let mut channels = Vec::with_capacity(4);
    if !image.is_gray() {
        for (plane, stored) in Plane::COLOR.into_iter().zip(sample.stored()) {
            channels.push(Channel {
                plane,
                bin: plot.bin_at(axis.place_stored(stored * scale)),
                words: component(*stored, float(current)),
            });
        }
    }
    let luma = axis.place_of(image, &sample)?;
    channels.push(Channel {
        plane: Plane::Luma,
        bin: plot.bin_at(luma),
        words: stored_words(current, luma),
    });
    Some(Mark {
        channels,
        ..Mark::default()
    })
}

/// The Image section, laid out at `rect`: the header, with the pixel's
/// values in it, and `plotted`, the plot of the picture as stored, with a
/// rule in each plane's color for the pixel's channels.
pub(super) fn show(
    pass: &mut Pass,
    ui: &mut egui::Ui,
    current: &Current,
    rect: Rect,
    plotted: &Plot,
    counts: Counts,
    mark: Option<&Mark>,
) {
    let plot_rect = Section::Image.plot(rect);
    // A rule for each channel, the luminance last
    // so that it stands over the colors where they meet; and the accent's
    // one rule where the pointer is on the plot instead.
    let rules: Vec<(usize, Color32)> = match mark {
        Some(mark) if !mark.channels.is_empty() => mark
            .channels
            .iter()
            .filter_map(|channel| Some((channel.bin?, channel.plane.ink())))
            .collect(),
        _ => accent_rule(pass, mark),
    };
    bars(pass, ui, Section::Image, plotted, counts, plot_rect, &rules);
    let row = ev_row(plot_rect, plot_rect.bottom() + PLOT_INSET);
    match current.stored.axis {
        Axis::Stops => plot::ev_scale(pass, ui, row, STOP_REACH, |stop| stop_across(plotted, stop)),
        Axis::Curve(_) => plot::value_scale(pass, ui, row, &ticks(current, plotted)),
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
/// red, green and blue, and the luminance last. Each is set right in a cell
/// as wide as the widest value the file's samples can be written as, so that
/// the numbers hold still as the pointer crosses the picture.
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
