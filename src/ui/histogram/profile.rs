//! The Profile section, for a raw whose profile says how it should look:
//! how far the profile's rendering lifts each pixel, as a histogram in
//! stops, as the Gain map section plots a photograph's lift; and under it
//! the profile's tone curve, which differs from file to file in its toe
//! and is shown nowhere else. The square at the end of the header is the
//! profile's switch, and the header says whether it is on.

use egui::Stroke;

use super::plot::{PLOT_BACKGROUND, bars};
use super::*;
use crate::image::gain_map::{GainMap, Profile};

/// How far below white the curve's two axes reach, in stops: past the toe
/// of every profile sampled, which falls away below four.
const CURVE_STOPS: i32 = 10;

/// How big the dot is that places the pointer's pixel on the curve.
const PIXEL_DOT: f32 = 2.5;

/// Whether the profile's rendering is on screen.
fn applied(current: &Current) -> bool {
    current
        .picture_face()
        .lift
        .as_ref()
        .is_some_and(|lift| lift.weight() > 0.0)
}

/// What the header says with nothing under the pointer: the profile's
/// baseline exposure, and whether its rendering is on screen.
fn summary(profile: &Profile, applied: bool) -> String {
    // Adding zero makes a negative zero positive.
    let baseline = format!("{:+.1}", profile.baseline + 0.0).replace('-', "\u{2212}");
    let state = if applied { "applied" } else { "not applied" };
    format!("{baseline} EV, {state}")
}

/// Where the pointer's pixel `(x, y)` of the turned picture stands on the
/// curve, as `[input, output]`, where it is on the picture and lit: the
/// developed color lifted by the whole of the profile, whether or not the
/// switch has it on screen, since the curve is the profile's.
fn on_curve(
    current: &Current,
    map: &GainMap,
    profile: &Profile,
    x: u32,
    y: u32,
) -> Option<[f32; 2]> {
    let face = current.picture_face();
    let size = [face.image.width, face.image.height];
    let [width, height] = current.turn.size(size);
    if x >= width || y >= height {
        return None;
    }
    let [x, y] = current.turn.stored([x, y], size);
    let sample = face.image.sample_as_stored(x, y)?;
    let linear = sample.linear();
    let color = [0, 1, 2].map(|c| linear.get(c).copied().unwrap_or(linear[0]));
    let ratios = map.gain_at(&map.table(1.0), x, y, size[0], size[1]);
    profile.on_curve(color, ratios)
}

/// The Profile section, laid out at `rect`: the switch, the histogram of
/// the lift and the tone curve.
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
        .unwrap_or_else(|| summary(profile, applied));
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
        None => {
            ui.painter().rect_filled(
                egui::Rect::from(plot_rect.inset(-PLOT_INSET, -PLOT_INSET)),
                PLOT_RADIUS,
                PLOT_BACKGROUND,
            );
        }
    }

    let pixel = pass
        .input
        .pointer
        .and_then(|[x, y]| on_curve(current, map, profile, x, y));
    curve(pass, ui, profile, section::curve_plot(rect), pixel);
}

/// The tone curve on its own ground at `bars`, both axes in stops below
/// white, and the pointer's pixel on it at `pixel`.
///
/// In stops, since all that tells one profile's curve from another's is
/// its toe, a hundredth of the way up a linear plot: in stops the shoulder
/// is the straight line every curve shares and the toe falls away where it
/// does. The diagonal, which a curve doing nothing would be, is drawn
/// faintly under it.
fn curve(pass: &Pass, ui: &egui::Ui, profile: &Profile, bars: Rect, pixel: Option<[f32; 2]>) {
    let theme = pass.theme;
    let painter = ui.painter();
    painter.rect_filled(
        egui::Rect::from(bars.inset(-PLOT_INSET, -PLOT_INSET)),
        PLOT_RADIUS,
        PLOT_BACKGROUND,
    );
    let reach = CURVE_STOPS as f32;
    let place = |[input, output]: [f32; 2]| {
        pos2(
            bars.x + (input + reach) / reach * bars.width,
            bars.bottom() - (output + reach) / reach * bars.height,
        )
    };
    let stops = |value: f32| value.max(f32::MIN_POSITIVE).log2().clamp(-reach, 0.0);
    painter.line_segment(
        [place([-reach, -reach]), place([0.0, 0.0])],
        Stroke::new(pass.grid.line_width(RULE_WIDTH), theme.border),
    );
    // A vertex to the device pixel, so that the toe is drawn as finely as
    // the screen can show it.
    let columns = pass.grid.columns(bars.x, bars.width).len().max(1);
    let points: Vec<egui::Pos2> = (0..=columns)
        .map(|column| {
            let input = -reach + reach * column as f32 / columns as f32;
            place([input, stops(profile.curve_at(input.exp2()))])
        })
        .collect();
    // Over an outline in the plot's ground, as the Display section's curve
    // is drawn.
    painter.add(egui::Shape::line(
        points.clone(),
        Stroke::new(CURVE_WIDTH + 2.0 * CURVE_OUTLINE, PLOT_BACKGROUND),
    ));
    painter.add(egui::Shape::line(
        points,
        Stroke::new(CURVE_WIDTH, theme.accent),
    ));
    if let Some([input, output]) = pixel {
        painter.circle(
            place([stops(input), stops(output)]),
            PIXEL_DOT,
            Color32::WHITE,
            Stroke::new(CURVE_OUTLINE, PLOT_BACKGROUND),
        );
    }
    painter.text(
        pos2(bars.x + CLIP_INSET, bars.y + CLIP_INSET),
        egui::Align2::LEFT_TOP,
        TONE_CURVE,
        FontId::proportional(CLIP_TEXT),
        theme.text_dim.into(),
    );
    plot::ev_scale(
        pass,
        ui,
        ev_row(bars, bars.bottom() + PLOT_INSET),
        -CURVE_STOPS..=0,
        |stop| Some((stop + CURVE_STOPS) as f32 / reach),
    );
}

/// What the curve's ground says it is.
pub(in crate::ui) const TONE_CURVE: &str = "Tone curve";

#[cfg(test)]
mod tests {
    use super::*;

    /// The header says the baseline signed, with a true minus, and whether
    /// the rendering is on screen.
    #[test]
    fn the_header_says_the_baseline_and_whether_it_is_applied() {
        let profile = |baseline| Profile {
            baseline,
            table: None,
            curve: None,
            range: [0.0; 2],
            to_curve: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
        };
        assert_eq!(summary(&profile(4.123), true), "+4.1 EV, applied");
        assert_eq!(
            summary(&profile(-1.27), false),
            "\u{2212}1.3 EV, not applied"
        );
        assert_eq!(summary(&profile(-0.0), true), "+0.0 EV, applied");
    }
}
