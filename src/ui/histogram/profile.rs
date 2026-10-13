//! The Profile section, for a raw whose profile says how it should look:
//! how far the profile's rendering lifts each pixel, as a histogram in
//! stops, as the Gain map section plots a photograph's lift; and under it
//! the rendering's three parts, read upward in the order they are applied,
//! as the column is: the tone curve, the gain table with the exposure in
//! it, and the exposure's own row. The square at the end of the header is
//! the profile's switch, and the header says whether it is on.

use std::sync::Arc;

use egui::Stroke;

use super::plot::{PLOT_BACKGROUND, bars};
use super::*;
use crate::image::gain_map::{GainMap, GainTable, Profile};

/// How far below white the curve's and the table's inputs reach, in stops:
/// past the toe of every profile sampled, which falls away below four.
const CURVE_STOPS: i32 = 10;

/// How big the dot is that places the pointer's pixel on a curve.
const PIXEL_DOT: f32 = 2.5;

/// How strongly the spread of the gain table's points is laid on its
/// ground: a wash of the accent, behind the line it is the range of.
const SPREAD_ALPHA: f32 = 0.35;

/// Whether the profile's rendering is on screen.
fn applied(current: &Current) -> bool {
    current
        .picture_face()
        .lift
        .as_ref()
        .is_some_and(|lift| lift.weight() > 0.0)
}

/// What the header says with nothing under the pointer: whether the
/// rendering is on screen.
fn summary(applied: bool) -> &'static str {
    if applied { APPLIED } else { NOT_APPLIED }
}

/// A number of stops of exposure as the section writes it: signed, with a
/// true minus, to `places`.
fn exposure_words(stops: f32, places: usize) -> String {
    // Adding zero makes a negative zero positive.
    format!("{:+.*} EV", places, stops + 0.0).replace('-', "\u{2212}")
}

/// The pointer's pixel, as the profile's parts read it: the developed
/// color in the picture's own primaries, the ratios the whole of the
/// profile lifts it by, and where it is as a share of the picture's height
/// and width — whether or not the switch has the rendering on screen,
/// since the curves are the profile's.
struct Pixel {
    developed: [f32; 3],
    ratios: [f32; 3],
    place: [f32; 2],
}

impl Pixel {
    /// The pixel at `(x, y)` of the turned picture, where it is on it.
    fn at(current: &Current, map: &GainMap, x: u32, y: u32) -> Option<Self> {
        let face = current.picture_face();
        let size = [face.image.width, face.image.height];
        let [width, height] = current.turn.size(size);
        if x >= width || y >= height {
            return None;
        }
        let [x, y] = current.turn.stored([x, y], size);
        let sample = face.image.sample_as_stored(x, y)?;
        let linear = sample.linear();
        Some(Self {
            developed: [0, 1, 2].map(|c| linear.get(c).copied().unwrap_or(linear[0])),
            ratios: map.gain_at(&map.table(1.0), x, y, size[0], size[1]),
            place: [
                (y as f32 + 0.5) / size[1] as f32,
                (x as f32 + 0.5) / size[0] as f32,
            ],
        })
    }

    /// Where the pixel stands on the gain table: the brightness the table
    /// reads it at once exposed, which is the input the table is indexed
    /// by, and the lift the exposure and the table give it there, in stops.
    fn on_table(&self, profile: &Profile, table: &GainTable) -> [f32; 2] {
        let exposed: [f32; 3] = std::array::from_fn(|row| {
            let matrix = profile.to_curve[row];
            (matrix[0] * self.developed[0]
                + matrix[1] * self.developed[1]
                + matrix[2] * self.developed[2])
                * profile.baseline.exp2()
        });
        let input = table.input(exposed);
        [
            input,
            profile.baseline + table.gain_for(input, self.place).log2(),
        ]
    }
}

/// The Profile section, laid out at `rect`: the switch, the histogram of
/// the lift, and the rendering's parts.
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
    let Some(profile) = map.profile() else {
        return;
    };
    let applied = applied(current);
    // The profile's switch, at the end of the header: lit while the
    // profile's rendering is on screen.
    icon_button(
        pass,
        ui,
        section::button(rect),
        Control::Profile,
        applied,
        icon::CAMERA,
    );
    let readout = mark
        .map(|mark| mark.words.clone())
        .unwrap_or_else(|| summary(applied).to_string());
    section::header(pass, ui, Section::Profile, rect, Some(&readout));

    let plot_rect = Section::Profile.plot(rect);
    match plots.lift {
        Some(lift) => {
            bars(
                pass,
                ui,
                Section::Profile,
                lift,
                plots.lift_counts,
                plot_rect,
                &accent_rule(pass, mark),
            );
            let (low, high) = (lift.min, lift.max);
            plot::ev_scale(
                pass,
                ui,
                ev_row(plot_rect, plot_rect.bottom() + PLOT_INSET),
                low.min(high).floor() as i32..=low.max(high).ceil() as i32,
                |stop| Some((stop as f32 - low) / (high - low)),
            );
        }
        None => ground(ui, plot_rect),
    }

    let pixel = pass
        .input
        .pointer
        .and_then(|[x, y]| Pixel::at(current, map, x, y));
    curve(pass, ui, profile, section::curve_plot(rect), pixel.as_ref());
    gain_table(pass, ui, profile, section::table_plot(rect), pixel.as_ref());
    exposure(pass, ui, profile, section::exposure_row(rect));
}

/// A plot's ground at `bars`.
fn ground(ui: &egui::Ui, bars: Rect) {
    ui.painter().rect_filled(
        egui::Rect::from(bars.inset(-PLOT_INSET, -PLOT_INSET)),
        PLOT_RADIUS,
        PLOT_BACKGROUND,
    );
}

/// `points` as one line in the accent, over an outline in the plot's
/// ground, as the Display section's curve is drawn.
fn line(pass: &Pass, ui: &egui::Ui, points: Vec<egui::Pos2>) {
    let painter = ui.painter();
    painter.add(egui::Shape::line(
        points.clone(),
        Stroke::new(CURVE_WIDTH + 2.0 * CURVE_OUTLINE, PLOT_BACKGROUND),
    ));
    painter.add(egui::Shape::line(
        points,
        Stroke::new(CURVE_WIDTH, pass.theme.accent),
    ));
}

/// The dot that places the pointer's pixel on a curve, at `at`.
fn dot(ui: &egui::Ui, at: egui::Pos2) {
    ui.painter().circle(
        at,
        PIXEL_DOT,
        Color32::WHITE,
        Stroke::new(CURVE_OUTLINE, PLOT_BACKGROUND),
    );
}

/// What a plot is, in its corner, in the dim ink the plots' own words are.
fn label(pass: &Pass, ui: &egui::Ui, bars: Rect, words: &str, align: egui::Align2) {
    let x = match align.x() {
        egui::Align::Max => bars.right() - CLIP_INSET,
        _ => bars.x + CLIP_INSET,
    };
    let y = match align.y() {
        egui::Align::Max => bars.bottom() - CLIP_INSET,
        _ => bars.y + CLIP_INSET,
    };
    ui.painter().text(
        pos2(x, y),
        align,
        words,
        FontId::proportional(CLIP_TEXT),
        pass.theme.text_dim.into(),
    );
}

/// The device columns of `bars`, each as its input in stops below white,
/// from `-CURVE_STOPS` at the left to 0 at the right: a vertex to the device
/// pixel, so that a steep stretch is drawn as finely as the screen can
/// show it.
fn inputs(pass: &Pass, bars: Rect) -> Vec<f32> {
    let columns = pass.grid.columns(bars.x, bars.width).len().max(1);
    let reach = CURVE_STOPS as f32;
    (0..=columns)
        .map(|column| -reach + reach * column as f32 / columns as f32)
        .collect()
}

/// Where an input of `stops` below white goes across `bars`.
fn across(bars: Rect, stops: f32) -> f32 {
    let reach = CURVE_STOPS as f32;
    bars.x + (stops.clamp(-reach, 0.0) + reach) / reach * bars.width
}

/// The row of stops under a plot whose input runs from `-CURVE_STOPS` to 0.
fn input_scale(pass: &Pass, ui: &egui::Ui, bars: Rect) {
    plot::ev_scale(
        pass,
        ui,
        ev_row(bars, bars.bottom() + PLOT_INSET),
        -CURVE_STOPS..=0,
        |stop| Some((stop + CURVE_STOPS) as f32 / CURVE_STOPS as f32),
    );
}

/// The tone curve on its own ground at `bars`, both axes in stops below
/// white, and the pointer's pixel on it.
///
/// In stops, since all that tells one profile's curve from another's is
/// its toe, a hundredth of the way up a linear plot: in stops the shoulder
/// is the straight line every curve shares and the toe falls away where it
/// does. The diagonal, which a curve doing nothing would be, is drawn
/// faintly under it.
fn curve(pass: &Pass, ui: &egui::Ui, profile: &Profile, bars: Rect, pixel: Option<&Pixel>) {
    ground(ui, bars);
    let reach = CURVE_STOPS as f32;
    let stops = |value: f32| value.max(f32::MIN_POSITIVE).log2().clamp(-reach, 0.0);
    let place = |input: f32, output: f32| {
        pos2(
            across(bars, input),
            bars.bottom() - (output + reach) / reach * bars.height,
        )
    };
    ui.painter().line_segment(
        [place(-reach, -reach), place(0.0, 0.0)],
        Stroke::new(pass.grid.line_width(RULE_WIDTH), pass.theme.border),
    );
    let points = inputs(pass, bars)
        .into_iter()
        .map(|input| place(input, stops(profile.curve_at(input.exp2()))))
        .collect();
    line(pass, ui, points);
    if let Some([input, output]) =
        pixel.and_then(|pixel| profile.on_curve(pixel.developed, pixel.ratios))
    {
        dot(ui, place(stops(input), stops(output)));
    }
    label(pass, ui, bars, TONE_CURVE, egui::Align2::LEFT_TOP);
    input_scale(pass, ui, bars);
}

/// The spread of the gain table at each of `columns` inputs in stops,
/// held between passes for as long as the table is the same one: a reading
/// of every point of the grid at every column, which is nothing once and
/// too much for every frame.
fn spread(ui: &egui::Ui, table: &Arc<GainTable>, inputs: &[f32]) -> Arc<Vec<[f32; 2]>> {
    #[derive(Clone)]
    struct Held {
        table: Arc<GainTable>,
        columns: usize,
        spread: Arc<Vec<[f32; 2]>>,
    }
    let id = egui::Id::new("histogram gain table");
    if let Some(held) = ui.data(|data| data.get_temp::<Held>(id))
        && Arc::ptr_eq(&held.table, table)
        && held.columns == inputs.len()
    {
        return held.spread;
    }
    let spread = Arc::new(
        inputs
            .iter()
            .map(|input| table.spread_at(input.exp2()))
            .collect::<Vec<_>>(),
    );
    ui.data_mut(|data| {
        data.insert_temp(
            id,
            Held {
                table: Arc::clone(table),
                columns: inputs.len(),
                spread: Arc::clone(&spread),
            },
        )
    });
    spread
}

/// The gain table on its own ground at `bars`, with the exposure in it:
/// across, the brightness the table is indexed by, in stops below white;
/// up, how far the exposure and the table together lift a pixel there, in
/// stops.
///
/// With the exposure in it, since neither means anything alone: the
/// exposure lifts everything alike, and the table gives back in the
/// highlights what the exposure took past white, which is what the line
/// coming down to no lift at white shows and either drawn alone would not.
/// The table is a curve at each point of a grid over the picture; their
/// spread is a wash behind, and the pointer's pixel's own curve — the
/// points around it blended for where it is — the line, with the pixel on
/// it. A faint rule across is where the exposure alone would put
/// everything, and one at the foot no lift at all. A profile with no table
/// is the exposure alone, a level line.
fn gain_table(pass: &Pass, ui: &egui::Ui, profile: &Profile, bars: Rect, pixel: Option<&Pixel>) {
    ground(ui, bars);
    let inputs = inputs(pass, bars);
    let baseline = profile.baseline;
    let spread = profile
        .table
        .as_ref()
        .map(|table| spread(ui, table, &inputs));
    let lifts = spread.iter().flat_map(|spread| spread.iter().flatten());
    let (low, high) = lifts
        .map(|gain| baseline + gain.max(f32::MIN_POSITIVE).log2())
        .filter(|stops| stops.is_finite())
        .fold(
            (baseline.min(0.0), baseline.max(0.0)),
            |(low, high), stops| (low.min(stops), high.max(stops)),
        );
    let (low, high) = (low.floor(), high.ceil().max(low.floor() + 1.0));
    let up = |stops: f32| bars.bottom() - (stops - low) / (high - low) * bars.height;
    let painter = ui.painter();
    let rule = Stroke::new(pass.grid.line_width(RULE_WIDTH), pass.theme.border);
    for level in [0.0, baseline] {
        let y = pass.grid.snap(up(level));
        painter.line_segment([pos2(bars.x, y), pos2(bars.right(), y)], rule);
    }
    if let Some(spread) = &spread {
        let wash = Color32::from(pass.theme.accent).gamma_multiply(SPREAD_ALPHA);
        for (input, [least, most]) in inputs.iter().zip(spread.iter()) {
            let x = across(bars, *input);
            let (top, bottom) = (
                up(baseline + most.max(f32::MIN_POSITIVE).log2()),
                up(baseline + least.max(f32::MIN_POSITIVE).log2()),
            );
            painter.line_segment(
                [pos2(x, top), pos2(x, bottom.max(top + 1.0))],
                Stroke::new(1.0, wash),
            );
        }
    }
    let table = profile.table.as_deref();
    match (table, pixel) {
        (Some(table), Some(pixel)) => {
            let points = inputs
                .iter()
                .map(|input| {
                    let gain = table.gain_for(input.exp2(), pixel.place);
                    pos2(
                        across(bars, *input),
                        up(baseline + gain.max(f32::MIN_POSITIVE).log2()),
                    )
                })
                .collect();
            line(pass, ui, points);
            let [input, lift] = pixel.on_table(profile, table);
            dot(
                ui,
                pos2(across(bars, input.max(f32::MIN_POSITIVE).log2()), up(lift)),
            );
        }
        // The exposure alone, the same at every brightness.
        (None, _) => line(
            pass,
            ui,
            vec![pos2(bars.x, up(baseline)), pos2(bars.right(), up(baseline))],
        ),
        (Some(_), None) => {}
    }
    // Low on the left, where the shadows' lift is high above it and the
    // highlights' falls away on the right.
    label(pass, ui, bars, GAIN_TABLE, egui::Align2::LEFT_BOTTOM);
    input_scale(pass, ui, bars);
}

/// The exposure's row, at `row`: its name, and the baseline the profile
/// gives, as the Display section's rows give theirs.
fn exposure(pass: &Pass, ui: &egui::Ui, profile: &Profile, row: Rect) {
    let font = FontId::proportional(ROW_TEXT);
    let middle = row.y + row.height / 2.0;
    let painter = ui.painter();
    painter.text(
        pos2(pass.grid.snap(row.x), middle),
        egui::Align2::LEFT_CENTER,
        EXPOSURE,
        font.clone(),
        pass.theme.text_dim.into(),
    );
    painter.text(
        pos2(pass.grid.snap(row.right()), middle),
        egui::Align2::RIGHT_CENTER,
        exposure_words(profile.baseline, 2),
        font,
        pass.theme.text_primary.into(),
    );
}

/// What the curve's ground says it is.
pub(in crate::ui) const TONE_CURVE: &str = "Tone curve";

/// What the gain table's ground says it is: the table, with the exposure
/// it is read after in it.
pub(in crate::ui) const GAIN_TABLE: &str = "Exposure and gain table";

/// The name of the exposure's row.
const EXPOSURE: &str = "Exposure";

/// What the header says with nothing under the pointer.
const APPLIED: &str = "Applied";
const NOT_APPLIED: &str = "Not applied";

#[cfg(test)]
mod tests {
    use super::*;

    /// The header says whether the rendering is on screen; the exposure is
    /// written signed, with a true minus.
    #[test]
    fn the_words_say_the_switch_and_the_exposure() {
        assert_eq!(summary(true), "Applied");
        assert_eq!(summary(false), "Not applied");
        assert_eq!(exposure_words(4.123, 2), "+4.12 EV");
        assert_eq!(exposure_words(-1.27, 2), "\u{2212}1.27 EV");
        assert_eq!(exposure_words(-0.0, 2), "+0.00 EV");
    }
}
