//! The Profile section, for a raw whose profile says how it should look:
//! how far the profile's rendering lifts each pixel, as a histogram in
//! stops, as the Gain map section plots a photograph's lift; and under it
//! the rendering's stages, read upward in the order they are applied, as
//! the column is: the tone curve, the gain table, the exposure's row, and
//! at the foot the developed pixel in the profile's own space, which is
//! where the chain starts. The square at the end of the header is the
//! profile's switch, and the header says whether it is on.
//!
//! The pointer's pixel is followed up the chain a channel at a time, each
//! in its own ink and every number in stops below white: the three channels
//! in linear ProPhoto on the ProPhoto row, the three after the exposure on
//! its row, the three after the table's one gain in the table's caption,
//! and the three the curve makes of them in the curve's. Each stage's
//! numbers are the stage above's inputs, so the chain reads straight up.
//! The two places it is not three arrows are drawn as what they are: the
//! table is read once a pixel, at a blend of the channels, so its plot has
//! the blend's dot on the curve and the channels as ticks on the axis; and
//! the curve is applied to the brightest and dimmest channels with the
//! middle one placed between them, so its plot has two dots on the curve
//! and a hollow one between. What the lift at the top makes of the three,
//! back in Rec. 2020, is the row under the lift's plot: the three numbers
//! the plot counts, read straight off it.

use std::sync::Arc;

use egui::Stroke;

use super::plot::{PLOT_BACKGROUND, bars};
use super::*;
use crate::image::gain_map::{GainTable, Profile, tone_order};

/// How far below white the curve's and the table's inputs reach, in stops:
/// past the toe of every profile sampled, which falls away below four.
const CURVE_STOPS: i32 = 10;

/// How big the dot is that places a channel on a curve.
const PIXEL_DOT: f32 = 2.5;

/// How long a channel's tick on a plot's axis is.
const TICK: f32 = 4.0;

/// How far a guide's words stand off the guide and off the plot's edge.
const GUIDE_GAP: f32 = 3.0;

/// How strongly the spread of the gain table's points is laid on its
/// ground: a wash of the accent, behind the line it is the range of.
const SPREAD_ALPHA: f32 = 0.35;

/// The least room between two numbers of a part's scale, in points, below
/// which the later of them is left out: a line of the plot's small words.
const SCALE_SPACING: f32 = CLIP_TEXT + 1.0;

/// The room between one channel's number and the next in a caption or a
/// row.
const CELL_GAP: f32 = 8.0;

/// Whether the profile's rendering is on screen.
fn applied(current: &Current) -> bool {
    current
        .picture_face()
        .lift
        .as_ref()
        .is_some_and(|lift| lift.weight() > 0.0)
}

/// What the header says: whether the
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

/// An exposure, as the exposure's row writes it: to the tenth the other
/// shares of the lift are written to, so the three can be summed by eye.
fn exposure_words(stops: f32) -> String {
    format!("{} EV", stops_words(stops, 1))
}

/// Linear light as stops below white.
fn stops(linear: f32) -> f32 {
    if linear > 0.0 {
        linear.log2()
    } else {
        f32::NEG_INFINITY
    }
}

/// Three channels in stops below white, to a tenth.
fn channel_words(color: [f32; 3]) -> [String; 3] {
    color.map(|value| stops_words(stops(value), 1))
}

/// The pointer's pixel as the profile's stages have it, a channel at a
/// time, at the whole of the profile whether or not the switch has it on
/// screen, since the stages are the profile's. Every triple is a fraction
/// of white in linear ProPhoto.
struct Pixel {
    /// As developed, carried into ProPhoto: what the chain starts from.
    /// Held at 0 where the matrix put a channel below it.
    prophoto: [f32; 3],
    /// After the exposure.
    exposed: [f32; 3],
    /// Where the profile has a table: the blend of the exposed channels
    /// the table is read at, as a fraction of white in exposed terms, and
    /// the gain it gave.
    table: Option<(f32, f32)>,
    /// After the gain, held to white at most: what the curve reads.
    into_curve: [f32; 3],
    /// What the curve made of them.
    out: [f32; 3],
    /// Where the pixel is, down and across, as a share of the picture: the
    /// place its own table is blended for.
    place: [f32; 2],
}

impl Pixel {
    /// The pixel at `(x, y)` of the turned picture, where it is on it.
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
            (matrix[0] * color[0] + matrix[1] * color[1] + matrix[2] * color[2]).max(0.0)
        });
        let exposure = profile.baseline.exp2();
        let exposed = prophoto.map(|value| value * exposure);
        // The table's grid is laid over the picture as the file holds it,
        // before the developer turned it the way the camera was held, so
        // the pixel's place is found in that frame: the sensor says how the
        // picture was turned.
        let turned = face
            .image
            .sensor
            .as_ref()
            .map_or(crate::image::orient::Orientation::NoTransforms, |sensor| {
                sensor.turned
            });
        let (file_width, file_height) = crate::image::orient::size(size[0], size[1], turned);
        let (fx, fy) = crate::image::orient::stored(
            turned,
            x as usize,
            y as usize,
            file_width as usize,
            file_height as usize,
        );
        let place = [
            (fy as f32 + 0.5) / file_height as f32,
            (fx as f32 + 0.5) / file_width as f32,
        ];
        let table = profile.table.as_deref().map(|table| {
            let input = table.input(exposed);
            (input / table.scale(), table.gain_for(input, place))
        });
        let gain = table.map_or(1.0, |(_, gain)| gain);
        let into_curve = exposed.map(|value| (value * gain).clamp(0.0, 1.0));
        Some(Self {
            prophoto,
            exposed,
            table,
            into_curve,
            out: profile.tone(into_curve),
            place,
        })
    }
}

/// The Profile section, laid out at `rect`: the switch, the histogram of
/// the lift, and the rendering's stages.
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
    // The header says whether the rendering is on screen; the pixel's
    // numbers are the row under the plot, where the plot counts them.
    section::header(pass, ui, Section::Profile, rect, Some(summary(applied)));

    lift(pass, ui, Section::Profile.plot(rect), plots, mark);

    let font = FontId::proportional(ROW_TEXT);
    // The row under the plot: the pixel's lift a channel to the cell in
    // its own ink, as every row under it writes the channels, where the
    // pointer is on the picture — the gain map's words, which the mark
    // is, with the true minus the rows write their stops with.
    let total_row = section::total_row(rect);
    let cells = mark.filter(|mark| !mark.channels.is_empty()).map(|mark| {
        std::array::from_fn(|c| {
            mark.channels.get(c).map_or_else(String::new, |channel| {
                channel.words.replace('-', "\u{2212}")
            })
        })
    });
    row(pass, ui, total_row, TOTAL_LIFT, "", cells, &font);
    tip_on(pass, ui, total_row, "profile lift", Tip::ProfileLift);

    let pixel = pass
        .input
        .pointer
        .and_then(|[x, y]| Pixel::at(current, profile, x, y));
    curve(pass, ui, profile, section::curve_part(rect), pixel.as_ref());
    gain_table(pass, ui, profile, section::table_part(rect), pixel.as_ref());
    // The exposure's row: the shift, and each channel after it.
    let exposure_row = section::exposure_row(rect);
    row(
        pass,
        ui,
        exposure_row,
        EXPOSURE,
        &exposure_words(profile.baseline),
        pixel.as_ref().map(|pixel| channel_words(pixel.exposed)),
        &font,
    );
    tip_on(pass, ui, exposure_row, "profile exposure", Tip::Baseline);
    // The foot: the developed pixel in the profile's space, where the
    // chain starts.
    let prophoto_row = section::prophoto_row(rect);
    row(
        pass,
        ui,
        prophoto_row,
        PROPHOTO,
        "",
        pixel.as_ref().map(|pixel| channel_words(pixel.prophoto)),
        &font,
    );
    tip_on(pass, ui, prophoto_row, "profile prophoto", Tip::ProPhoto);
}

/// `tip` under the pointer over `rect`, which is painted rather than a
/// widget: a row, a header's line, or a part's caption and plot together.
fn tip_on(pass: &Pass, ui: &egui::Ui, rect: Rect, id: &str, tip: Tip) {
    let response = ui.interact(egui::Rect::from(rect), ui.id().with(id), Sense::hover());
    pass.tooltip(response, tip);
}

/// A part's caption and its ground as one rectangle, for the tooltip over
/// both.
fn part_rect((head, bars): (Rect, Rect)) -> Rect {
    let x = head.x.min(bars.x);
    let y = head.y.min(bars.y);
    Rect::new(
        x,
        y,
        head.right().max(bars.right()) - x,
        head.bottom().max(bars.bottom()) - y,
    )
}

/// The histogram of the lift at `plot_rect`, with its row of stops: each
/// channel's lift at the pointer's pixel a rule in its own ink, where the
/// map has a channel to the color.
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
        &plane_rules(pass, mark),
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

/// A dot that places a channel on a curve, at `at` in `ink`: filled for a
/// channel the curve was applied to, hollow for one placed between the
/// others.
fn dot(ui: &egui::Ui, at: egui::Pos2, ink: Color32, filled: bool) {
    let fill = if filled { ink } else { PLOT_BACKGROUND.into() };
    ui.painter()
        .circle(at, PIXEL_DOT, fill, Stroke::new(CURVE_OUTLINE, ink));
}

/// The dot that places the pointer's pixel itself, rather than a channel of
/// it, on a curve: the gain table's blend.
fn pixel_dot(ui: &egui::Ui, at: egui::Pos2) {
    ui.painter().circle(
        at,
        PIXEL_DOT,
        Color32::WHITE,
        Stroke::new(CURVE_OUTLINE, PLOT_BACKGROUND),
    );
}

/// A channel's tick on the axis under `bars` at `x`, and one on the scale
/// beside it at `y`, each in `ink`: where the channel stands going in and
/// coming out, read against the row of stops and the scale.
fn tick_across(ui: &egui::Ui, bars: Rect, x: f32, ink: Color32) {
    ui.painter().line_segment(
        [pos2(x, bars.bottom()), pos2(x, bars.bottom() - TICK)],
        Stroke::new(CURVE_WIDTH, ink),
    );
}

fn tick_up(ui: &egui::Ui, bars: Rect, y: f32, ink: Color32) {
    ui.painter().line_segment(
        [pos2(bars.x, y), pos2(bars.x + TICK, y)],
        Stroke::new(CURVE_WIDTH, ink),
    );
}

/// A part's caption at `caption`, laid out as the section's rows are: its
/// name, in the face and the dim ink the rows' names are in, with on the
/// right of it what the pointer's pixel comes out of the part as, a
/// channel to the cell in its own ink, the line taking
/// `section::PART_CAPTION_HEIGHT`; and under both what its axes are, in
/// the plot's small words, where `axes` says anything — a second line the
/// caption is then laid out with room for.
pub(super) fn caption(
    pass: &Pass,
    ui: &egui::Ui,
    caption: Rect,
    name: &str,
    axes: &str,
    cells: Option<[String; 3]>,
) {
    let painter = ui.painter();
    let font = FontId::proportional(ROW_TEXT);
    let first = caption.y + section::PART_CAPTION_HEIGHT / 2.0;
    painter.text(
        pos2(pass.grid.snap(caption.x), first),
        egui::Align2::LEFT_CENTER,
        name,
        font.clone(),
        pass.theme.text_dim.into(),
    );
    if let Some(cells) = cells {
        channel_cells(pass, ui, caption.right(), first, &cells, &font);
    }
    if !axes.is_empty() {
        let second = (caption.y + section::PART_CAPTION_HEIGHT + caption.bottom()) / 2.0;
        painter.text(
            pos2(pass.grid.snap(caption.x), second),
            egui::Align2::LEFT_CENTER,
            axes,
            FontId::proportional(CLIP_TEXT),
            pass.theme.text_dim.into(),
        );
    }
}

/// Three channels' numbers set right from `end`, centered on `middle`,
/// each in its plane's ink, blue last.
fn channel_cells(
    pass: &Pass,
    ui: &egui::Ui,
    end: f32,
    middle: f32,
    cells: &[String; 3],
    font: &FontId,
) {
    let painter = ui.painter();
    let mut end = end;
    for (words, plane) in cells.iter().zip(Plane::COLOR).rev() {
        let galley = painter.layout_no_wrap(words.clone(), font.clone(), Color32::PLACEHOLDER);
        let width = galley.size().x;
        let at = pos2(pass.grid.snap(end - width), middle - galley.size().y / 2.0);
        painter.galley(at, galley, plane.ink());
        end -= width + CELL_GAP;
    }
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
/// pointer's pixel on it a channel at a time.
///
/// In stops, since all that tells one profile's curve from another's is
/// its toe, a hundredth of the way up a linear plot: in stops the shoulder
/// is the straight line every curve shares and the toe falls away where it
/// does. The diagonal, which a curve doing nothing would be, is drawn
/// faintly under it. The brightest and dimmest channels are on the curve,
/// since it is applied to them; the middle one is placed between them and
/// drawn hollow where it stands, off the curve, which is what keeps the hue.
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
    if let Some(pixel) = pixel {
        let [high, middle, low] = tone_order(pixel.into_curve);
        for (c, plane) in Plane::COLOR.into_iter().enumerate() {
            let (input, output) = (pixel.into_curve[c], pixel.out[c]);
            if input > 0.0 {
                tick_across(ui, bars, across(bars, stops(input)), plane.ink());
            }
            if output > 0.0 {
                tick_up(ui, bars, up(clamped(output)), plane.ink());
            }
            if output > 0.0 {
                // The middle channel is between the other two by
                // construction, and on the curve only where there are no
                // other two to be between.
                let between = c == middle && pixel.into_curve[high] > pixel.into_curve[low];
                dot(
                    ui,
                    pos2(across(bars, stops(input)), up(clamped(output))),
                    plane.ink(),
                    !between,
                );
            }
        }
    }
    caption(
        pass,
        ui,
        head,
        TONE_CURVE,
        "",
        pixel.map(|pixel| channel_words(pixel.out)),
    );
    tip_on(
        pass,
        ui,
        part_rect((head, bars)),
        "profile curve",
        Tip::ToneCurve,
    );
    scale(pass, ui, bars, &[0.0, -reach, -reach / 2.0], up);
    input_scale(pass, ui, bars);
}

/// The spread of the gain table at each of `inputs`, in the table's own
/// domain, held between passes for as long as the table is the same one: a
/// reading of every point of the grid at every column, which is nothing
/// once and too much for every frame.
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
            .map(|input| table.spread_at(*input))
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

/// The gain table at `part`: across, the brightness the table reads, as
/// the exposed pixel's, in stops below white; up, the gain alone, in stops,
/// with the stops up the left.
///
/// The gain alone, since the exposure has its own row under it and the
/// chain reads up through both. The table is read once a pixel, at a
/// blend of its channels: the dot is that blend on the pixel's own curve,
/// and the three channels going in are ticks on the axis, all of them
/// multiplied by the one gain the dot reads. The table is a curve at each
/// point of a grid over the picture; their spread is a wash behind, and the
/// pointer's pixel's own curve — the points around it blended for where it
/// is — the line. Faint rules across are no gain and the gain that undoes
/// the exposure, both named on the scale: on an iPhone the table comes
/// down to the second at white, which is what takes the highlights back to
/// where the file stored them. A profile with no table is no gain, a level
/// line.
///
/// The table's own input is the blend scaled by the sum of its weights, so
/// a column at `e` stops in exposed terms reads the table at `2^e` times
/// that sum — see `GainTable::scale`.
fn gain_table(
    pass: &Pass,
    ui: &egui::Ui,
    profile: &Profile,
    (head, bars): (Rect, Rect),
    pixel: Option<&Pixel>,
) {
    ground(ui, bars);
    let exposed = inputs(pass, bars);
    let table = profile.table.as_deref();
    let factor = table.map_or(1.0, GainTable::scale);
    let own: Vec<f32> = exposed.iter().map(|stops| stops.exp2() * factor).collect();
    let spread = profile.table.as_ref().map(|table| spread(ui, table, &own));
    let gain_stops = |gain: f32| gain.max(f32::MIN_POSITIVE).log2();
    let undoes = -profile.baseline;
    let (low, high) = spread
        .iter()
        .flat_map(|spread| spread.iter().flatten())
        .map(|gain| gain_stops(*gain))
        .filter(|stops| stops.is_finite())
        .fold((undoes.min(0.0), undoes.max(0.0)), |(low, high), stops| {
            (low.min(stops), high.max(stops))
        });
    let (low, high) = (low.floor(), high.ceil().max(low.floor() + 1.0));
    let up = |stops: f32| bars.bottom() - (stops - low) / (high - low) * bars.height;
    let painter = ui.painter();
    let rule = Stroke::new(pass.grid.line_width(RULE_WIDTH), pass.theme.border);
    for level in [0.0, undoes] {
        let y = pass.grid.snap(up(level));
        painter.line_segment([pos2(bars.x, y), pos2(bars.right(), y)], rule);
    }
    if let Some(spread) = &spread {
        let wash = Color32::from(pass.theme.accent).gamma_multiply(SPREAD_ALPHA);
        for (stops, [least, most]) in exposed.iter().zip(spread.iter()) {
            let x = across(bars, *stops);
            let (top, bottom) = (up(gain_stops(*most)), up(gain_stops(*least)));
            painter.line_segment(
                [pos2(x, top), pos2(x, bottom.max(top + 1.0))],
                Stroke::new(1.0, wash),
            );
        }
    }
    match (table, pixel) {
        (Some(table), Some(pixel)) => {
            let points = exposed
                .iter()
                .zip(&own)
                .map(|(stops, input)| {
                    pos2(
                        across(bars, *stops),
                        up(gain_stops(table.gain_for(*input, pixel.place))),
                    )
                })
                .collect();
            line(pass, ui, points);
            // The channels going in, and the blend the table is read at
            // with its guide across from the scale: the one gain all three
            // get.
            for (c, plane) in Plane::COLOR.into_iter().enumerate() {
                if pixel.exposed[c] > 0.0 {
                    tick_across(ui, bars, across(bars, stops(pixel.exposed[c])), plane.ink());
                }
            }
            if let Some((blend, gain)) = pixel.table {
                let at = pos2(across(bars, stops(blend)), up(gain_stops(gain)));
                let guide = Stroke::new(
                    pass.grid.line_width(RULE_WIDTH),
                    Color32::from(pass.theme.accent.with_alpha(CURSOR_ALPHA)),
                );
                painter.line_segment([pos2(bars.x, at.y), at], guide);
                painter.line_segment([pos2(at.x, bars.bottom()), at], guide);
                let font = FontId::proportional(CLIP_TEXT);
                let ink = Color32::from(pass.theme.text_primary);
                let (y, align) = if at.y < bars.y + bars.height / 2.0 {
                    (at.y + GUIDE_GAP, egui::Align2::LEFT_TOP)
                } else {
                    (at.y - GUIDE_GAP, egui::Align2::LEFT_BOTTOM)
                };
                painter.text(
                    pos2(bars.x + GUIDE_GAP, y),
                    align,
                    format!("{GAIN} {}", stops_words(gain_stops(gain), 1)),
                    font,
                    ink,
                );
                pixel_dot(ui, at);
            }
        }
        // No gain, the same at every brightness.
        (None, _) => {
            line(
                pass,
                ui,
                vec![pos2(bars.x, up(0.0)), pos2(bars.right(), up(0.0))],
            );
            if let Some(pixel) = pixel {
                for (c, plane) in Plane::COLOR.into_iter().enumerate() {
                    if pixel.exposed[c] > 0.0 {
                        tick_across(ui, bars, across(bars, stops(pixel.exposed[c])), plane.ink());
                    }
                }
            }
        }
        (Some(_), None) => {}
    }
    caption(
        pass,
        ui,
        head,
        GAIN_TABLE,
        "",
        pixel.map(|pixel| channel_words(pixel.into_curve)),
    );
    tip_on(
        pass,
        ui,
        part_rect((head, bars)),
        "profile table",
        Tip::GainTable,
    );
    // No gain and the gain that undoes the exposure first, since they are
    // the rules, then the plot's top and foot where there is room.
    scale(pass, ui, bars, &[0.0, round_tenth(undoes), high, low], up);
    input_scale(pass, ui, bars);
}

/// `stops` to a tenth, as the scale writes it.
fn round_tenth(stops: f32) -> f32 {
    (stops * 10.0).round() / 10.0
}

/// One of the section's rows at `row`: its name down the left in the dim
/// ink, `value` after it, and the pointer's pixel's three channels at the
/// right in their own inks, as the Display section's rows are laid out.
fn row(
    pass: &Pass,
    ui: &egui::Ui,
    row: Rect,
    name: &str,
    value: &str,
    cells: Option<[String; 3]>,
    font: &FontId,
) {
    let middle = row.y + row.height / 2.0;
    let painter = ui.painter();
    let named = painter.text(
        pos2(pass.grid.snap(row.x), middle),
        egui::Align2::LEFT_CENTER,
        name,
        font.clone(),
        pass.theme.text_dim.into(),
    );
    if !value.is_empty() {
        painter.text(
            pos2(pass.grid.snap(named.right() + CELL_GAP), middle),
            egui::Align2::LEFT_CENTER,
            value,
            font.clone(),
            pass.theme.text_primary.into(),
        );
    }
    if let Some(cells) = cells {
        channel_cells(pass, ui, row.right(), middle, &cells, font);
    }
}

/// The tone curve's caption: its name.
pub(in crate::ui) const TONE_CURVE: &str = "Tone curve";

/// The gain table's caption: its name.
pub(in crate::ui) const GAIN_TABLE: &str = "Gain table";

/// The word on the guide to the blend's dot on the gain table: the gain the
/// table gave there.
const GAIN: &str = "gain";

/// The name of the row under the lift's plot: how far the profile lifted
/// each channel of the pointer's pixel, which is what the plot counts.
pub(in crate::ui) const TOTAL_LIFT: &str = "Total lift";

/// The names of the section's two rows: the exposure, and the developed
/// pixel in the profile's own space.
const EXPOSURE: &str = "Exposure";
pub(in crate::ui) const PROPHOTO: &str = "ProPhoto";

/// A stage at which a channel is black, which no number of stops is.
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
        assert_eq!(exposure_words(4.123), "+4.1 EV");
        assert_eq!(exposure_words(-1.27), "\u{2212}1.3 EV");
        assert_eq!(exposure_words(-0.0), "+0.0 EV");
        assert_eq!(stops_words(f32::NEG_INFINITY, 1), "black");
        assert_eq!(
            channel_words([1.0, 0.5, 0.0]),
            ["+0.0", "\u{2212}1.0", "black"]
        );
    }

    /// A pixel is followed through the profile a channel at a time: into
    /// ProPhoto, exposed, read by the table at a blend of the channels in
    /// exposed terms and multiplied by the one gain it gives, and put
    /// through the curve, which the brightest and dimmest channels take
    /// and the middle one keeps its place between.
    #[test]
    fn a_pixel_is_followed_through_the_profile_a_channel_at_a_time() {
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
        assert_eq!(pixel.prophoto, [0.5, 1.0, 0.25]);
        assert_eq!(pixel.exposed, [0.125, 0.25, 0.0625]);
        let (blend, gain) = pixel.table.expect("a table");
        // The table reads 0.3, 0.6 and 0.1 of the exposed channels, which
        // its weights' sum of 1 makes the same blend in exposed terms.
        assert!((blend - (0.3 * 0.125 + 0.6 * 0.25 + 0.1 * 0.0625)).abs() < 1e-6);
        assert_eq!(gain, 2.0);
        assert_eq!(pixel.into_curve, [0.25, 0.5, 0.125]);
        // The curve takes a half to a quarter and an eighth to a
        // sixteenth; the red, between them, keeps its place.
        assert_eq!(pixel.out[1], 0.25);
        assert_eq!(pixel.out[2], 0.0625);
        assert!((pixel.out[0] - (0.0625 + (0.25 - 0.0625) * (0.125 / 0.375))).abs() < 1e-6);
        assert_eq!(tone_order(pixel.into_curve), [1, 0, 2]);
        assert_eq!(
            channel_words(pixel.into_curve),
            ["\u{2212}2.0", "\u{2212}1.0", "\u{2212}3.0"]
        );
    }
}
