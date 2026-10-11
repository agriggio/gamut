//! The exposure's slider under the band.

use super::*;
use crate::ui::Command;
use crate::ui::slider::{self as line, Line};

/// The exposure's slider: the interface's one slider (see `ui::slider`),
/// from nothing each way, its fill running from the mark at nothing so
/// that the push reads as a length and a direction before the number is
/// read. Snapped to the quarter stops the keys count in, so that the
/// reading beside it is always one the keys could have reached, and asked
/// for through [`Command::Exposure`].
///
/// The run is [`SLIDER_STOPS`] each way. An exposure past that, from the
/// keys or `--exposure`, stands hollow at the end, as a band handle out past
/// the plot does. Dead where the picture has no exposure
/// (`Display::has_exposure`) — a measurement, whose values are not light —
/// and says why when rested on; the row stays, since the section's height
/// is the file's.
pub(super) fn slider(pass: &mut Pass, ui: &mut egui::Ui, exposure: f32, live: bool, room: Rect) {
    let ground = pass.theme.bar_background;
    let asked = line::show(
        pass,
        ui,
        Line {
            name: "Exposure",
            value: exposure,
            low: -SLIDER_STOPS,
            high: SLIDER_STOPS,
            step: EV_STEP,
            origin: 0.0,
            tip: Some(Tip::Exposure),
            live,
            ground,
        },
        room,
    );
    if let Some(asked) = asked {
        pass.commands.push(Command::Exposure(asked));
    }
}
