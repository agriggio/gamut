//! The Output section: what goes out to the screen. The picture as the
//! display starts from it, every value pushed through the display's
//! response and binned again on the curve the screen is driven with — sRGB's
//! — from black to as far above white as the surface goes, so that what the
//! surface clips piles into the end it is clipped at. A gray image read
//! through a ramp — a false color, or a measurement — is binned along the
//! ramp instead, from one end to the other, which for the gray ramp is the
//! screen's own code: the axis is the band under it, read straight across.

use std::sync::Arc;

use super::plot::{Past, band, bars, white_mark};
use super::*;
use crate::image::Transfer;
use crate::image::display::{Colormap, Display, Headroom, ToneMap};
use crate::image::stats::{self, Axis, Fine, Quantity};
use crate::ui::FrameInput;

/// The curve the output's axis is spaced on: the one the screen is driven
/// with, so that a display doing nothing to an sRGB file plots the file's
/// own codes where its own plot does.
const ENCODING: Transfer = Transfer::Srgb;

/// The plot of what goes out, and the room above white it runs up to: the
/// display's input rebinned through its response, held between passes for
/// as long as nothing it was worked out from has changed.
pub(super) struct Binned {
    key: Key,
    /// The fine bins it was rebinned from, held so that the key's address
    /// for them cannot be another's while this is kept.
    fine: Arc<Fine>,
    pub plot: Plot,
    /// How far above white the plot's axis runs, as a ratio to white: the
    /// surface's room, or, on a surface whose room is not known, as far as
    /// the response goes. 1 along a ramp, which has no room past its end.
    pub room: f32,
    /// The ramp the axis runs along, where the image is read through one;
    /// otherwise it is the screen's light, spaced on [`ENCODING`].
    pub ramp: Option<Colormap>,
}

/// What a plot of the output is worked out from: the input's fine bins, by
/// where they are held, and everything the response reads.
#[derive(Clone, Copy, PartialEq, Debug)]
struct Key {
    fine: usize,
    offset: u32,
    gain: u32,
    tone_map: ToneMap,
    ramp: Option<Colormap>,
    headroom: Headroom,
    room: u32,
}

impl Key {
    fn of(current: &Current, input: &FrameInput) -> Self {
        let (offset, gain) = current.display.transform();
        Self {
            fine: Arc::as_ptr(&current.stats.fine) as usize,
            offset: offset.to_bits(),
            gain: gain.to_bits(),
            tone_map: current.display.tone_map(),
            ramp: current.display.ramp(current.image.is_gray()),
            headroom: input.headroom,
            room: input.room.to_bits(),
        }
    }
}

/// The plot of what goes out for `current`, from the pass before where
/// nothing it reads has changed since, and worked out again otherwise.
///
/// Rebinned from the fine bins the input was counted in, never scanned
/// again: a drag of a handle changes the response on every frame, and four
/// thousand bins through it is nothing where two million pixels through it
/// would be a frame each. Kept where egui keeps a pass's data, as the
/// information panel's contents are, since only the panel reads it.
pub(super) fn binned(ui: &egui::Ui, current: &Current, input: &FrameInput) -> Arc<Binned> {
    let id = egui::Id::new("histogram output");
    let key = Key::of(current, input);
    if let Some(held) = ui.data(|data| data.get_temp::<Arc<Binned>>(id))
        && held.key == key
        && Arc::ptr_eq(&held.fine, &current.stats.fine)
    {
        return held;
    }
    let made = Arc::new(rebin(current, input.headroom, input.room, key));
    ui.data_mut(|data| data.insert_temp(id, made.clone()));
    made
}

/// The plot of what goes out for `current`, worked out afresh: what a test
/// holds the trace against, with no pass to keep it in.
#[cfg(test)]
pub(super) fn binned_for(current: &Current, headroom: Headroom, room: f32) -> Binned {
    let input_key = Key {
        fine: Arc::as_ptr(&current.stats.fine) as usize,
        offset: 0,
        gain: 0,
        tone_map: current.display.tone_map(),
        ramp: current.display.ramp(current.image.is_gray()),
        headroom,
        room: room.to_bits(),
    };
    rebin(current, headroom, room, input_key)
}

/// What the display makes of one value at `place` on the input's axis, as
/// a ratio to white, never below black.
fn response(display: &Display, axis: Axis, gray: bool, headroom: Headroom, place: f32) -> f32 {
    display.response(axis.value(place), gray, headroom).max(0.0)
}

/// The room a plot of the output runs up to: the surface's, where it is
/// known, and otherwise as far as the response takes any value the input
/// holds, rounded up to a whole number of stops above white, and never
/// below white. An HDR surface whose monitor has not said how far it goes
/// has its room put where the picture needs it rather than nowhere.
fn resolved_room(fine: &Fine, room: f32, through: impl Fn(f32) -> f32) -> f32 {
    if room.is_finite() {
        return room.max(1.0);
    }
    let span = fine.max - fine.min;
    let planes = std::iter::once(&fine.luma).chain(fine.color.iter().flatten());
    let mut highest = 1.0f32;
    for plane in planes {
        for (index, &count) in plane.iter().enumerate() {
            if count > 0 {
                let value = fine.min + index as f32 / (plane.len() - 1) as f32 * span;
                let out = through(value);
                if out.is_finite() {
                    highest = highest.max(out);
                }
            }
        }
    }
    highest.log2().ceil().exp2().max(1.0)
}

fn rebin(current: &Current, headroom: Headroom, room: f32, key: Key) -> Binned {
    let fine = &current.stats.fine;
    let display = &current.display;
    let axis = current.stats.axis;
    let gray = current.image.is_gray();
    let ramp = display.ramp(gray);
    let through = |v| response(display, axis, gray, headroom, v);
    // Along a ramp the response is where on it a value lands, 0 to 1 and
    // never past: the axis is the ramp itself. Light is spaced as the
    // screen codes it, to the surface's room — or to white where it is
    // clipped there whatever the surface, as data is.
    let room = match ramp {
        Some(_) => 1.0,
        None if display.clips_white(gray, headroom) => 1.0,
        None => resolved_room(fine, room, through),
    };
    let plot = fine.rebinned([0.0, placed(ramp, room)], |v| placed(ramp, through(v)));
    Binned {
        key,
        fine: Arc::clone(fine),
        plot,
        room,
        ramp,
    }
}

/// Where an output value falls on the axis: its code on the screen's curve
/// for light, and its own place along a ramp.
fn placed(ramp: Option<Colormap>, out: f32) -> f32 {
    match ramp {
        Some(_) => out,
        None => ENCODING.to_encoded(out),
    }
}

/// The output value at the middle of `bin`: a ratio to white, or the place
/// along the ramp.
fn output_at(binned: &Binned, bin: usize) -> f32 {
    let at = bin_value(&binned.plot, bin);
    match binned.ramp {
        Some(_) => at,
        None => ENCODING.to_linear(at),
    }
}

/// An output value as the header writes it: a ratio to white, or the place
/// along the ramp, to three places with the zeros it does not need taken
/// off.
fn output_words(value: f32) -> String {
    trimmed(format!("{value:.3}"))
}

/// What the display makes of a linear value, held to the plot's axis as
/// the plot holds it: never below black, and never past the room, where
/// what runs over piles into the end bin.
fn output_of<'a>(
    current: &'a Current,
    headroom: Headroom,
    binned: &'a Binned,
) -> impl Fn(f32) -> f32 + 'a {
    let gray = current.image.is_gray();
    move |linear| {
        current
            .display
            .response(linear, gray, headroom)
            .max(0.0)
            .min(binned.room)
    }
}

/// Where the output value `out` stands among what the display makes of
/// each pixel's `plane`, as a fraction of them: ranked after the response
/// rather than before it, so that a pile the response clips together
/// stands in its middle, as its bar does. The response to a gray never
/// falls — a window is never inside out, [`Display::set_window`]
/// refusing one — which is what lets the tally be searched rather than
/// walked: see `the_response_to_a_gray_never_falls` in the tests.
fn output_rank(
    current: &Current,
    headroom: Headroom,
    binned: &Binned,
    plane: Plane,
    out: f32,
) -> Option<f32> {
    current
        .stats
        .rank_through(plane.quantity(), out, output_of(current, headroom, binned))
}

/// What the section says with the pointer on its plot at `bin`: the output
/// value the bin stands for, and where it stands among what each plane's
/// pixels come out as.
pub(super) fn bin_mark(current: &Current, headroom: Headroom, binned: &Binned, bin: usize) -> Mark {
    // The top bin's value is the room itself, which a round trip through
    // the screen's curve would come back from a hair off, and miss the
    // pile there.
    let out = if bin == BINS - 1 {
        binned.room
    } else {
        output_at(binned, bin)
    };
    let channels = Plane::drawn(current)
        .into_iter()
        .map(|plane| Channel {
            plane,
            bin: None,
            words: String::new(),
            rank: output_rank(current, headroom, binned, plane, out),
        })
        .collect();
    Mark {
        bin: Some(bin),
        words: output_words(output_at(binned, bin)),
        channels,
    }
}

/// What the section says with the pointer on the picture's pixel `(x, y)`:
/// what the screen puts out for it, which is the display's response held
/// to the surface's room; and each plane of it — its luminance and its
/// channels, each put through the response as the plot puts them — with
/// its bin and where it stands among what that plane's pixels come out as.
pub(super) fn pixel_mark(
    current: &Current,
    headroom: Headroom,
    binned: &Binned,
    x: u32,
    y: u32,
) -> Option<Mark> {
    let sample = current.sample(x, y)?;
    let stats = &current.stats;
    let axis = stats.axis;
    let place = axis.place_of(&current.image, &sample)?;
    let output = output_of(current, headroom, binned);
    let out = output(axis.value(place));
    let channels = Plane::drawn(current)
        .into_iter()
        .map(|plane| {
            let value = match plane.quantity() {
                Quantity::Luminance => stats::luminance_of(&sample),
                Quantity::Channel(channel) => sample.linear()[channel],
            };
            let bin = match plane.quantity() {
                Quantity::Luminance => binned.plot.bin_at(placed(binned.ramp, out)),
                Quantity::Channel(_) => binned.plot.bin_at(placed(binned.ramp, output(value))),
            };
            // Ranked as the bin the pixel was counted in comes out, so that
            // the pixel stands among the pixels it was counted with rather
            // than a hair to one side of them.
            let rank = stats
                .binned(plane.quantity(), value)
                .and_then(|binned_value| {
                    output_rank(current, headroom, binned, plane, output(binned_value))
                });
            Channel {
                plane,
                bin,
                words: String::new(),
                rank,
            }
        })
        .collect();
    Some(Mark {
        bin: binned.plot.bin_at(placed(binned.ramp, out)),
        words: output_words(out),
        channels,
    })
}

/// The Output section, laid out at `rect`: the plot of what goes out with
/// white marked where white is not its top, and the band of what each
/// output is.
pub(super) fn show(
    pass: &mut Pass,
    ui: &mut egui::Ui,
    current: &Current,
    rect: Rect,
    plots: &Plots,
    mark: Option<&Mark>,
) {
    let binned = plots.output;
    let plot_rect = Section::Output.plot(rect);
    let stands = bars(
        pass,
        ui,
        Section::Output,
        &binned.plot,
        plots.output_counts,
        plot_rect,
        &plane_rules(pass, mark),
    );

    let top = placed(binned.ramp, binned.room);
    let white = placed(binned.ramp, 1.0) / top;
    if white < 1.0 - 1e-3 {
        white_mark(pass, ui, plot_rect, white, 1.0);
    }
    plane_labels(ui, plot_rect, mark, &stands);

    // The row of stops under the band: 0 at white, below it how far under
    // white an output is, and on a surface with room above white how far
    // past it the screen goes, up to the room. Exact, the output's values
    // being ratios to white already. Left off an axis that is a ramp, whose
    // places are not light, and off a measurement, as the Display section's
    // row is: its values are quantities rather than light, and stops of the
    // screen's light would read as stops of them.
    if binned.ramp.is_none() && current.image.referred != crate::image::Referred::Measured {
        plot::ev_scale(
            pass,
            ui,
            ev_row(plot_rect, ramp(plot_rect).bottom()),
            -24..=binned.room.log2().ceil() as i32,
            |stop| Some(ENCODING.to_encoded((stop as f32).exp2()) / top),
        );
    }

    // What each output comes out as, along the foot: gray from black to
    // white, chevrons in the accent over the stretch past white where the
    // surface goes there, and along a ramp the ramp itself, end to end.
    band(pass, ui, plot_rect, Past::Chevrons, |t| match binned.ramp {
        Some(ramp) => (Color::from_linear(ramp.color(t)), false),
        None => {
            let out = ENCODING.to_linear(t * top);
            (
                Color::from_linear([out.clamp(0.0, 1.0); 3]),
                out > ABOVE_WHITE,
            )
        }
    });

    section::header(
        pass,
        ui,
        Section::Output,
        rect,
        mark.map(|mark| mark.words.as_str()),
    );
}
