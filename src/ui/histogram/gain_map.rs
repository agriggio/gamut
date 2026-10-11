//! The Gain map section, for a picture that carries one: how far the map
//! lifts the picture, as a histogram in stops at the whole of the lift, with
//! how much of that lift the display's room is showing written in its
//! header beside the button that shows the map in the picture's place.

use std::sync::Arc;

use super::plot::{PLOT_BACKGROUND, bars};
use super::*;
use crate::image::auxiliary::{Auxiliary, Showing};
use crate::image::gain_map::GainMap;

/// The map's histogram, held between passes for as long as the picture's
/// map is the same one: a walk over the map, which is nothing once and too
/// much for every frame.
#[derive(Clone)]
struct Held {
    map: Arc<GainMap>,
    plot: Option<Arc<Plot>>,
}

/// The picture's gain map's histogram, in stops — see
/// [`GainMap::lift_plot`] — or `None` for a picture with no map, or a map
/// that lifts nothing.
pub(super) fn lift_plot(ui: &egui::Ui, current: &Current) -> Option<Arc<Plot>> {
    let map = current.picture_face().image.gain_map.as_ref()?;
    let id = egui::Id::new("histogram gain map");
    if let Some(held) = ui.data(|data| data.get_temp::<Held>(id))
        && Arc::ptr_eq(&held.map, map)
    {
        return held.plot;
    }
    let plot = map.lift_plot().map(Arc::new);
    ui.data_mut(|data| {
        data.insert_temp(
            id,
            Held {
                map: Arc::clone(map),
                plot: plot.clone(),
            },
        )
    });
    plot
}

/// A number of stops as the section writes it: to two places with the
/// zeros it does not need taken off, so that the ends of the axis read `0`
/// and `1.72`.
fn stops_words(stops: f32) -> String {
    trimmed(format!("{stops:.2}"))
}

/// A lift as the header writes it: signed, to a tenth of a stop.
fn lift_words(stops: f32) -> String {
    // Adding zero makes a negative zero positive.
    format!("{:+.1}", stops + 0.0)
}

/// What the section says with the pointer on its plot at `bin`.
pub(super) fn bin_mark(lift: Option<&Plot>, bin: usize) -> Mark {
    let Some(lift) = lift else {
        return Mark::default();
    };
    Mark {
        bin: Some(bin),
        words: format!("{} stops", lift_words(bin_value(lift, bin))),
        ..Mark::default()
    }
}

/// What the section says with the pointer on the picture's pixel `(x, y)`:
/// how far the map lifts it at the whole of the lift, a channel each for a
/// map with three. Read at the same place in the scene whichever of the
/// file's images is up, the map being read by the fraction of the way
/// across and down.
pub(super) fn pixel_mark(current: &Current, lift: Option<&Plot>, x: u32, y: u32) -> Option<Mark> {
    let map = current.picture_face().image.gain_map.as_deref()?;
    let [width, height] = current.pixels();
    if x >= width || y >= height {
        return None;
    }
    let size = [current.image.width, current.image.height];
    let [x, y] = current.turn.stored([x, y], size);
    let stops = map.stops_at(x, y, size[0], size[1]);
    let mean = stops.iter().sum::<f32>() / stops.len() as f32;
    let words = if map.channels == 1 {
        format!("{} stops", lift_words(stops[0]))
    } else {
        format!("{} stops", stops.map(lift_words).join(" / "),)
    };
    Some(Mark {
        bin: lift.and_then(|lift| lift.bin_at(mean)),
        words,
        ..Mark::default()
    })
}

/// What the header says with nothing under the pointer: what the map
/// holds, and how much of it is on screen.
fn summary(current: &Current, map: &GainMap) -> String {
    let weight = current
        .picture_face()
        .lift
        .as_ref()
        .map_or(0.0, |lift| lift.weight());
    let range = match map.lift_range() {
        Some([low, high]) => format!("{}\u{2013}{} stops", stops_words(low), stops_words(high)),
        None => "no lift".to_string(),
    };
    let shown = if weight >= 1.0 {
        "all shown".to_string()
    } else if weight <= 0.0 {
        "none shown".to_string()
    } else {
        format!("{:.1} shown", map.applied_stops(weight))
    };
    format!("{range}, {shown}")
}

/// The Gain map section, laid out at `rect`: the button that shows the map,
/// and the histogram of its lift.
/// How much of the lift is on screen is the header's to say, in words.
pub(super) fn show(
    pass: &mut Pass,
    ui: &mut egui::Ui,
    current: &Current,
    rect: Rect,
    plots: &Plots,
    mark: Option<&Mark>,
) {
    let Some(map) = current.picture_face().image.gain_map.as_deref() else {
        return;
    };
    let plot_rect = Section::GainMap.plot(rect);
    // The switch that shows the map in the picture's place, at the end of
    // the header: an eye, since what it changes is what is on screen, and
    // lit while the map is.
    icon_button(
        pass,
        ui,
        section::button(rect),
        Control::ShowGainMap,
        current.showing == Showing::Auxiliary(Auxiliary::GainMap),
        icon::EYE,
    );

    let readout = mark
        .map(|mark| mark.words.clone())
        .unwrap_or_else(|| summary(current, map));
    section::header(pass, ui, Section::GainMap, rect, Some(&readout), None);

    let Some(lift) = plots.lift else {
        ui.painter().rect_filled(
            egui::Rect::from(plot_rect.inset(-PLOT_INSET, -PLOT_INSET)),
            PLOT_RADIUS,
            PLOT_BACKGROUND,
        );
        return;
    };
    bars(
        pass,
        ui,
        Section::GainMap,
        lift,
        plots.lift_counts,
        plot_rect,
        &accent_rule(pass, mark),
    );
    let (low, high) = (lift.min, lift.max);

    // The row of stops under the plot: each whole stop of lift, evenly
    // spaced, the axis being in stops.
    plot::ev_scale(
        pass,
        ui,
        ev_row(plot_rect, plot_rect.bottom() + PLOT_INSET),
        low.min(high).floor() as i32..=low.max(high).ceil() as i32,
        |stop| Some((stop as f32 - low) / (high - low)),
    );
}
