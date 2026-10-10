//! The Image section: the picture as the file stores it — no lift, and in
//! its own primaries — at the foot of the column, where everything above it
//! starts from. Named for the picture rather than for the file, since the
//! gain map above it is the file's too.
//!
//! The pixel under the pointer is marked here a channel at a time: a rule
//! on each plane, and each channel's value in the header, in the plane's
//! own color and in the units the file keeps it in, as the bottom bar's
//! Decimal readout writes them.

use egui::FontId;

use super::plot::{self, Counts, bars};
use super::*;
use crate::image::Samples;
use crate::ui::pixel::component;

/// The room between one of the pixel's values and the next in the header.
const VALUE_GAP: f32 = 8.0;

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

/// The values written along the plot's axis, each where it falls on the
/// axis and in the units the file keeps its samples in. A file on a curve
/// has its full scale in quarters — 0, 64, 128, 192 and 255 for eight bits,
/// the quarters of a power of two, the last held to the top — and a linear
/// one, whose axis is only as wide as what it holds, its two ends.
fn ticks(current: &Current) -> Vec<(f32, String)> {
    let plot = &current.stored.plot;
    let span = plot.max - plot.min;
    if span.is_nan() || span <= 0.0 {
        return Vec::new();
    }
    let along = |axis: f32| (axis - plot.min) / span;
    let full = current.image.samples.full_scale();
    let float = float(current);
    if current.image.color.transfer.is_linear() {
        return [plot.min, plot.max]
            .into_iter()
            .map(|axis| (along(axis), ends_words(axis * full, float)))
            .collect();
    }
    (0..=4)
        .map(|quarter| {
            let stored = if float {
                quarter as f32 / 4.0
            } else {
                ((full + 1.0) * quarter as f32 / 4.0).min(full)
            };
            (along(stored / full), ends_words(stored, float))
        })
        .collect()
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

/// What the section says with the pointer on its plot at `bin`: the value
/// the bin stands for, in the units the file keeps its samples in.
pub(super) fn bin_mark(current: &Current, bin: usize) -> Mark {
    Mark {
        bin: Some(bin),
        words: stored_words(current, bin_value(&current.stored.plot, bin)),
        ..Mark::default()
    }
}

/// What the section says with the pointer on the picture's pixel `(x, y)`:
/// the pixel as the file stores it, a channel at a time — red, green and
/// blue as the file holds them, and the luminance worked out from them as
/// the plot's luminance plane is, put back on the file's curve and scale —
/// each with the bin of its own plane. A gray file has the one channel.
pub(super) fn pixel_mark(current: &Current, x: u32, y: u32) -> Option<Mark> {
    let sample = current.sample_as_stored(x, y)?;
    let image = &current.image;
    let plot = &current.stored.plot;
    let scale = 1.0 / image.samples.full_scale();
    let mut channels = Vec::with_capacity(4);
    if !image.is_gray() {
        for (plane, stored) in Plane::COLOR.into_iter().zip(sample.stored()) {
            channels.push(Channel {
                plane,
                bin: plot.bin_at(stored * scale),
                words: component(*stored, float(current)),
            });
        }
    }
    let luma = Plot::value_of(image, &sample)?;
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
/// values in it, and the plot of the picture as stored, with a
/// rule in each plane's color for the pixel's channels.
pub(super) fn show(
    pass: &mut Pass,
    ui: &mut egui::Ui,
    current: &Current,
    rect: Rect,
    counts: Counts,
    mark: Option<&Mark>,
) {
    let plot_rect = Section::Image.plot(rect);
    let plotted = &current.stored.plot;
    // A rule for each channel whose plane is on screen, the luminance last
    // so that it stands over the colors where they meet; and the accent's
    // one rule where the pointer is on the plot instead.
    let rules: Vec<(usize, Color32)> = match mark {
        Some(mark) if !mark.channels.is_empty() => mark
            .channels
            .iter()
            .filter(|channel| channel.plane.shown(pass.panels))
            .filter_map(|channel| Some((channel.bin?, channel.plane.ink())))
            .collect(),
        _ => accent_rule(pass, mark),
    };
    bars(pass, ui, plotted, counts, plot_rect, &rules);
    plot::value_scale(
        pass,
        ui,
        ev_row(plot_rect, plot_rect.bottom() + PLOT_INSET),
        &ticks(current),
    );
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
    let mut end = section::readout_end(line);
    for channel in mark.channels.iter().rev() {
        let galley =
            painter.layout_no_wrap(channel.words.clone(), font.clone(), Color32::PLACEHOLDER);
        let width = galley.size().x;
        let at = pos2(pass.grid.snap(end - width), middle - galley.size().y / 2.0);
        painter.galley(at, galley, channel.plane.ink());
        end -= width.max(cell) + VALUE_GAP;
    }
}
