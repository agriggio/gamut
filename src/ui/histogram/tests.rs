use super::display::{Rows, plot_area, row_buttons, swatch_button};
use super::*;
use crate::image::auxiliary::{Auxiliary, Showing};
use crate::image::depth::DepthMap;
use crate::image::display::{Colormap, Headroom, ToneMap};
use crate::image::gain_map::{GainMap, Lift};
use crate::image::stats::Axis;
use crate::image::{AlphaMode, Channels, ColorSpace, DecodedImage, Referred, Samples};
use crate::ui::Face;
use crate::ui::side;
use crate::ui::slider::{HANDLE_GRIP, HANDLE_WIDTH};

/// `image`, on screen.
fn shown(image: DecodedImage) -> Current {
    Current::of(image, "file")
}

/// An 8-bit sRGB picture, color or gray, with a ramp across it.
fn picture(gray: bool) -> Current {
    let channels = if gray { Channels::Gray } else { Channels::Rgb };
    let count = channels.count();
    let data = (0..64u32)
        .flat_map(|x| std::iter::repeat_n((x * 4) as u8, count))
        .collect();
    shown(DecodedImage::new(
        64,
        1,
        Samples::U8 { channels, data },
        ColorSpace::SRGB,
        AlphaMode::Opaque,
    ))
}

/// Scene light stored as floats, as a Radiance picture is: a ramp of a
/// stop to every `per_stop` pixels from fourteen stops under 1 to eight
/// over, metered on arrival.
fn scene(per_stop: usize) -> Current {
    let stops = 22 * per_stop;
    let data: Vec<f32> = (0..stops)
        .map(|x| (x as f32 / per_stop as f32 - 14.0).exp2())
        .collect();
    let mut image = DecodedImage::new(
        stops as u32,
        1,
        Samples::F32 {
            channels: Channels::Gray,
            data,
        },
        ColorSpace::LINEAR_BT709,
        AlphaMode::Opaque,
    );
    image.referred = Referred::Scene;
    shown(image)
}

/// The same color picture with a gray gain map beside it, lifting its right
/// half by two stops.
fn gain_mapped() -> Current {
    let mut image = picture(false).image.as_ref().clone();
    image.gain_map = Some(std::sync::Arc::new(GainMap {
        width: 2,
        height: 1,
        channels: 1,
        data: vec![0, 255],
        lift: Lift::Apple { headroom: 4.0 },
    }));
    shown(image)
}

/// The column laid out from the top of a scroll area `height` tall.
fn laid_out(current: &Current, height: f32) -> Vec<(Section, Rect)> {
    section::layout(Rect::new(0.0, 0.0, COLUMN_WIDTH, height), current)
}

/// Where `section` was laid out.
fn rect_of(layout: &[(Section, Rect)], section: Section) -> Rect {
    layout
        .iter()
        .find(|(laid, _)| *laid == section)
        .map(|(_, rect)| *rect)
        .unwrap_or_else(|| panic!("{section:?} is laid out"))
}

/// The exposure is stepped in quarter stops, so it is written in
/// quarters: a tenth of a stop cannot say what one press is worth, and
/// would read one step as `0.2` and the next but one as `0.8`. Only a
/// value from `--exposure` can be anything else, and it is written back
/// as it was asked for.
#[test]
fn an_exposure_is_written_in_the_quarters_it_is_stepped_in() {
    assert_eq!(stops_label(0.0), "0");
    assert_eq!(stops_label(EV_STEP), "+\u{00bc}");
    assert_eq!(stops_label(-EV_STEP), "-\u{00bc}");
    assert_eq!(stops_label(0.5), "+\u{00bd}");
    assert_eq!(stops_label(-0.75), "-\u{00be}");
    assert_eq!(stops_label(1.0), "+1");
    assert_eq!(stops_label(-1.25), "-1\u{00bc}");
    assert_eq!(
        stops_label(-16.0),
        "-16",
        "the far end of what a key reaches"
    );
    assert_eq!(stops_label(0.1), "+0.10");
}

/// A share of the picture is written so that a small one is still
/// something and a large one is not four digits: nothing at all for
/// none, a floor under the least that is not none, tenths while they
/// tell the reader something and whole percents once they do not.
#[test]
fn a_clipped_share_is_written_to_be_read_at_a_glance() {
    assert_eq!(share_words(0.0), None);
    assert_eq!(share_words(-1.0), None);
    assert_eq!(share_words(f32::NAN), None);
    assert_eq!(share_words(0.0001).as_deref(), Some("<0.1%"));
    assert_eq!(share_words(0.001).as_deref(), Some("0.1%"));
    assert_eq!(share_words(0.0234).as_deref(), Some("2.3%"));
    assert_eq!(share_words(0.1).as_deref(), Some("10%"));
    assert_eq!(share_words(0.5).as_deref(), Some("50%"));
    assert_eq!(share_words(1.0).as_deref(), Some("100%"));
}

/// The ends of an axis and the value under the pointer are written with
/// no more digits than they need: `1`, not `1.0000`.
#[test]
fn a_decimal_is_written_without_the_zeros_it_does_not_need() {
    assert_eq!(trimmed("1.000".into()), "1");
    assert_eq!(trimmed("0.500".into()), "0.5");
    assert_eq!(trimmed("0.059".into()), "0.059");
    assert_eq!(trimmed("0.000".into()), "0");
    assert_eq!(trimmed("-0.000".into()), "0");
    assert_eq!(trimmed("12".into()), "12");

    let graded = shown(DecodedImage::new(
        2,
        1,
        Samples::U8 {
            channels: Channels::Gray,
            data: vec![0, 255],
        },
        ColorSpace::SRGB,
        AlphaMode::Opaque,
    ));
    assert_eq!(axis_words(&graded, 1.0), "1");
    assert_eq!(axis_words(&graded, 0.0), "0");

    // Linear integer data reads back as the counts it was stored in.
    let counts = shown(DecodedImage::new(
        2,
        1,
        Samples::U16 {
            channels: Channels::Gray,
            data: vec![0, 4095],
        },
        ColorSpace::LINEAR_BT709,
        AlphaMode::Opaque,
    ));
    assert_eq!(axis_words(&counts, 4095.0 / 65535.0), "4095");
}

/// The panel is the whole of the side, and every section in it is laid out
/// at its own height and the column's width, whatever the side's height: on
/// a side shorter than the column the column runs on past it, to be
/// scrolled to, rather than any section being shrunk or left out.
#[test]
fn every_section_is_laid_out_at_its_own_height_whatever_the_room() {
    assert_eq!(side::WIDTH, PANEL_WIDTH);
    assert_eq!(
        PANEL_WIDTH,
        COLUMN_WIDTH + crate::ui::style::SCROLLBAR_GUTTER + 2.0 * PANEL_INSET,
        "the column, its scrollbar's gutter and the panel's insets"
    );
    let side = Rect::new(40.0, 30.0, side::WIDTH, 300.0);
    let whole = panel(side);
    assert_eq!(
        (whole.x, whole.y, whole.width, whole.height),
        (side.x, side.y, PANEL_WIDTH, side.height)
    );

    let current = gain_mapped();
    let column = Rect::new(50.0, 70.0, COLUMN_WIDTH, 240.0);
    let layout = section::layout(column, &current);
    let sections: Vec<Section> = layout.iter().map(|(section, _)| *section).collect();
    assert_eq!(sections, Section::ALL, "in the order the data flows");
    assert_eq!(layout[0].1.y, column.y, "the first opens the column");
    for (section, rect) in &layout {
        assert_eq!(rect.height, section.height(false), "{section:?}");
        assert_eq!((rect.x, rect.width), (column.x, COLUMN_WIDTH));
    }
    for pair in layout.windows(2) {
        assert_eq!(
            pair[1].1.y,
            pair[0].1.bottom() + SECTION_GAP,
            "{:?} under {:?}",
            pair[1].0,
            pair[0].0
        );
    }
    let bottom = layout.last().expect("sections").1.bottom();
    assert_eq!(bottom - column.y, section::height(&current));
    assert!(
        bottom - column.y > 2.0 * column.height,
        "the column runs on past a short side: {}",
        bottom - column.y
    );
    assert_eq!(
        MIN_HEIGHT,
        2.0 * PANEL_INSET + TOOLBAR_HEIGHT + HEADER_GAP + Section::Output.height(false),
        "the row of buttons and the head section are the least the panel needs"
    );
}

/// The gain map's section is there only for a picture that carries one.
#[test]
fn the_gain_maps_section_is_there_only_with_a_map() {
    let plain: Vec<Section> = laid_out(&picture(false), 600.0)
        .into_iter()
        .map(|(section, _)| section)
        .collect();
    assert_eq!(plain, [Section::Output, Section::Display, Section::Image]);
    let mapped = laid_out(&gain_mapped(), 600.0);
    assert!(
        mapped
            .iter()
            .any(|(section, _)| *section == Section::GainMap)
    );
}

/// While the gain map is shown in the picture's place, the Gain map section
/// is the stage the display starts from, and the Image section, which would
/// plot the same map again, is left out; it comes back with the picture.
#[test]
fn the_image_section_gives_way_to_a_shown_gain_map() {
    let mut current = gain_mapped();
    let sections = |current: &Current| -> Vec<Section> {
        laid_out(current, 600.0)
            .into_iter()
            .map(|(section, _)| section)
            .collect()
    };
    let showing = Showing::Auxiliary(Auxiliary::GainMap);
    assert!(current.show(showing, |picture| {
        picture.auxiliary(Auxiliary::GainMap).map(Face::new)
    }));
    assert_eq!(
        sections(&current),
        [Section::Output, Section::Display, Section::GainMap]
    );
    assert!(current.show(Showing::Picture, |_| None));
    assert_eq!(
        sections(&current),
        [
            Section::Output,
            Section::Display,
            Section::GainMap,
            Section::Image
        ]
    );
}

/// While the depth map is shown in the picture's place, nothing on screen
/// is lifted, and the Gain map section is left out; the Image section is
/// the depth map's own.
#[test]
fn the_gain_maps_section_gives_way_to_a_shown_depth_map() {
    let mut image = gain_mapped().image.as_ref().clone();
    image.depth = Some(std::sync::Arc::new(DepthMap {
        width: 2,
        height: 1,
        samples: Samples::U8 {
            channels: Channels::Gray,
            data: vec![0, 255],
        },
        scale: None,
    }));
    let mut current = shown(image);
    assert!(
        current.show(Showing::Auxiliary(Auxiliary::Depth), |picture| {
            picture.auxiliary(Auxiliary::Depth).map(Face::new)
        })
    );
    let sections: Vec<Section> = laid_out(&current, 600.0)
        .into_iter()
        .map(|(section, _)| section)
        .collect();
    assert_eq!(
        sections,
        [Section::Output, Section::Display, Section::Image]
    );
}

/// Every plot keeps a point to the bin, which is what the pointer reads it
/// by, and sits inside its section, clear of the header above it.
#[test]
fn every_plot_keeps_one_pixel_to_the_bin() {
    for gray in [true, false] {
        let current = if gray { picture(true) } else { gain_mapped() };
        for (section, rect) in laid_out(&current, 600.0) {
            let bars = section.plot(rect);
            assert_eq!(bars.width, BINS as f32, "{section:?}, gray {gray}");
            assert!(
                bars.y - PLOT_INSET >= section::header_line(rect).bottom(),
                "{section:?}"
            );
            assert!(bars.bottom() + PLOT_INSET <= rect.bottom(), "{section:?}");
        }
    }
}

/// The pointer reads a bin of a plot and nothing outside it — not the
/// section around it, and not the line its header is set on.
#[test]
fn only_the_plot_itself_answers_the_pointer() {
    let layout = laid_out(&picture(false), 600.0);
    let bars = Section::Output.plot(rect_of(&layout, Section::Output));

    assert_eq!(hovered_bin(bars, None), None, "no pointer, no mark");
    assert_eq!(hovered_bin(bars, Some([bars.x - 1.0, bars.y + 1.0])), None);
    assert_eq!(
        hovered_bin(bars, Some([bars.x + 1.0, bars.y - 1.0])),
        None,
        "the header is above the plot, not part of it"
    );
    assert_eq!(
        hovered_bin(bars, Some([bars.right(), bars.y + 1.0])),
        None,
        "half-open at the far edge, as every other hit test here is"
    );
    assert_eq!(hovered_bin(bars, Some([bars.x + 1.0, bars.bottom()])), None);
}

/// And it reads the bin the bar under it was drawn from, so that the rule
/// and the bar it stands on cannot part company.
#[test]
fn the_pointer_marks_the_bar_it_is_over() {
    let layout = laid_out(&picture(false), 600.0);
    let bars = Section::Image.plot(rect_of(&layout, Section::Image));
    let at = |x: f32| hovered_bin(bars, Some([bars.x + x, bars.y + 1.0]));

    assert_eq!(at(0.0), Some(0), "the first pixel of the plot is bin zero");
    assert_eq!(
        at(bars.width - 0.5),
        Some(BINS - 1),
        "and the last of it is the last bin"
    );
    assert_eq!(at(3.1), at(3.9), "one pixel of pointer, one bin");
    assert_ne!(at(3.1), at(4.1), "the next pixel is the next bin");

    // Every bin the pointer can name has a bar drawn inside the plot for
    // it to stand on, the two ends included.
    for bin in [0, 1, BINS / 2, BINS - 2, BINS - 1] {
        let across = bin_across(bin);
        assert!((0.0..=1.0).contains(&across), "bin {bin} at {across}");
    }
    assert_eq!(bin_across(0), 0.0);
    assert_eq!(bin_across(BINS - 1), 1.0);
}

/// The row of buttons at the head of the panel: the clipped pixels' toggle
/// at the start and the reset at the far end, both inside the row.
#[test]
fn the_row_of_buttons_heads_the_panel() {
    let row = Rect::new(10.0, 10.0, COLUMN_WIDTH, TOOLBAR_HEIGHT);
    let [(marks, at_start), (reset, at_end)] = toolbar(row);
    assert_eq!((marks, reset), (Control::Marks, Control::Reset));
    assert_eq!(at_start.x, row.x, "the marks at the start");
    assert_eq!(at_end.right(), row.right(), "the reset at the far end");
    for rect in [at_start, at_end] {
        assert_eq!((rect.y, rect.height), (row.y, row.height), "{rect:?}");
    }
}

/// A section's own button stands at the end of its header, and the
/// readout ends short of it.
#[test]
fn a_sections_button_ends_its_header() {
    let layout = laid_out(&gain_mapped(), 600.0);
    let rect = rect_of(&layout, Section::GainMap);
    let line = section::header_line(rect);
    let button = section::button(rect);
    assert_eq!(button.right(), line.right());
    assert_eq!((button.y, button.height), (line.y, line.height));
    assert!(section::readout_end(Section::GainMap, line) < button.x);
    assert!(
        line.bottom() <= Section::GainMap.plot(rect).y - PLOT_INSET,
        "the header is clear of the plot's ground"
    );
}

/// A section with no button of its own sets its readout flush with the
/// end of its header.
#[test]
fn a_buttonless_sections_readout_ends_its_header() {
    let layout = laid_out(&gain_mapped(), 600.0);
    for (section, rect) in layout {
        if !section.has_button() {
            let line = section::header_line(rect);
            assert_eq!(section::readout_end(section, line), line.right());
        }
    }
}

/// The Display plot is as tall as the Output's on every file; the false
/// colors a gray image has sit under its band and its row of stops, and
/// make the section taller by their own row rather than taking it off the
/// plot.
#[test]
fn the_false_colors_are_under_the_display_band_and_nowhere_else() {
    let layout = laid_out(&picture(true), 600.0);
    let rect = rect_of(&layout, Section::Display);
    let with = plot_area(rect);
    let color = rect_of(&laid_out(&picture(false), 600.0), Section::Display);
    assert_eq!(with.height, PLOT_HEIGHT);
    assert_eq!(plot_area(color).height, PLOT_HEIGHT);
    assert_eq!(
        Section::Output
            .plot(rect_of(&layout, Section::Output))
            .height,
        PLOT_HEIGHT
    );
    assert_eq!(rect.height, color.height + display::SWATCH_ROW);

    let last = swatch_button(with, Colormap::ALL.len() - 1);
    assert!(
        last.y >= ramp(with).bottom(),
        "the ramps sit under the band"
    );
    assert!(last.right() <= with.right() + 0.01, "{last:?}");
    assert!(
        swatch_button(with, 0).y >= ev_row(with, ramp(with).bottom()).bottom(),
        "the false colors are under the row of stops"
    );
    assert!(swatch_button(with, 0).bottom() <= Rows::new(rect).exposure.y);
}

/// The rows sit under the Display section's band, inside the section, and
/// the last of them ends it.
#[test]
fn the_rows_sit_under_the_display_band() {
    let layout = laid_out(&picture(false), 600.0);
    let rect = rect_of(&layout, Section::Display);
    let rows = Rows::new(rect);
    let inside = rect;
    let band = ramp(plot_area(rect));

    for (widget, button) in row_buttons(rect) {
        assert!(button.y >= band.bottom(), "{widget:?} clears the band");
        assert!(button.x >= inside.x + 56.0, "{widget:?} clears its word");
        assert!(
            button.right() <= inside.right() + 0.01,
            "{widget:?} {button:?}"
        );
    }
    assert_eq!(
        rows.bottom(),
        rect.bottom(),
        "the last row ends the section"
    );
    let labels: Vec<&str> = rows.labels().map(|(word, _)| word).collect();
    assert_eq!(labels, ["Exposure", "Window", "Curve"]);

    let widgets: Vec<Control> = row_buttons(rect).map(|(widget, _)| widget).collect();
    assert_eq!(widgets.len(), WINDOWS.len() + ToneMap::ALL.len());
    assert_eq!(rows.slider().x, rows.exposure.x);
    assert!(rows.slider().right() < rows.stops().x);
    assert_eq!(rows.stops().right(), inside.right());
    assert!(
        rows.slider().width - HANDLE_GRIP > 3.0 * 2.0 * SLIDER_STOPS / EV_STEP,
        "a quarter stop is wider than a few pixels"
    );
}

/// The handles stand on the band and reach past it, and the room that
/// takes the pointer is wider than the mark — a mark five pixels wide is
/// not something a hand lands on — and clear of the plot above and of the
/// row under it.
#[test]
fn a_handle_is_easier_to_take_hold_of_than_it_is_wide() {
    for gray in [true, false] {
        let layout = laid_out(&picture(gray), 600.0);
        let rect = rect_of(&layout, Section::Display);
        let plot = plot_area(rect);
        let band = ramp(plot);
        let at = band.x + 100.0;
        let grip = grip(band, at);
        assert!(grip.width > HANDLE_WIDTH);
        assert_eq!(grip.x + grip.width / 2.0, at, "centered on its value");
        assert!(grip.y < band.y && grip.bottom() > band.bottom());
        assert!(
            grip.y >= plot.bottom() + PLOT_INSET,
            "{grip:?} over {plot:?}"
        );
        assert!(grip.bottom() <= Rows::new(rect).exposure.y);
        if gray {
            assert!(grip.bottom() <= swatch_button(plot, 0).y);
        }
    }
}

/// A pointer on a pixel of the picture is traced down the column: every
/// section marks a bin and writes the pixel's value at its stage. A pointer
/// on one section's plot marks that section alone.
#[test]
fn a_pixel_is_traced_down_every_section() {
    let current = gain_mapped();
    let layout = laid_out(&current, 600.0);
    let binned = output::binned_for(&current, Headroom::None, 1.0);
    let lift = current.image.gain_map.as_ref().unwrap().lift_plot();
    let plots = Plots {
        output: &binned,
        lift: lift.as_ref(),
        image: &current.stored.plot,
        counts: input_counts(&current),
        image_counts: input_counts(&current),
        output_counts: plot::Counts::of(&binned.plot),
        lift_counts: plot::Counts::of(lift.as_ref().unwrap()),
    };

    let traced = marked(
        &current,
        &layout,
        None,
        Some([60, 0]),
        Headroom::None,
        &plots,
    );
    for section in [Section::Output, Section::Display, Section::GainMap] {
        let mark = traced
            .of(section)
            .unwrap_or_else(|| panic!("{section:?} marks the pixel"));
        assert!(mark.bin.is_some(), "{section:?}: {mark:?}");
        assert!(!mark.words.is_empty(), "{section:?}");
    }
    // The Image section marks it a channel at a time.
    let image = traced.image.as_ref().expect("the Image section marks it");
    let planes: Vec<Plane> = image.channels.iter().map(|channel| channel.plane).collect();
    assert_eq!(
        planes,
        [Plane::Red, Plane::Green, Plane::Blue, Plane::Luma],
        "red, green, blue, then the luminance"
    );
    assert!(image.channels.iter().all(|channel| channel.bin.is_some()));
    assert_eq!(
        traced.gain_map.as_ref().unwrap().words,
        "+2.0 stops",
        "the right half is lifted the whole of the lift"
    );

    // On the Image section's plot, only it marks anything, and what it
    // says is its bin's value.
    let bars = Section::Image.plot(rect_of(&layout, Section::Image));
    let traced = marked(
        &current,
        &layout,
        Some([bars.x + bars.width / 2.0, bars.y + 1.0]),
        Some([60, 0]),
        Headroom::None,
        &plots,
    );
    assert!(
        traced
            .image
            .as_ref()
            .is_some_and(|mark| mark.bin == Some(BINS / 2))
    );
    assert!(traced.output.is_none());
    assert!(traced.display.is_none());
    assert!(traced.gain_map.is_none());

    // Off the picture and off every plot, nothing is marked.
    let traced = marked(
        &current,
        &layout,
        None,
        Some([999, 0]),
        Headroom::None,
        &plots,
    );
    assert!(
        Section::ALL
            .iter()
            .all(|section| traced.of(*section).is_none())
    );
}

/// The Image section writes each channel in the units the file keeps it
/// in, as the bottom bar's Decimal readout does: counts for 8- and 16-bit
/// samples, the number for floats. Each channel's rule stands on its own
/// plane's bar, and a gray file has the one channel.
#[test]
fn the_image_section_writes_each_channel_as_the_file_keeps_it() {
    let words = |mark: &Mark| -> Vec<String> {
        mark.channels
            .iter()
            .map(|channel| channel.words.clone())
            .collect()
    };

    // The ramp's 61st pixel is code 240 in every channel.
    let color = picture(false);
    let mark = stored::pixel_mark(&color, &color.stored.plot, 60, 0).expect("on the picture");
    assert_eq!(words(&mark), ["240", "240", "240", "240"]);
    let planes = color.stored.plot.color.expect("three planes");
    for (index, channel) in mark.channels.iter().take(3).enumerate() {
        assert!(
            planes[index][channel.bin.expect("on the axis")] > 0,
            "{channel:?}"
        );
    }

    let gray = picture(true);
    let mark = stored::pixel_mark(&gray, &gray.stored.plot, 60, 0).expect("on the picture");
    assert_eq!(words(&mark), ["240"]);
    assert_eq!(mark.channels[0].plane, Plane::Luma);

    let float = shown(DecodedImage::new(
        2,
        1,
        Samples::F32 {
            channels: Channels::Rgb,
            data: vec![0.25, 0.5, 1.0, 0.0, 0.0, 0.0],
        },
        ColorSpace::LINEAR_BT709,
        AlphaMode::Opaque,
    ));
    let mark = stored::pixel_mark(&float, &float.stored.plot, 0, 0).expect("on the picture");
    assert_eq!(words(&mark), ["0.2500", "0.5000", "1.0000", "0.4830"]);
}

/// On an SDR screen the output stops at white: a stop of exposure piles
/// the top of a ramp into the last bin.
#[test]
fn the_output_piles_what_the_screen_clips_at_white() {
    let mut current = picture(true);
    let plain = output::binned_for(&current, Headroom::None, 1.0);
    assert_eq!(plain.room, 1.0);
    current.display.set_exposure(1.0);
    let pushed = output::binned_for(&current, Headroom::None, 1.0);
    let white = crate::image::Transfer::Srgb.to_encoded(1.0);
    let [_, above] = pushed.plot.clipped(0.0, white);
    assert!(above > 0.25, "{above}");
    assert!(pushed.plot.luma[BINS - 1] > plain.plot.luma[BINS - 1]);

    // A room nothing has said goes as far as the response does, in whole
    // stops above white.
    current.display.set_exposure(2.0);
    let unknown = output::binned_for(&current, Headroom::Above, f32::INFINITY);
    assert_eq!(unknown.room, 4.0);
}

/// A bar cut off at the top of its plot says what share of the picture it
/// holds, ahead of its value, with the pointer on it; a bar under the top
/// says only its value.
#[test]
fn a_bar_cut_off_says_its_share() {
    // Every code once, and as many pixels again at one of them.
    let data: Vec<u8> = (0..512u32)
        .map(|x| if x < 256 { x as u8 } else { 200 })
        .flat_map(|value| [value; 3])
        .collect();
    let current = shown(DecodedImage::new(
        512,
        1,
        Samples::U8 {
            channels: Channels::Rgb,
            data,
        },
        ColorSpace::SRGB,
        AlphaMode::Opaque,
    ));
    let layout = laid_out(&current, 600.0);
    let binned = output::binned_for(&current, Headroom::None, 1.0);
    let plots = Plots {
        output: &binned,
        lift: None,
        image: &current.stored.plot,
        counts: input_counts(&current),
        image_counts: input_counts(&current),
        output_counts: plot::Counts::of(&binned.plot),
        lift_counts: plot::Counts::of(&current.stored.plot),
    };
    let bars = Section::Image.plot(rect_of(&layout, Section::Image));
    let words_at = |value: u8| {
        let bin = current
            .stored
            .plot
            .bin_at(f32::from(value) / 255.0)
            .expect("on the axis");
        let x = bars.x + (bin as f32 + 0.5) / BINS as f32 * bars.width;
        marked(
            &current,
            &layout,
            Some([x, bars.y + 1.0]),
            None,
            Headroom::None,
            &plots,
        )
        .image
        .expect("marked")
        .words
    };
    assert_eq!(words_at(200), "50% · 200");
    assert_eq!(words_at(100), "100");
}

/// The input's scale holds still while the display is set, and the
/// Output's follows what goes out: a stop of exposure up spreads the
/// picture over more of the Output's axis, and its scale comes down with it
/// rather than leaving the bars short of the top.
#[test]
fn the_output_scale_follows_the_display_and_the_inputs_holds_still() {
    // A ramp of many values to a bin, as a photograph's are, rather than a
    // code to one, which a stretch would only spread into a comb.
    let data: Vec<u16> = (0..4096u32)
        .map(|x| (x * 16) as u16)
        .flat_map(|value| [value; 3])
        .collect();
    let mut current = shown(DecodedImage::new(
        4096,
        1,
        Samples::U16 {
            channels: Channels::Rgb,
            data,
        },
        ColorSpace::SRGB,
        AlphaMode::Opaque,
    ));
    let top = |counts: plot::Counts| 1.0 / counts.up(1, 1);
    let input = input_counts(&current);
    let output = top(plot::Counts::of(
        &output::binned_for(&current, Headroom::None, 1.0).plot,
    ));

    current.shown.display.set_exposure(1.0);
    assert_eq!(input_counts(&current), input);
    let brighter = top(plot::Counts::of(
        &output::binned_for(&current, Headroom::None, 1.0).plot,
    ));
    assert!(brighter < output, "{brighter} against {output}");
}

/// A gray image read through a ramp — a measurement, or a false color — is
/// binned along the ramp, so the middle of its window lands at the middle
/// of the Output's axis, where light at half of white lands three quarters
/// of the way along; the axis has no room past the ramp's end on any
/// surface; and the pixel's mark is its place along the ramp.
#[test]
fn the_output_runs_along_the_ramp_where_the_picture_is_read_through_one() {
    let mut measured = shown(DecodedImage::new(
        1024,
        1,
        Samples::F32 {
            channels: Channels::Gray,
            data: (0..1024).map(|x| x as f32 / 1023.0).collect(),
        },
        ColorSpace::LINEAR_BT709,
        AlphaMode::Opaque,
    ));
    measured.shown.display.set_displayed_bounds(0.0, 1.0);
    let heaviest = |binned: &output::Binned| {
        let luma = &binned.plot.luma;
        (0..luma.len()).max_by_key(|&bin| luma[bin]).unwrap()
    };
    let binned = output::binned_for(&measured, Headroom::Above, f32::INFINITY);
    assert_eq!(binned.room, 1.0, "a ramp has no room past its end");
    let mark = output::pixel_mark(&measured, Headroom::None, &binned, 511, 0).unwrap();
    assert_eq!(mark.words, "0.5");
    let bin = mark.bin.unwrap();
    assert!((bin as f32 / BINS as f32 - 0.5).abs() < 0.01, "{bin}");
    // Evenly spread along the ramp: no bin stands out.
    let luma = &binned.plot.luma;
    let (low, high) = (luma[BINS / 4] as f32, luma[3 * BINS / 4] as f32);
    assert!(
        (low - high).abs() <= 0.1 * low.max(high),
        "{low} against {high}"
    );

    // Light: the same value lands where the screen codes half of white.
    let graded = picture(true);
    let binned = output::binned_for(&graded, Headroom::None, 1.0);
    let mark = output::pixel_mark(&graded, Headroom::None, &binned, 47, 0).unwrap();
    let encoded = crate::image::Transfer::Srgb.to_encoded(0.5);
    let bin = mark.bin.unwrap();
    assert!((bin as f32 / BINS as f32 - encoded).abs() < 0.02, "{bin}");
    assert!(
        heaviest(&binned) > BINS / 2,
        "the ramp of codes piles toward white as light"
    );

    // And a false color on it is a ramp again.
    let mut graded = picture(true);
    graded.shown.display.set_colormap(Colormap::Viridis, true);
    let binned = output::binned_for(&graded, Headroom::None, 1.0);
    assert_eq!(binned.ramp, Some(Colormap::Viridis));
    let mark = output::pixel_mark(&graded, Headroom::None, &binned, 47, 0).unwrap();
    let bin = mark.bin.unwrap();
    assert!((bin as f32 / BINS as f32 - 0.5).abs() < 0.02, "{bin}");
}

/// Scene light stored as floats is plotted in stops, so that what the
/// display makes of it is plotted from bins fine enough to say: a Radiance
/// picture's few hundred stops of nothing above its subject would otherwise
/// put the whole window into two fine bins, and the Output's plot into a
/// staircase of two steps. Its Image section has a row of stops from 1 in
/// place of the values at its ends, and the window's black, at no light,
/// is at the floor.
#[test]
fn scene_light_is_plotted_in_stops() {
    let mut current = scene(40);
    assert_eq!(current.stats.axis, Axis::Stops);
    assert_eq!(
        (current.stats.plot.min, current.stats.plot.max),
        (-14.0, 8.0)
    );
    assert_eq!(current.image.referred, Referred::Scene);
    assert!(current.display.has_exposure(), "metered");
    // The window as metered: 0..1 of the file, a few stops of exposure on.
    let (black, white) = current.display.displayed_bounds();
    assert_eq!(black, 0.0);
    assert!(white > 0.0 && white < 1.0, "{white}");

    // What goes out is spread over the Output's axis, not stepped.
    let binned = output::binned_for(&current, Headroom::None, 1.0);
    let occupied = binned.plot.luma.iter().filter(|&&count| count > 0).count();
    assert!(occupied > BINS / 2, "{occupied} bins hold anything");

    // The pixel at 1 marks the Display and Image plots where 1 stands.
    let unity = 14 * 40;
    let mark = display::pixel_mark(&current, Headroom::None, unity, 0).expect("on the picture");
    let bin = mark.bin.expect("on the axis");
    assert!(
        (bin as f32 / BINS as f32 - 14.0 / 22.0).abs() < 0.01,
        "{bin}"
    );
    assert!(mark.words.starts_with("1  "), "{}", mark.words);
    let mark =
        stored::pixel_mark(&current, &current.stored.plot, unity, 0).expect("on the picture");
    assert_eq!(mark.channels[0].bin, Some(bin));
    assert!(stored::ticks(&current, &current.stored.plot).is_empty());
    assert_eq!(
        stored::stop_across(&current.stored.plot, 0),
        Some(14.0 / 22.0)
    );
    assert_eq!(stored::stop_across(&current.stored.plot, 8), Some(1.0));

    // A stop of exposure moves the Output's plot the same distance wherever
    // it is: the display is a shift along the axis.
    current.display.set_exposure(0.0);
    let plain = output::binned_for(&current, Headroom::None, 1.0);
    current.display.set_exposure(1.0);
    let pushed = output::binned_for(&current, Headroom::None, 1.0);
    assert!(pushed.plot.luma[BINS - 1] > plain.plot.luma[BINS - 1]);
}

/// The Display section stands under the word for how the file is read,
/// one per reading, so that what its rows do is said rather than inferred.
#[test]
fn the_display_section_names_the_reading_the_file_is_under() {
    use crate::image::Referred;
    let words: Vec<_> = [Referred::Display, Referred::Scene, Referred::Measured]
        .map(display::mode)
        .to_vec();
    assert_eq!(words, ["graded", "metered", "measurement"]);
    assert_eq!(display::mode(picture(true).image.referred), "graded");
}

/// Integer samples are plotted on the whole range they can hold, a raw's
/// from black to the sensor's saturation, however little of it a linear
/// file was measured over, and their axis is written in quarters of it;
/// floats, which have no top, on what they were measured over, and written
/// at its ends.
#[test]
fn the_image_axis_is_the_whole_range_of_integer_samples() {
    let linear = |samples| {
        shown(DecodedImage::new(
            2,
            1,
            samples,
            ColorSpace::LINEAR_BT709,
            AlphaMode::Opaque,
        ))
    };
    let counts = linear(Samples::U16 {
        channels: Channels::Gray,
        data: vec![16384, 32768],
    });
    let plot = stored::rebinned(&counts).expect("put on the whole range");
    assert_eq!((plot.min, plot.max), (0.0, 1.0));
    for value in [16384.0, 32768.0] {
        let bin = plot.bin_at(value / 65535.0).expect("on the axis");
        assert!(plot.luma[bin] > 0, "{value}");
    }
    let past = plot.bin_at(0.6).expect("on the axis");
    assert!(plot.luma[past..].iter().all(|&count| count == 0));
    let mark = stored::pixel_mark(&counts, &plot, 1, 0).expect("on the picture");
    assert_eq!(mark.channels[0].bin, plot.bin_at(32768.0 / 65535.0));
    let ticks: Vec<String> = stored::ticks(&counts, &plot)
        .into_iter()
        .map(|(_, words)| words)
        .collect();
    assert_eq!(ticks, ["0", "16384", "32768", "49152", "65535"]);

    let floats = linear(Samples::F32 {
        channels: Channels::Gray,
        data: vec![0.5, 4.0],
    });
    assert!(stored::rebinned(&floats).is_none());
    let ticks: Vec<String> = stored::ticks(&floats, &floats.stored.plot)
        .into_iter()
        .map(|(_, words)| words)
        .collect();
    assert_eq!(ticks, ["0.5", "4"]);

    assert!(
        stored::rebinned(&picture(false)).is_none(),
        "on a curve already"
    );
}
