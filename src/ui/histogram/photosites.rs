//! The photosites around the pointer's pixel, at the foot of the Image
//! section for a raw that keeps its mosaic: a window of the counts a
//! demosaic reads to make the pixel, each in its plane's color, which no
//! number worked out of them could show. They are the sensor's counts, as
//! the section's plots are, whether the developed picture or the mosaic
//! itself is up; and their caption carries the button that puts the
//! mosaic up in the picture's place.

use egui::Stroke;

use super::plot::PLOT_BACKGROUND;
use super::stored;
use super::*;
use crate::image::auxiliary::Auxiliary;

/// How many photosites each way of the pointer's the window shows: two,
/// which is every photosite a demosaic reads for the pixel, and a 5 by 5
/// window whose counts still fit their cells.
const GRID_REACH: u32 = 2;
/// How tall each row of the window is: a line of the plot's small words.
const CELL_HEIGHT: f32 = 14.0;
/// The window's height.
const GRID_HEIGHT: f32 = (2 * GRID_REACH + 1) as f32 * CELL_HEIGHT;

/// What the photosites take under what they follow: the gap, the caption —
/// as tall as a section's header, for the button at its end — and the
/// window set in as the plots are.
pub(super) const HEIGHT: f32 =
    section::PART_GAP + LABEL_HEIGHT + PLOT_INSET + GRID_HEIGHT + PLOT_INSET;

/// How strongly a cell is washed with its plane's color under its count.
const CELL_WASH: f32 = 0.22;

/// The photosites' caption and ground in the section laid out at `rect`,
/// under what ends at `top`: the caption the width of the section, the
/// ground set in as the plots are.
pub(super) fn part(rect: Rect, top: f32) -> (Rect, Rect) {
    let x = rect.x + PLOT_INSET;
    let width = rect.width - 2.0 * PLOT_INSET;
    let caption = Rect::new(rect.x, top + section::PART_GAP, rect.width, LABEL_HEIGHT);
    let ground = Rect::new(x, caption.bottom() + PLOT_INSET, width, GRID_HEIGHT);
    (caption, ground)
}

/// The square at the end of the photosites' caption that the mosaic's
/// button takes.
fn button_square(caption: Rect) -> Rect {
    Rect::new(
        caption.right() - LABEL_HEIGHT,
        caption.y,
        LABEL_HEIGHT,
        LABEL_HEIGHT,
    )
}

/// The photosites around the pointer's pixel, under what ends at `top` in
/// the section laid out at `rect`: the caption, with the button that shows
/// the mosaic in the picture's place at its end — the eye the gain map's
/// wears, lit while the mosaic is up — and under it the window of counts,
/// a cell to the photosite, each washed with its plane's color with the
/// count in it, the pointer's own outlined. Off the picture, the ground
/// alone.
pub(super) fn show(pass: &mut Pass, ui: &mut egui::Ui, current: &Current, rect: Rect, top: f32) {
    let (caption, ground) = part(rect, top);
    // The words centered on the line the button stands on.
    let words = Rect::new(
        caption.x,
        caption.y + (caption.height - section::PART_CAPTION_HEIGHT) / 2.0,
        caption.width,
        section::PART_CAPTION_HEIGHT,
    );
    profile::caption(pass, ui, words, PHOTOSITES, None);
    icon_button(
        pass,
        ui,
        button_square(caption),
        Control::ShowMosaic,
        current.showing == Showing::Auxiliary(Auxiliary::Mosaic),
        icon::EYE,
    );
    let painter = ui.painter();
    painter.rect_filled(
        egui::Rect::from(ground.inset(-PLOT_INSET, -PLOT_INSET)),
        PLOT_RADIUS,
        PLOT_BACKGROUND,
    );
    let Some(sensor) = stored::counts_of(current) else {
        return;
    };
    let Some([x, y]) = pass.input.pointer else {
        return;
    };
    let image = &current.image;
    let size = [image.width, image.height];
    let [width, height] = current.pixels();
    if x >= width || y >= height {
        return;
    }
    let side = 2 * GRID_REACH + 1;
    let cell = egui::vec2(ground.width / side as f32, CELL_HEIGHT);
    let font = FontId::proportional(CLIP_TEXT);
    let gray = sensor.planes() == 1;
    // The window is read as the picture is shown, a cell to the pixel
    // around the pointer's, each found on the sensor through the turn the
    // user asked for and the camera's.
    for row in 0..side {
        for column in 0..side {
            let min = pos2(
                ground.x + column as f32 * cell.x,
                ground.y + row as f32 * cell.y,
            );
            let square = egui::Rect::from_min_size(min, cell);
            let (Some(tx), Some(ty)) = (
                (x + column).checked_sub(GRID_REACH),
                (y + row).checked_sub(GRID_REACH),
            ) else {
                continue;
            };
            if tx >= width || ty >= height {
                continue;
            }
            let at = current.turn.stored([tx, ty], size);
            let Some((plane, count)) = sensor.photosite(at, size) else {
                continue;
            };
            let plane = if gray {
                Plane::Luma
            } else {
                Plane::COLOR[plane.min(Plane::COLOR.len() - 1)]
            };
            painter.rect_filled(square, 0.0, plane.ink().gamma_multiply(CELL_WASH));
            painter.text(
                square.center(),
                egui::Align2::CENTER_CENTER,
                count.to_string(),
                font.clone(),
                plane.ink(),
            );
        }
    }
    let center = GRID_REACH as f32;
    let own = egui::Rect::from_min_size(
        pos2(ground.x + center * cell.x, ground.y + center * cell.y),
        cell,
    );
    painter.rect_stroke(
        own,
        0.0,
        Stroke::new(
            pass.grid.line_width(RULE_WIDTH),
            Color32::from(pass.theme.text_primary),
        ),
        egui::StrokeKind::Inside,
    );
}

/// The photosites' caption: its name.
pub(in crate::ui) const PHOTOSITES: &str = "Photosites";
