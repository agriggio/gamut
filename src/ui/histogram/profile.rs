//! The Profile section, for a raw whose profile says how it should look:
//! how far the profile's rendering lifts each pixel, as a histogram in
//! stops, as the Gain map section plots a photograph's lift; and under it
//! the rendering's three parts, read upward in the order they are applied,
//! as the column is: the tone curve, the gain table with the exposure in
//! it, and the exposure's own row. The last row follows the pointer's pixel
//! up through the three. The square at the end of the header is the
//! profile's switch, and the header says whether it is on.
//!
//! Every reading here is of one quantity wherever it can be: the pixel's
//! brightest channel in linear ProPhoto, in stops below white, which is the
//! channel the curve is put through. The gain table alone reads something
//! else — a blend of the channels with its own weights — and its caption
//! says so and gives that reading as well as the lift.

use std::sync::Arc;

use egui::Stroke;

use super::plot::{PLOT_BACKGROUND, bars};
use super::*;
use crate::image::gain_map::{GainTable, Profile};

/// How far below white the curve's and the table's inputs reach, in stops:
/// past the toe of every profile sampled, which falls away below four.
const CURVE_STOPS: i32 = 10;

/// How big the dot is that places the pointer's pixel on a curve.
const PIXEL_DOT: f32 = 2.5;

/// How strongly the spread of the gain table's points is laid on its
/// ground: a wash of the accent, behind the line it is the range of.
const SPREAD_ALPHA: f32 = 0.35;

/// The least room between two numbers of a part's scale, in points, below
/// which the later of them is left out: a line of the plot's small words.
const SCALE_SPACING: f32 = CLIP_TEXT + 1.0;

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

/// A number of stops as the section writes it: signed, with a true minus,
/// to `places`; `black` for none at all, which no number of stops is.
fn stops_words(stops: f32, places: usize) -> String {
    if !stops.is_finite() {
        return BLACK.to_string();
    }
    // Adding zero makes a negative zero positive.
    format!("{:+.*}", places, stops + 0.0).replace('-', "\u{2212}")
}

/// An exposure, as the exposure's row writes it.
fn exposure_words(stops: f32) -> String {
    format!("{} EV", stops_words(stops, 2))
}

/// Linear light as stops below white.
fn stops(linear: f32) -> f32 {
    if linear > 0.0 {
        linear.log2()
    } else {
        f32::NEG_INFINITY
    }
}

/// The pointer's pixel as the profile's parts read it, at the whole of the
/// profile whether or not the switch has it on screen, since the parts are
/// the profile's.
struct Pixel {
    /// Its brightest channel in linear ProPhoto, as developed.
    developed: f32,
    /// What the exposure makes of that.
    exposed: f32,
    /// The brightness the gain table reads, and the lift the exposure and
    /// the table give, in stops; where the profile has a table.
    table: Option<[f32; 2]>,
    /// Where the pixel is, down and across, as a share of the picture: the
    /// place its own table is blended for.
    place: [f32; 2],
    /// The brightest channel after the exposure and the table — what the
    /// curve reads, white at most — and what the curve made of it.
    curve: [f32; 2],
}

impl Pixel {
    /// The pixel at `(x, y)` of the turned picture, where it is on it.
    ///
    /// The table's gain is one number for every channel, so the brightest
    /// channel after it is the brightest before it, lifted by it: which is
    /// why the stage the table plot draws in its own terms joins the
    /// stages before and after it.
    fn at(current: &Current, profile: &Profile, x: u32, y: u32) -> Option<Self> {
        let face = current.picture_face();
        let size = [face.image.width, face.image.height];
        let [width, height] = current.turn.size(size);
        if x >= width || y >= height {
            return None;
        }
        let [x, y] = current.turn.stored([x, y], size);
        let sample = face.image.sample_as_stored(x, y)?;
        let linear = sample.linear();
        let color: [f32; 3] = [0, 1, 2].map(|c| linear.get(c).copied().unwrap_or(linear[0]));
        let prophoto: [f32; 3] = std::array::from_fn(|row| {
            let matrix = profile.to_curve[row];
            matrix[0] * color[0] + matrix[1] * color[1] + matrix[2] * color[2]
        });
        let developed = prophoto.into_iter().fold(0.0f32, f32::max);
        let exposure = profile.baseline.exp2();
        let place = [
            (y as f32 + 0.5) / size[1] as f32,
            (x as f32 + 0.5) / size[0] as f32,
        ];
        let table = profile.table.as_deref().map(|table| {
            let input = table.input(prophoto.map(|value| value * exposure));
            [
                input,
                profile.baseline + table.gain_for(input, place).log2(),
            ]
        });
        let lift = table.map_or(profile.baseline, |[_, lift]| lift);
        let into_curve = (developed * lift.exp2()).clamp(0.0, 1.0);
        Some(Self {
            developed,
            exposed: developed * exposure,
            table,
            place,
            curve: [into_curve, profile.curve_at(into_curve)],
        })
    }

    /// The pixel's brightest channel at each stage, in stops: as developed,
    /// exposed, through the table, through the curve.
    fn trace(&self) -> String {
        let after_table = match self.table {
            Some([_, lift]) => stops(self.developed) + lift,
            None => stops(self.exposed),
        };
        let stages = [
            stops(self.developed),
            stops(self.exposed),
            after_table,
            stops(self.curve[1]),
        ];
        stages
            .map(|stage| stops_words(stage, 1))
            .join(&format!(" {BECOMES} "))
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

    lift(pass, ui, Section::Profile.plot(rect), plots, mark);

    let pixel = pass
        .input
        .pointer
        .and_then(|[x, y]| Pixel::at(current, profile, x, y));
    curve(pass, ui, profile, section::curve_part(rect), pixel.as_ref());
    gain_table(pass, ui, profile, section::table_part(rect), pixel.as_ref());
    let font = FontId::proportional(ROW_TEXT);
    row(
        pass,
        ui,
        section::exposure_row(rect),
        EXPOSURE,
        &exposure_words(profile.baseline),
        &font,
    );
    let trace = pixel.as_ref().map(Pixel::trace).unwrap_or_default();
    let trace_row = section::trace_row(rect);
    row(pass, ui, trace_row, PIXEL, &trace, &font);
    // What the row's numbers are, under the pointer: the row has no room
    // to say it.
    let response = ui.interact(
        egui::Rect::from(trace_row),
        ui.id().with("profile trace"),
        Sense::hover(),
    );
    pass.tooltip(response, Tip::ProfilePixel);
}

/// The histogram of the lift at `plot_rect`, with its row of stops.
fn lift(pass: &Pass, ui: &egui::Ui, plot_rect: Rect, plots: &Plots, mark: Option<&Mark>) {
    let Some(lift) = plots.lift else {
        ground(ui, plot_rect);
        return;
    };
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

/// A part's caption at `caption`: its name over what its axes are, in the
/// plot's small words, and on the right of the name what the pointer's
/// pixel reads on it.
pub(super) fn caption(
    pass: &Pass,
    ui: &egui::Ui,
    caption: Rect,
    name: &str,
    axes: &str,
    reading: &str,
) {
    let painter = ui.painter();
    let font = FontId::proportional(CLIP_TEXT);
    let line = caption.height / 2.0;
    let first = caption.y + line / 2.0;
    painter.text(
        pos2(pass.grid.snap(caption.x), first),
        egui::Align2::LEFT_CENTER,
        name,
        font.clone(),
        pass.theme.text_primary.into(),
    );
    painter.text(
        pos2(pass.grid.snap(caption.right()), first),
        egui::Align2::RIGHT_CENTER,
        reading,
        font.clone(),
        pass.theme.text_primary.into(),
    );
    painter.text(
        pos2(pass.grid.snap(caption.x), first + line),
        egui::Align2::LEFT_CENTER,
        axes,
        font,
        pass.theme.text_dim.into(),
    );
}

/// The numbers up the left of a part's ground at `bars`: each value at the
/// height `up` puts it, in the gutter, in the order given, a later one left
/// out where it would crowd one already written.
fn scale(pass: &Pass, ui: &egui::Ui, bars: Rect, ticks: &[f32], up: impl Fn(f32) -> f32) {
    let painter = ui.painter();
    let font = FontId::proportional(CLIP_TEXT);
    let mut written: Vec<f32> = Vec::new();
    for &tick in ticks {
        let y = up(tick);
        if written
            .iter()
            .any(|other| (other - y).abs() < SCALE_SPACING)
        {
            continue;
        }
        written.push(y);
        painter.text(
            pos2(bars.x - PLOT_INSET - CLIP_INSET, y),
            egui::Align2::RIGHT_CENTER,
            // No lift is no sign.
            match tick {
                0.0 => "0".to_string(),
                _ => stops_words(tick, if tick.fract() == 0.0 { 0 } else { 1 }),
            },
            font.clone(),
            pass.theme.text_dim.into(),
        );
    }
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

/// The tone curve at `part`, both axes in stops below white, and the
/// pointer's pixel on it.
///
/// In stops, since all that tells one profile's curve from another's is
/// its toe, a hundredth of the way up a linear plot: in stops the shoulder
/// is the straight line every curve shares and the toe falls away where it
/// does. The diagonal, which a curve doing nothing would be, is drawn
/// faintly under it.
fn curve(
    pass: &Pass,
    ui: &egui::Ui,
    profile: &Profile,
    (head, bars): (Rect, Rect),
    pixel: Option<&Pixel>,
) {
    ground(ui, bars);
    let reach = CURVE_STOPS as f32;
    let clamped = |value: f32| stops(value).clamp(-reach, 0.0);
    let up = |output: f32| bars.bottom() - (output + reach) / reach * bars.height;
    ui.painter().line_segment(
        [pos2(bars.x, up(-reach)), pos2(bars.right(), up(0.0))],
        Stroke::new(pass.grid.line_width(RULE_WIDTH), pass.theme.border),
    );
    let points = inputs(pass, bars)
        .into_iter()
        .map(|input| {
            pos2(
                across(bars, input),
                up(clamped(profile.curve_at(input.exp2()))),
            )
        })
        .collect();
    line(pass, ui, points);
    let reading = match pixel {
        Some(pixel) => {
            let [input, output] = pixel.curve;
            if output > 0.0 {
                dot(ui, pos2(across(bars, stops(input)), up(clamped(output))));
            }
            format!(
                "{IN} {}, {OUT} {}",
                stops_words(stops(input), 1),
                stops_words(stops(output), 1)
            )
        }
        None => String::new(),
    };
    caption(pass, ui, head, TONE_CURVE, TONE_CURVE_AXES, &reading);
    scale(pass, ui, bars, &[0.0, -reach, -reach / 2.0], up);
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

/// The gain table at `part`, with the exposure in it: across, the
/// brightness the table reads, in stops below white; up, how far the
/// exposure and the table together lift a pixel there, in stops, with the
/// stops up the left.
///
/// With the exposure in it, since neither means anything alone: the
/// exposure lifts everything alike, and the table gives back in the
/// highlights what the exposure took past white, which is what the line
/// coming down to no lift at white shows and either drawn alone would not.
/// The table is a curve at each point of a grid over the picture; their
/// spread is a wash behind, and the pointer's pixel's own curve — the
/// points around it blended for where it is — the line, with the pixel on
/// it. Faint rules across are no lift and the exposure alone, both named on
/// the scale. A profile with no table is the exposure alone, a level line.
///
/// The brightness the table reads is not the brightest channel the rest of
/// the section reads: it is the table's own blend of the channels, said in
/// the caption, so the pixel is placed across by that, and its lift up is
/// what joins the stage before to the stage after.
fn gain_table(
    pass: &Pass,
    ui: &egui::Ui,
    profile: &Profile,
    (head, bars): (Rect, Rect),
    pixel: Option<&Pixel>,
) {
    ground(ui, bars);
    let inputs = inputs(pass, bars);
    let baseline = profile.baseline;
    let spread = profile
        .table
        .as_ref()
        .map(|table| spread(ui, table, &inputs));
    let lift = |gain: f32| baseline + gain.max(f32::MIN_POSITIVE).log2();
    let (low, high) = spread
        .iter()
        .flat_map(|spread| spread.iter().flatten())
        .map(|gain| lift(*gain))
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
            let (top, bottom) = (up(lift(*most)), up(lift(*least)));
            painter.line_segment(
                [pos2(x, top), pos2(x, bottom.max(top + 1.0))],
                Stroke::new(1.0, wash),
            );
        }
    }
    let table = profile.table.as_deref();
    let reading = match (table, pixel) {
        (Some(table), Some(pixel)) => {
            let points = inputs
                .iter()
                .map(|input| {
                    pos2(
                        across(bars, *input),
                        up(lift(table.gain_for(input.exp2(), pixel.place))),
                    )
                })
                .collect();
            line(pass, ui, points);
            match pixel.table {
                Some([input, raised]) => {
                    dot(ui, pos2(across(bars, stops(input)), up(raised)));
                    format!(
                        "{READS} {}, {LIFT} {}",
                        stops_words(stops(input), 1),
                        stops_words(raised, 1)
                    )
                }
                None => String::new(),
            }
        }
        // The exposure alone, the same at every brightness.
        (None, _) => {
            line(
                pass,
                ui,
                vec![pos2(bars.x, up(baseline)), pos2(bars.right(), up(baseline))],
            );
            pixel.map_or(String::new(), |_| {
                format!("{LIFT} {}", stops_words(baseline, 1))
            })
        }
        (Some(_), None) => String::new(),
    };
    caption(pass, ui, head, GAIN_TABLE, GAIN_TABLE_AXES, &reading);
    // No lift and the exposure alone first, since they are the rules, then
    // the plot's top and foot where there is room.
    scale(pass, ui, bars, &[0.0, round_tenth(baseline), high, low], up);
    input_scale(pass, ui, bars);
}

/// `stops` to a tenth, as the scale writes it.
fn round_tenth(stops: f32) -> f32 {
    (stops * 10.0).round() / 10.0
}

/// One of the section's rows at `row`: its name down the left, and `value`
/// at the right, as the Display section's rows are laid out.
fn row(pass: &Pass, ui: &egui::Ui, row: Rect, name: &str, value: &str, font: &FontId) {
    let middle = row.y + row.height / 2.0;
    let painter = ui.painter();
    painter.text(
        pos2(pass.grid.snap(row.x), middle),
        egui::Align2::LEFT_CENTER,
        name,
        font.clone(),
        pass.theme.text_dim.into(),
    );
    painter.text(
        pos2(pass.grid.snap(row.right()), middle),
        egui::Align2::RIGHT_CENTER,
        value,
        font.clone(),
        pass.theme.text_primary.into(),
    );
}

/// The tone curve's caption: its name, and what its axes are.
pub(in crate::ui) const TONE_CURVE: &str = "Tone curve";
const TONE_CURVE_AXES: &str = "Brightest channel, out by in";

/// The gain table's caption: the table, with the exposure it is read after
/// in it, and what its axes are.
pub(in crate::ui) const GAIN_TABLE: &str = "Exposure and gain table";
const GAIN_TABLE_AXES: &str = "Lift by the brightness the table reads";

/// The words of the pointer's readings on the two parts.
const IN: &str = "in";
const OUT: &str = "out";
const READS: &str = "reads";
const LIFT: &str = "lift";

/// The names of the section's two rows.
const EXPOSURE: &str = "Exposure";
pub(in crate::ui) const PIXEL: &str = "Pixel";

/// A stage at which the pixel is black, which no number of stops is.
const BLACK: &str = "black";

/// What the header says with nothing under the pointer.
const APPLIED: &str = "Applied";
const NOT_APPLIED: &str = "Not applied";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image::gain_map::{Data, GainMap, Lift};

    /// The header says whether the rendering is on screen; stops are
    /// written signed, with a true minus, and none at all as black.
    #[test]
    fn the_words_say_the_switch_and_the_stops() {
        assert_eq!(summary(true), "Applied");
        assert_eq!(summary(false), "Not applied");
        assert_eq!(exposure_words(4.123), "+4.12 EV");
        assert_eq!(exposure_words(-1.27), "\u{2212}1.27 EV");
        assert_eq!(exposure_words(-0.0), "+0.00 EV");
        assert_eq!(stops_words(f32::NEG_INFINITY, 1), "black");
    }

    /// A pixel is followed through the profile in one quantity, its
    /// brightest channel: exposed, lifted by the table's gain — one number
    /// for every channel, so the brightest stays the brightest — and put
    /// through the curve, which reads it at white at most.
    #[test]
    fn a_pixel_is_traced_through_the_profile_by_its_brightest_channel() {
        let table = GainTable {
            points: [2, 2],
            spacing: [1.0, 1.0],
            origin: [0.0, 0.0],
            steps: 2,
            weights: [0.3, 0.6, 0.1, 0.0, 0.0],
            gains: vec![2.0; 2 * 2 * 2],
        };
        let profile = Profile {
            baseline: -2.0,
            table: Some(Arc::new(table)),
            curve: Some(Arc::new(vec![[0.0, 0.0], [0.5, 0.25], [1.0, 1.0]])),
            range: [0.0; 2],
            to_curve: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
        };
        let mut image = crate::image::DecodedImage::new(
            1,
            1,
            crate::image::Samples::F32 {
                channels: crate::image::Channels::Rgb,
                data: vec![0.5, 1.0, 0.25],
            },
            crate::image::ColorSpace::LINEAR_BT709,
            crate::image::AlphaMode::Opaque,
        );
        image.gain_map = Some(Arc::new(GainMap {
            width: 1,
            height: 1,
            channels: 3,
            data: Data::Ratios(vec![half::f16::ONE; 3]),
            lift: Lift::Profile(profile.clone()),
        }));
        let current = Current::of(image, "raw");
        let pixel = Pixel::at(&current, &profile, 0, 0).expect("on the picture");
        assert_eq!(pixel.developed, 1.0, "the brightest channel");
        assert_eq!(pixel.exposed, 0.25);
        let [reads, lift] = pixel.table.expect("a table");
        assert!((reads - (0.3 * 0.5 + 0.6 + 0.1 * 0.25) / 4.0).abs() < 1e-6);
        assert!((lift + 1.0).abs() < 1e-6, "-2 EV and a gain of 2");
        assert_eq!(pixel.curve, [0.5, 0.25]);
        assert_eq!(
            pixel.trace(),
            "+0.0 \u{00bb} \u{2212}2.0 \u{00bb} \u{2212}1.0 \u{00bb} \u{2212}2.0"
        );
    }
}
