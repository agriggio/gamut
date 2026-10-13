//! The histogram lab: the side panel laid out over a real file and drawn
//! to pixels, so that its look can be judged off the screen and ways of
//! drawing it compared side by side — a row to a file, and in each row the
//! panel on SDR and then on HDR, in a column of its own. Not a test
//! of anything; run it by hand:
//!
//! ```sh
//! cargo test --release ui::lab -- --ignored --nocapture
//! ```
//!
//! `GAMUT_LAB_IMAGES` names the files, separated by colons, and
//! `GAMUT_LAB_OUT` where the picture goes (`target/histograms.png`).

use std::path::PathBuf;
use std::sync::Arc;

use egui_kittest::Harness;

use crate::image::decode::{Opened, Overrides};
use crate::image::display::Headroom;
use crate::image::exif::Exif;
use crate::image::{DecodedImage, Stats};
use crate::theme::Theme;
use crate::view::View;

use super::chrome;
use super::control::Unnamed;
use super::driven::{input, panels};
use super::side::Side;
use super::{Current, FrameInput, Panels};

/// The window the panel is laid out in: tall enough for the whole column
/// without scrolling, and drawn at two device pixels to the point.
const WINDOW: [f32; 2] = [1400.0, 1100.0];
const SCALE: f32 = 2.0;

/// How far above white the HDR column's surface goes.
const HDR_ROOM: f32 = 4.0;

/// Between one column of the picture and the next, in device pixels.
const GUTTER: u32 = 16;

struct State {
    ready: bool,
    theme: Theme,
    input: FrameInput,
    panels: Panels,
    current: Current,
    view: View,
}

/// The files drawn, a row each, as `GAMUT_LAB_IMAGES` names them.
fn paths() -> Vec<PathBuf> {
    let paths = std::env::var_os("GAMUT_LAB_IMAGES")
        .expect("GAMUT_LAB_IMAGES naming the files to draw, separated by colons");
    std::env::split_paths(&paths).collect()
}

/// The file as the loader hands it over, and its name.
fn decoded(path: &std::path::Path) -> (DecodedImage, Exif) {
    let mut opened = Opened::new(path).expect("opening the file");
    let exif = Exif::read_with(path, Some(&mut opened));
    let (image, _) = opened
        .decode(Overrides::default(), None)
        .expect("decoding the file");
    (image, exif)
}

/// The file on screen as the application holds it on a surface with `room`
/// above white: the base measured where there is none, and the picture
/// measured through its gain map's lift where there is, as
/// `App::refresh_lift` does; a raw's profile is lifted on either, with the
/// switch on.
fn current(image: DecodedImage, exif: &Exif, name: &str, room: f32) -> Current {
    let mut current = Current::of(image, name);
    current.exif = exif.clone();
    if let Some(map) = &current.image.gain_map
        && (room > 1.0 || map.is_profile())
    {
        let table = Arc::new(map.table(map.weight(room)));
        current.stats = Stats::scan_with(&current.image, Some(&table));
        current.lift = Some(table);
    }
    current
}

/// The side panel holding the histogram, laid out over `current`.
fn render(current: Current, room: f32) -> image::RgbaImage {
    let mut panels = panels();
    panels.side = Some(Side::Histogram);
    let mut input = input(WINDOW, 1);
    input.scale = SCALE;
    input.side = Some(Side::Histogram);
    input.room = room;
    input.headroom = match room > 1.0 {
        true => Headroom::Above,
        false => Headroom::None,
    };
    input.hdr_available = true;
    let parts = chrome::Parts {
        side: true,
        ..chrome::Parts::NONE
    };
    input.viewport =
        chrome::image_viewport([WINDOW[0] * SCALE, WINDOW[1] * SCALE], SCALE, true, parts);
    let state = State {
        ready: false,
        theme: Theme::detect(),
        input,
        panels,
        current,
        view: View::new(),
    };
    let mut harness = Harness::builder()
        .with_size(egui::vec2(WINDOW[0], WINDOW[1]))
        .with_pixels_per_point(SCALE)
        .wgpu()
        .build_ui_state(
            |ui, state: &mut State| {
                if !state.ready {
                    ui.ctx()
                        .set_fonts(super::fonts::system().expect("the desktop's fonts"));
                    super::style::apply(ui.ctx(), &state.theme);
                    state.ready = true;
                    return;
                }
                let _ = super::show(
                    ui,
                    &state.input,
                    &state.panels,
                    Some(&state.current),
                    &state.view,
                    &state.theme,
                    &Unnamed,
                );
            },
            state,
        );
    harness.run();
    let shot = harness.render().expect("rendering the pass");

    // The side panel alone, as `Chrome` places it, down to the last row
    // that differs from its own ground — read short of the right edge, where
    // the scroll track runs the panel's whole height, and short of the foot,
    // where the bottom bar's buttons are.
    let side = chrome::Chrome::new(WINDOW, parts)
        .side
        .expect("room for the side panel");
    let [x, y, w, h] =
        [side.x, side.y, side.width, side.height].map(|v| (v * SCALE).round() as u32);
    let panel = image::imageops::crop_imm(&shot, x, y, w, h).to_image();
    let ground = *panel.get_pixel(w / 2, h - 1);
    let used = (0..h * 9 / 10)
        .rev()
        .find(|&row| (0..w * 85 / 100).any(|col| *panel.get_pixel(col, row) != ground))
        .map_or(h, |row| row + 1);
    let foot = (used + GUTTER).min(h);
    image::imageops::crop_imm(&panel, 0, 0, w, foot).to_image()
}

/// The pictures beside one another, or under one another, on black.
fn tiled(tiles: &[image::RgbaImage], across: bool) -> image::RgbaImage {
    let along = |tile: &image::RgbaImage| if across { tile.width() } else { tile.height() };
    let breadth = |tile: &image::RgbaImage| if across { tile.height() } else { tile.width() };
    let length =
        tiles.iter().map(along).sum::<u32>() + GUTTER * tiles.len().saturating_sub(1) as u32;
    let thickness = tiles.iter().map(breadth).max().unwrap_or(0);
    let (width, height) = if across {
        (length, thickness)
    } else {
        (thickness, length)
    };
    let mut sheet = image::RgbaImage::from_pixel(width, height, image::Rgba([0, 0, 0, 255]));
    let mut at = 0;
    for tile in tiles {
        let (x, y) = if across { (at, 0) } else { (0, at) };
        image::imageops::replace(&mut sheet, tile, x as i64, y as i64);
        at += along(tile) + GUTTER;
    }
    sheet
}

#[test]
#[ignore = "writes a picture of the histogram for judging by eye"]
fn histograms() {
    let rows: Vec<_> = paths()
        .iter()
        .map(|path| {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let (image, exif) = decoded(path);
            let columns: Vec<_> = [1.0, HDR_ROOM]
                .into_iter()
                .map(|room| render(current(image.clone(), &exif, &name, room), room))
                .collect();
            tiled(&columns, true)
        })
        .collect();
    let out = std::env::var_os("GAMUT_LAB_OUT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/histograms.png"));
    tiled(&rows, false).save(&out).expect("writing the picture");
    println!("{}", out.display());
}
