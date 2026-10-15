//! The Develop section, for a raw: the picture as the developer made it from
//! the sensor's counts — demosaiced, balanced, carried into Rec. 2020 —
//! which is what every stage above starts from and what the Image section,
//! plotting the counts themselves, no longer shows. It stands between the
//! two, where the development stands in the flow.
//!
//! Its plots are the developed channels, drawn as the Image section draws
//! stored channels ([`stored::body`]), with the developed pixel's values in
//! the header as the Image section writes the counts in its own: the pixel
//! read in the one header and in the other is the development of one pixel.
//! Under the plots, what the development did, a row each: the demosaic the
//! mosaic went through; the balance each plane was multiplied by, in stops,
//! which is why a channel against the right edge of a developed plot may
//! stand well short of it in the sensor's plot below; and the share of each
//! developed channel standing at white, which is the clipping the two plots
//! together say the sensor's or the balance's.

use super::stored::{self, Layout, Source};
use super::*;
use crate::image::sensor::Development;

/// How the section is laid out for a file: how its developed channels are
/// plotted, and how many rows of words it has — the light's only where the
/// file says what it was.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Shape {
    pub layout: Layout,
    pub rows: usize,
}

impl Shape {
    /// A file with no Develop section: what the other sections' heights
    /// are given, which read nothing of it.
    pub const NONE: Shape = Shape {
        layout: Layout::ONE,
        rows: 0,
    };

    pub fn of(current: &Current) -> Self {
        match stored::sensor_of(current) {
            Some(sensor) => Self {
                layout: Layout::of_source(&Source::developed(current)),
                rows: ROWS + usize::from(sensor.development.kelvin.is_some()),
            },
            None => Self::NONE,
        }
    }

    /// What the section holds under its header: the plots and their rows
    /// of values, and the rows of words.
    pub const fn height(self) -> f32 {
        self.layout.body_height() + self.rows as f32 * (display::ROW_GAP + display::ROW_HEIGHT)
    }
}

/// The rows of words every raw has under the plots, in the order they are
/// applied read upward as the column is: the demosaic first, then the
/// balance, then what stands clipped at the end of it. The light's row
/// goes after the balance's where there is one.
const ROWS: usize = 3;

/// The `index`th row of words in the section laid out at `rect`, under
/// the plots and their rows of values.
pub(super) fn row(rect: Rect, shape: Shape, index: usize) -> Rect {
    let top = rect.y + LABEL_HEIGHT + HEAD_GAP + PLOT_INSET + shape.layout.body_height();
    let y = top + index as f32 * (display::ROW_GAP + display::ROW_HEIGHT) + display::ROW_GAP;
    Rect::new(rect.x, y, rect.width, display::ROW_HEIGHT)
}

/// The Develop section, laid out at `rect`: the header with the developed
/// pixel's values, the developed channels' plots, and the rows of what
/// the development did.
pub(super) fn show(
    pass: &mut Pass,
    ui: &mut egui::Ui,
    current: &Current,
    rect: Rect,
    plotted: &stored::Plotted,
    mark: Option<&Mark>,
) {
    let Some(sensor) = stored::sensor_of(current) else {
        return;
    };
    let source = Source::developed(current);
    let shape = Shape::of(current);
    stored::body(pass, ui, &source, rect, plotted, mark);
    stored::header(pass, ui, &source, rect, mark);

    let font = FontId::proportional(ROW_TEXT);
    let development = &sensor.development;
    let mut rows = (0..shape.rows).map(|index| row(rect, shape, index));
    let mut next = || rows.next().expect("a row the shape counted");
    let demosaic = next();
    words_row(
        pass,
        ui,
        demosaic,
        DEMOSAIC,
        development.demosaic.label(),
        &font,
    );
    // What the name means, under the pointer.
    let response = ui.interact(
        egui::Rect::from(demosaic),
        ui.id().with("develop demosaic"),
        Sense::hover(),
    );
    pass.tooltip(response, Tip::Demosaic(development.demosaic));
    let planes: Vec<Plane> = (0..source.stored.planes.len())
        .map(|index| {
            if source.gray {
                Plane::Luma
            } else {
                Plane::COLOR[index.min(Plane::COLOR.len() - 1)]
            }
        })
        .collect();
    balance_row(pass, ui, next(), development, &planes, &font);
    if let Some(kelvin) = development.kelvin {
        words_row(pass, ui, next(), LIGHT, &kelvin_words(kelvin), &font);
    }
    let clipped: Vec<(String, Color32)> = source
        .stored
        .planes
        .iter()
        .zip(&planes)
        .map(|(plane, ink)| {
            // Below white and half of those at it is the rank of white, so
            // what stands at it is twice the rest.
            let share = plane
                .rank(1.0)
                .map_or(String::new(), |rank| share_words(2.0 * (1.0 - rank)));
            (share, ink.ink())
        })
        .collect();
    let clipped_row = next();
    cells_row(pass, ui, clipped_row, CLIPPED, &clipped, &font);
    let response = ui.interact(
        egui::Rect::from(clipped_row),
        ui.id().with("develop clipped"),
        Sense::hover(),
    );
    pass.tooltip(response, Tip::Clipped);
}

/// A row of one value: its name down the left, and the value at the right.
fn words_row(pass: &Pass, ui: &egui::Ui, row: Rect, name: &str, value: &str, font: &FontId) {
    cells_row(
        pass,
        ui,
        row,
        name,
        &[(value.to_string(), pass.theme.text_primary.into())],
        font,
    );
}

/// The balance row: what each plane was multiplied by, in stops, in the
/// plane's color — or the one word for counts balanced as stored.
fn balance_row(
    pass: &Pass,
    ui: &egui::Ui,
    row: Rect,
    development: &Development,
    planes: &[Plane],
    font: &FontId,
) {
    if development.balanced_as_stored() || planes.len() == 1 {
        return words_row(pass, ui, row, BALANCE, APPLIED_IN_FILE, font);
    }
    let cells: Vec<(String, Color32)> = planes
        .iter()
        .zip(development.balance)
        .map(|(plane, multiplier)| {
            (
                stops_words(multiplier.max(f32::MIN_POSITIVE).log2()),
                plane.ink(),
            )
        })
        .collect();
    cells_row(pass, ui, row, BALANCE, &cells, font);
}

/// A row of cells: its name down the left in the dim ink, and the cells
/// from the right, each in its own ink, the last of them flush with the
/// row's end.
fn cells_row(
    pass: &Pass,
    ui: &egui::Ui,
    row: Rect,
    name: &str,
    cells: &[(String, Color32)],
    font: &FontId,
) {
    let middle = row.y + row.height / 2.0;
    let painter = ui.painter();
    painter.text(
        pos2(pass.grid.snap(row.x), middle),
        egui::Align2::LEFT_CENTER,
        name,
        font.clone(),
        pass.theme.text_dim.into(),
    );
    let mut end = row.right();
    for (words, ink) in cells.iter().rev() {
        let galley = painter.layout_no_wrap(words.clone(), font.clone(), Color32::PLACEHOLDER);
        let width = galley.size().x;
        let at = pos2(pass.grid.snap(end - width), middle - galley.size().y / 2.0);
        painter.galley(at, galley, *ink);
        end -= width + CELL_SPACING;
    }
}

/// The room between one cell of a row and the next.
const CELL_SPACING: f32 = 10.0;

/// A multiplier in stops, as the balance row writes it: signed, to two
/// places, with a true minus.
fn stops_words(stops: f32) -> String {
    // Adding zero makes a negative zero positive.
    format!("{:+.2} EV", stops + 0.0).replace('-', "\u{2212}")
}

/// A color temperature as the light's row writes it: to the nearest fifty
/// kelvin, as the information panel writes the same number.
fn kelvin_words(kelvin: f32) -> String {
    format!("{} K", (kelvin / 50.0).round() as u32 * 50)
}

/// A share of a plane's pixels as the clipped row writes it: a percentage
/// to a tenth, `0%` for none at all, and `<0.1%` for a few.
fn share_words(share: f32) -> String {
    let percent = share.clamp(0.0, 1.0) * 100.0;
    if percent == 0.0 {
        "0%".to_string()
    } else if percent < 0.1 {
        "<0.1%".to_string()
    } else {
        format!("{percent:.1}%")
    }
}

/// The names of the section's rows.
pub(in crate::ui) const DEMOSAIC: &str = "Demosaic";
pub(in crate::ui) const BALANCE: &str = "Balance";
pub(in crate::ui) const LIGHT: &str = "Light";
pub(in crate::ui) const CLIPPED: &str = "Clipped";

/// What the balance row says where the camera balanced the counts before
/// writing them, and the development multiplied nothing.
const APPLIED_IN_FILE: &str = "applied in the file";

#[cfg(test)]
mod tests {
    use super::*;

    /// Stops are written signed with a true minus, and shares as the
    /// percentile words are.
    #[test]
    fn the_rows_write_stops_and_shares() {
        assert_eq!(stops_words(1.0), "+1.00 EV");
        assert_eq!(stops_words(-0.5), "\u{2212}0.50 EV");
        assert_eq!(stops_words(-0.0), "+0.00 EV");
        assert_eq!(share_words(0.0), "0%");
        assert_eq!(share_words(0.0004), "<0.1%");
        assert_eq!(share_words(0.0123), "1.2%");
        assert_eq!(share_words(1.5), "100.0%");
        assert_eq!(kelvin_words(5226.0), "5250 K");
    }
}
