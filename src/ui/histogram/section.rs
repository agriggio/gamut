//! A section of the column: which stage of the pipeline it is, how tall it
//! stands, where it goes, and the line it opens with.

use egui::{Align2, FontId, pos2};

use super::*;
use crate::image::Referred;
use crate::image::auxiliary::{Auxiliary, Showing};

/// One stage of the picture's way from the file to the screen, as a
/// section of the column. In the order the column lays them out, which is
/// the order the data flows in read back from the screen: what goes out at
/// the top, since that is the question the panel is most often opened to
/// answer, and the file at the foot.
///
/// A stage the picture does not pass through has no section — the gain map
/// on a picture with none — and one a later change adds goes in where it
/// stands in the flow, between the display and the file, at a height of its
/// own.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Section {
    /// What goes out to the screen: the display's response to every value,
    /// clipped where the surface stops.
    Output,
    /// What the display starts from, and what it does to it: the window,
    /// the exposure and the curve, which are set here.
    Display,
    /// How far the gain map lifts the picture, where it has one.
    GainMap,
    /// The picture as the file stores it.
    Image,
}

impl Section {
    /// Every section there can be, in the order the column lays them out.
    pub const ALL: [Section; 4] = [
        Section::Output,
        Section::Display,
        Section::GainMap,
        Section::Image,
    ];

    /// What the section's header calls it.
    pub fn title(self) -> &'static str {
        match self {
            Section::Output => "Output",
            Section::Display => "Display",
            Section::GainMap => "Gain map",
            Section::Image => "Image",
        }
    }

    /// The mark beside its name, in the accent, as the information panel's
    /// sections wear theirs: a screen for what goes out, the sliders the
    /// display is set by, the sun the gain map is known by elsewhere in the
    /// window, and a picture for the image, as the information panel's
    /// Image section wears.
    pub(in crate::ui) fn mark(self) -> &'static [icon::Mark] {
        match self {
            Section::Output => icon::MONITOR,
            Section::Display => icon::SLIDERS_HORIZONTAL,
            Section::GainMap => icon::SUN,
            Section::Image => icon::IMAGE,
        }
    }

    /// Whether the section has a button of its own at the end of its
    /// header: the Gain map section's, which shows the map.
    pub fn has_button(self) -> bool {
        matches!(self, Section::GainMap)
    }

    /// Whether `current` passes through this stage. The Gain map section is
    /// there while the picture or its map is on screen, and left out while
    /// another of the file's images is, which nothing lifts. The Image
    /// section is left out while the gain map is shown in the picture's
    /// place: what the display starts from is then the map in stops, which
    /// is the Gain map section's plot already, and the file stores neither
    /// it nor the picture under it.
    pub fn applies(self, current: &Current) -> bool {
        match self {
            Section::GainMap => {
                current.picture_face().image.gain_map.is_some()
                    && matches!(
                        current.showing,
                        Showing::Picture | Showing::Auxiliary(Auxiliary::GainMap)
                    )
            }
            Section::Image => current.showing != Showing::Auxiliary(Auxiliary::GainMap),
            Section::Output | Section::Display => true,
        }
    }

    /// How tall the section stands: its header, its plot, and what hangs
    /// under the plot. Fixed but for the Display section's row of false
    /// colors, which only a `gray` image has, and its row of windows, which
    /// only an image read as `data` has, so that one file's column is laid
    /// out where the last one's was, and a section is never shrunk to fit a
    /// short window — the column scrolls instead.
    pub const fn height(self, gray: bool, data: bool) -> f32 {
        let head = LABEL_HEIGHT + HEAD_GAP + PLOT_INSET;
        let band = PLOT_INSET + RAMP_GAP + RAMP_HEIGHT;
        match self {
            // The plot, the band of what each output comes out as, and the
            // row of stops under it.
            Section::Output => head + PLOT_HEIGHT + band + EV_ROW,
            // The plot, the band, the row of stops, on a gray image the row
            // of false colors, and the rows of settings under them, which
            // take the inset under the plot as their own: see
            // `display::rows_height`.
            Section::Display => {
                let swatches = if gray { display::SWATCH_ROW } else { 0.0 };
                head + PLOT_HEIGHT
                    + RAMP_GAP
                    + RAMP_HEIGHT
                    + EV_ROW
                    + swatches
                    + display::rows_height(data)
            }
            // A shorter plot and the row of stops under it: how much of the
            // lift is applied is the header's to say.
            Section::GainMap => head + SHORT_PLOT_HEIGHT + PLOT_INSET + EV_ROW,
            // A shorter plot and the row of values under it.
            Section::Image => head + SHORT_PLOT_HEIGHT + PLOT_INSET + EV_ROW,
        }
    }

    /// The ground the bins stand on in the section laid out at `rect`: the
    /// whole width of the column, less the inset the ground is drawn out
    /// into, which leaves the bins a point each.
    pub fn plot(self, rect: Rect) -> Rect {
        let x = rect.x + PLOT_INSET;
        let y = rect.y + LABEL_HEIGHT + HEAD_GAP + PLOT_INSET;
        let width = rect.width - 2.0 * PLOT_INSET;
        match self {
            Section::Output => Rect::new(x, y, width, PLOT_HEIGHT),
            Section::Display => display::plot_area(rect),
            Section::GainMap | Section::Image => Rect::new(x, y, width, SHORT_PLOT_HEIGHT),
        }
    }
}

/// How tall the column is for `current`: every section it passes through,
/// and the gaps between them.
pub fn height(current: &Current) -> f32 {
    let sections: Vec<Section> = Section::ALL
        .into_iter()
        .filter(|section| section.applies(current))
        .collect();
    let (gray, data) = (current.image.is_gray(), is_data(current));
    sections
        .iter()
        .map(|section| section.height(gray, data))
        .sum::<f32>()
        + (sections.len().saturating_sub(1)) as f32 * SECTION_GAP
}

/// Where each section `current` passes through goes in the column laid out
/// from the top of `column`: one under the other, each at its own height and
/// the column's width, whatever height `column` has.
pub fn layout(column: Rect, current: &Current) -> Vec<(Section, Rect)> {
    let mut y = column.y;
    let (gray, data) = (current.image.is_gray(), is_data(current));
    Section::ALL
        .into_iter()
        .filter(|section| section.applies(current))
        .map(|section| {
            let rect = Rect::new(column.x, y, column.width, section.height(gray, data));
            y += rect.height + SECTION_GAP;
            (section, rect)
        })
        .collect()
}

/// Whether the image on screen is read as data — a measurement, windowed
/// to what it holds — which is what gives the Display section its row of
/// windows; a photo's window is as stored and the handles'.
pub fn is_data(current: &Current) -> bool {
    current.image.referred == Referred::Measured
}

/// The section's header line, in the section laid out at `rect`.
pub fn header_line(rect: Rect) -> Rect {
    Rect::new(rect.x, rect.y, rect.width, LABEL_HEIGHT)
}

/// The square at the end of a section's header that its own button takes,
/// where it has one: the gain map's, which shows the map.
pub fn button(rect: Rect) -> Rect {
    let line = header_line(rect);
    Rect::new(
        line.right() - LABEL_HEIGHT,
        line.y,
        LABEL_HEIGHT,
        LABEL_HEIGHT,
    )
}

/// The line a section opens with, as the information panel's sections
/// open: the section's mark and name in the accent, the one thing on the
/// panel picked out in it; and on the right what the pointer is reading at
/// its stage — `readout` — ending where [`readout_end`] says.
pub fn header(pass: &Pass, ui: &egui::Ui, section: Section, rect: Rect, readout: Option<&str>) {
    let theme = pass.theme;
    let grid = pass.grid;
    let painter = ui.painter();
    let line = header_line(rect);
    let middle = line.y + line.height / 2.0;
    let mark = egui::Rect::from_min_size(pos2(line.x, line.y), egui::vec2(ICON_SIDE, line.height));
    icon::paint(
        painter,
        section.mark(),
        icon::square(grid, mark, ICON_SIDE),
        theme.accent.into(),
        theme.bar_background.into(),
    );
    painter.text(
        pos2(grid.snap(line.x + ICON_SIDE + MARK_GAP), middle),
        Align2::LEFT_CENTER,
        section.title(),
        FontId::proportional(TEXT_SIZE),
        theme.accent.into(),
    );
    if let Some(readout) = readout {
        painter.text(
            pos2(grid.snap(readout_end(section, line)), middle),
            Align2::RIGHT_CENTER,
            readout,
            FontId::proportional(ROW_TEXT),
            theme.text_primary.into(),
        );
    }
}

/// Where a header's readout ends: short of the square at the end of the
/// line where the section has a button of its own, [`button`], and at the
/// line's end where it does not.
pub fn readout_end(section: Section, line: Rect) -> f32 {
    if section.has_button() {
        line.right() - LABEL_HEIGHT - CELL_GAP
    } else {
        line.right()
    }
}
