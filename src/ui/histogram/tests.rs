use super::display::{Rows, plot_area, row_buttons, swatch_button};
use super::*;
use crate::image::auxiliary::{Auxiliary, Showing};
use crate::image::depth::DepthMap;
use crate::image::display::{Colormap, Headroom, ToneMap};
use crate::image::gain_map::{Data, GainMap, Lift, Profile};
use crate::image::stats::Axis;
use crate::image::{AlphaMode, Channels, ColorSpace, DecodedImage, Referred, Samples};
use crate::ui::Face;
use crate::ui::side;
use crate::ui::slider::{HANDLE_GRIP, HANDLE_WIDTH};

/// `image`, on screen.
fn shown(image: DecodedImage) -> Current {
    Current::of(image, "file")
}

/// The same ramp read as data: a measurement, windowed to what it holds.
fn measured(gray: bool) -> Current {
    let mut image = picture(gray).image.as_ref().clone();
    image.referred = Referred::Measured;
    shown(image)
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
        data: crate::image::gain_map::Data::Codes(vec![0, 255]),
        lift: Lift::Apple { headroom: 4.0 },
    }));
    shown(image)
}

/// The same color picture as a raw's development with its profile beside
/// it: a map of ratios its own size, lifting the left half three stops and
/// the right half one, and a curve taking a half to a quarter.
fn profiled() -> Current {
    let mut image = picture(false).image.as_ref().clone();
    let ratios = (0..64)
        .flat_map(|x| [if x < 32 { 8.0 } else { 2.0 }; 3])
        .map(half::f16::from_f32)
        .collect();
    image.gain_map = Some(std::sync::Arc::new(GainMap {
        width: 64,
        height: 1,
        channels: 3,
        data: Data::Ratios(ratios),
        lift: Lift::Profile(Profile {
            baseline: 4.12,
            table: None,
            curve: Some(std::sync::Arc::new(vec![
                [0.0, 0.0],
                [0.5, 0.25],
                [1.0, 1.0],
            ])),
            range: [1.0, 3.0],
            to_curve: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
        }),
    }));
    let mut current = shown(image);
    let map = current.image.gain_map.clone().unwrap();
    current.shown.lift = Some(std::sync::Arc::new(map.table(1.0)));
    current
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

/// A percentile is whole through the middle, to a tenth at either end
/// where a tail is what is looked for, and never 0 or 100 unless it is
/// exactly so.
#[test]
fn a_percentile_is_written_whole_but_at_the_tails() {
    assert_eq!(percentile_words(0.0), "0%");
    assert_eq!(percentile_words(0.0004), "<0.1%");
    assert_eq!(percentile_words(0.0042), "0.4%");
    assert_eq!(percentile_words(0.42), "42%");
    assert_eq!(percentile_words(0.995), "99.5%");
    assert_eq!(percentile_words(0.9999), ">99.9%");
    assert_eq!(percentile_words(1.0), "100%");
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
    let applying: Vec<Section> = Section::ALL
        .into_iter()
        .filter(|section| section.applies(&current))
        .collect();
    assert_eq!(sections, applying, "in the order the data flows");
    assert_eq!(
        sections,
        [
            Section::Output,
            Section::Display,
            Section::GainMap,
            Section::Image
        ],
        "every section a photograph's lift passes through"
    );
    assert_eq!(layout[0].1.y, column.y, "the first opens the column");
    let image = stored::Layout::of(&current);
    assert_eq!(
        image,
        stored::Layout {
            planes: 3,
            shared: true
        }
    );
    for (section, rect) in &layout {
        assert_eq!(
            rect.height,
            section.height(false, false, image, develop::Shape::NONE),
            "{section:?}"
        );
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
        2.0 * PANEL_INSET
            + TOOLBAR_HEIGHT
            + HEADER_GAP
            + Section::Output.height(false, false, stored::Layout::ONE, develop::Shape::NONE),
        "the row of buttons and the head section are the least the panel needs"
    );
}

/// The Image section has a plot to the stored channel: one for a gray
/// file, three for a color one, one under the other inside the section and
/// clear of each other's grounds, with one row of values under the last
/// where the channels share an axis and one under each where they do not.
#[test]
fn the_image_section_has_a_plot_to_the_channel() {
    for (current, planes, shared) in [
        (picture(true), 1, true),
        (picture(false), 3, true),
        (measured(false), 3, false),
    ] {
        let layout = stored::Layout::of(&current);
        assert_eq!(layout, stored::Layout { planes, shared });
        let rect = rect_of(&laid_out(&current, 600.0), Section::Image);
        let plots = stored::plots(rect, layout);
        assert_eq!(plots.len(), layout.plots());
        assert_eq!(layout.plots(), if shared { 1 } else { planes });
        let first = Section::Image.plot(rect);
        assert_eq!(
            (plots[0].x, plots[0].y, plots[0].width),
            (first.x, first.y, first.width)
        );
        assert_eq!(plots[0].height, layout.plot_height());
        for pair in plots.windows(2) {
            assert!(
                pair[1].y - pair[0].bottom() >= 3.0 * PLOT_INSET,
                "{:?} under {:?}",
                pair[1],
                pair[0]
            );
            if !shared {
                assert!(pair[1].y - pair[0].bottom() >= 3.0 * PLOT_INSET + EV_ROW);
            }
        }
        let last = plots.last().unwrap();
        assert!(
            last.bottom() + PLOT_INSET + EV_ROW <= rect.bottom(),
            "{last:?} in {rect:?}"
        );
        for (index, bars) in plots.iter().enumerate() {
            assert_eq!(bars.width, BINS as f32);
            assert_eq!(
                stored::hovered(rect, layout, Some([bars.x + 0.5, bars.y + 1.0])),
                Some((index, 0))
            );
        }
        assert_eq!(
            stored::hovered(rect, layout, Some([rect.x + 1.0, rect.y + 1.0])),
            None,
            "the header is no plot"
        );
    }
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

/// The row of buttons at the head of the panel: the pair that reads the
/// file as a photo or as data at the start, and at the far end the reset
/// with the clipped pixels' toggle just before it, all inside the row and
/// none over another.
#[test]
fn the_row_of_buttons_heads_the_panel() {
    let row = Rect::new(10.0, 10.0, COLUMN_WIDTH, TOOLBAR_HEIGHT);
    let [
        (photo, first),
        (data, second),
        (marks, third),
        (reset, at_end),
    ] = toolbar(row);
    assert_eq!(
        (photo, data, marks, reset),
        (
            Control::Reading(Reading::Photo),
            Control::Reading(Reading::Data),
            Control::Marks,
            Control::Reset
        )
    );
    assert_eq!(first.x, row.x, "the reading at the start");
    assert_eq!(at_end.right(), row.right(), "the reset at the far end");
    assert_eq!(third.right() + CELL_GAP, at_end.x, "the marks beside it");
    assert_eq!(first.width, second.width, "the pair reads as one switch");
    for pair in [(first, second), (second, third), (third, at_end)] {
        assert!(pair.0.right() < pair.1.x, "{pair:?}");
    }
    for rect in [first, second, third, at_end] {
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

/// A raw's profile has the Profile section in the Gain map section's place,
/// nearest the file; its square is the profile's switch, and its curve is
/// drawn under its lift's plot, inside the section.
#[test]
fn the_profile_section_takes_the_gain_maps_place_for_a_raw() {
    let current = profiled();
    let layout = laid_out(&current, 600.0);
    let sections: Vec<Section> = layout.iter().map(|(section, _)| *section).collect();
    assert_eq!(
        sections,
        [
            Section::Output,
            Section::Display,
            Section::Profile,
            Section::Image
        ]
    );
    assert!(!sections.contains(&Section::Profile) || !sections.contains(&Section::GainMap));
    assert!(
        !laid_out(&gain_mapped(), 600.0)
            .iter()
            .any(|(section, _)| *section == Section::Profile)
    );
    assert!(
        !laid_out(&picture(false), 600.0)
            .iter()
            .any(|(section, _)| *section == Section::Profile)
    );

    let rect = rect_of(&layout, Section::Profile);
    assert!(Section::Profile.has_button());
    assert_eq!(section::button(rect).right(), rect.right());
    let lift = Section::Profile.plot(rect);
    let (caption, curve) = section::curve_part(rect);
    assert!(
        caption.y > lift.bottom() + PLOT_INSET + EV_ROW,
        "clear of the lift's row"
    );
    assert!(
        curve.y - PLOT_INSET >= caption.bottom(),
        "the caption over the ground"
    );
    assert_eq!(
        (curve.x, curve.right()),
        (lift.x + section::SCALE_GUTTER, lift.right()),
        "set in for the scale"
    );
    let (caption, table) = section::table_part(rect);
    assert!(
        caption.y > curve.bottom() + PLOT_INSET + EV_ROW,
        "clear of the curve's row"
    );
    assert!(table.y - PLOT_INSET >= caption.bottom());
    assert_eq!((table.x, table.width), (curve.x, curve.width));
    let exposure = section::exposure_row(rect);
    assert!(
        exposure.y > table.bottom() + PLOT_INSET + EV_ROW,
        "clear of the table's row"
    );
    let trace = section::trace_row(rect);
    assert!(trace.y > exposure.bottom());
    assert!(
        (trace.bottom() - rect.bottom()).abs() < 1e-3,
        "the pixel's row ends the section"
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
    assert!(swatch_button(with, 0).bottom() <= Rows::new(rect, false).span().0);
}

/// The rows sit under the Display section's band, inside the section, and
/// the last of them ends it. A photo has the exposure and the curve; an
/// image read as data has the row of windows alone, and its section is
/// shorter by a row — so the column moves only between the two kinds of
/// file, never between two photos.
#[test]
fn the_rows_sit_under_the_display_band() {
    let photo = picture(false);
    let data = measured(false);
    assert!(!section::is_data(&photo) && section::is_data(&data));
    let photo_rect = rect_of(&laid_out(&photo, 600.0), Section::Display);
    let data_rect = rect_of(&laid_out(&data, 600.0), Section::Display);
    assert_eq!(
        data_rect.height - photo_rect.height,
        display::rows_height(true) - display::rows_height(false),
        "a row shorter"
    );

    for (current, rect) in [(&photo, photo_rect), (&data, data_rect)] {
        let is_data = section::is_data(current);
        let rows = Rows::new(rect, is_data);
        let inside = rect;
        let band = ramp(plot_area(rect));

        for (widget, button) in row_buttons(rect, is_data) {
            assert!(button.y >= band.bottom(), "{widget:?} clears the band");
            assert!(button.x >= inside.x + 56.0, "{widget:?} clears its word");
            assert!(
                button.right() <= inside.right() + 0.01,
                "{widget:?} {button:?}"
            );
        }
        assert_eq!(
            rows.span().1,
            rect.bottom(),
            "the last row ends the section"
        );
        let labels: Vec<&str> = rows.labels().map(|(word, _)| word).collect();
        let widgets: Vec<Control> = row_buttons(rect, is_data)
            .map(|(widget, _)| widget)
            .collect();
        if is_data {
            assert_eq!(labels, ["Window"]);
            assert_eq!(widgets.len(), WINDOWS.len());
            assert!(
                rows.slider().is_none() && rows.stops().is_none(),
                "data has no exposure"
            );
            continue;
        }
        assert_eq!(labels, ["Exposure", "Curve"]);
        assert_eq!(widgets.len(), ToneMap::ALL.len());
        assert!(
            !widgets.iter().any(|w| matches!(w, Control::Window(_))),
            "a photo's window is as stored and the handles'"
        );
        let (slider, stops) = (rows.slider().unwrap(), rows.stops().unwrap());
        assert_eq!(slider.x, rows.exposure.unwrap().0.x);
        assert!(slider.right() < stops.x);
        assert_eq!(stops.right(), inside.right());
        assert!(
            slider.width - HANDLE_GRIP > 3.0 * 2.0 * SLIDER_STOPS / EV_STEP,
            "a quarter stop is wider than a few pixels"
        );
    }
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
        assert!(grip.bottom() <= Rows::new(rect, false).span().0);
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
    let image = stored::plotted_for(&stored::Source::image(&current));
    let plots = Plots {
        output: &binned,
        lift: lift.as_ref(),
        image: &image,
        develop: None,
        counts: input_counts(&current),
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
        [Plane::Red, Plane::Green, Plane::Blue],
        "red, green and blue, as the file stores them, and nothing it does not"
    );
    assert!(image.channels.iter().all(|channel| channel.bin.is_some()));
    assert_eq!(
        traced.gain_map.as_ref().unwrap().words,
        "+2.0 stops",
        "the right half is lifted the whole of the lift"
    );

    // A profile's lift is traced as a gain map's is: the right half is
    // lifted one stop.
    let current = profiled();
    let layout = laid_out(&current, 600.0);
    let lift = current.image.gain_map.as_ref().unwrap().lift_plot();
    let plots = Plots {
        lift: lift.as_ref(),
        lift_counts: plot::Counts::of(lift.as_ref().unwrap()),
        ..plots
    };
    let profile = marked(
        &current,
        &layout,
        None,
        Some([60, 0]),
        Headroom::None,
        &plots,
    );
    let mark = profile
        .profile
        .as_ref()
        .expect("the Profile section marks it");
    assert_eq!(mark.words, "+1.0 / +1.0 / +1.0 stops");
    assert!(mark.bin.is_some());

    // On one of the Image section's plots, only it marks anything, and
    // what it says is its bin's value on that plane.
    let rect = rect_of(&layout, Section::Image);
    let bars = stored::plots(rect, stored::Layout::of(&current))[0];
    let traced = marked(
        &current,
        &layout,
        Some([bars.x + bars.width / 2.0, bars.y + 1.0]),
        Some([60, 0]),
        Headroom::None,
        &plots,
    );
    let mark = traced.image.as_ref().expect("the plot marks its bin");
    assert_eq!(mark.bin, Some(BINS / 2));
    // The one value, with where it stands among each plane stacked beside
    // the one rule.
    assert_eq!(mark.channels.len(), 3);
    assert_eq!(mark.channels[1].plane, Plane::Green);
    assert!(mark.channels.iter().all(|channel| channel.bin.is_none()));
    assert!(mark.channels.iter().all(|channel| channel.rank.is_some()));
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
    let mark = stored::pixel_mark(&color, 60, 0).expect("on the picture");
    assert_eq!(words(&mark), ["240", "240", "240"]);
    let plotted = stored::plotted_for(&stored::Source::image(&color));
    assert_eq!(plotted.planes.len(), 3);
    for (index, channel) in mark.channels.iter().enumerate() {
        assert!(
            plotted.planes[index][channel.bin.expect("on the axis")] > 0,
            "{channel:?}"
        );
    }

    let gray = picture(true);
    let mark = stored::pixel_mark(&gray, 60, 0).expect("on the picture");
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
    let mark = stored::pixel_mark(&float, 0, 0).expect("on the picture");
    assert_eq!(words(&mark), ["0.2500", "0.5000", "1.0000"]);
}

/// The display's response to a gray never falls, whatever the window, the
/// exposure, the curve and the surface: what lets the Output section rank
/// a value by searching the tally rather than walking every bin of it
/// through the response.
#[test]
fn the_response_to_a_gray_never_falls() {
    let values: Vec<f32> = (-400..=600)
        .map(|step| step as f32 / 100.0)
        .map(|stops| {
            if stops < 0.0 {
                -(-stops).exp2() + 1.0
            } else {
                stops.exp2() - 1.0
            }
        })
        .collect();
    for gray in [true, false] {
        for tone_map in ToneMap::ALL {
            for headroom in [Headroom::None, Headroom::Above] {
                for (low, high, stops) in [(0.0, 1.0, 0.0), (0.1, 0.4, 2.0), (-3.0, 20.0, -1.5)] {
                    let mut display = crate::image::display::Display::default();
                    display.set_window(low, high);
                    display.set_exposure(stops);
                    display.set_tone_map(tone_map, gray);
                    let mut last = f32::NEG_INFINITY;
                    for &value in &values {
                        let out = display.response(value, gray, headroom);
                        assert!(
                            out >= last,
                            "{tone_map:?} {headroom:?} [{low}, {high}] {stops:+}: \
                             {value} comes out {out}, under {last}"
                        );
                        last = out;
                    }
                }
            }
        }
    }
}

/// The Display section ranks the pixel's luminance and each of its
/// channels among that plane's pixels, the luminance first, each with a
/// rule on its own plane's plot; a pointer on the plot ranks the one value
/// it is on among every plane.
#[test]
fn the_display_section_ranks_each_plane_of_the_pixel() {
    let current = picture(false);
    let mark = display::pixel_mark(&current, Headroom::None, 10, 0).expect("on the picture");
    let planes: Vec<Plane> = mark.channels.iter().map(|channel| channel.plane).collect();
    assert_eq!(planes, [Plane::Luma, Plane::Red, Plane::Green, Plane::Blue]);
    for channel in &mark.channels {
        // The ramp's eleventh of sixty-four distinct values.
        let rank = channel.rank.expect("ranked");
        assert!((rank - 10.5 / 64.0).abs() < 1e-6, "{channel:?}");
        assert!(channel.bin.is_some(), "{channel:?}");
    }

    let gray = picture(true);
    let mark = display::pixel_mark(&gray, Headroom::None, 10, 0).expect("on the picture");
    assert_eq!(mark.channels.len(), 1);
    assert_eq!(mark.channels[0].rank, Some(10.5 / 64.0));

    // The plot's ends: black, which the ramp's first pixel is at, and
    // white, which the ramp stops short of.
    for (bin, expected) in [(0, 0.5 / 64.0), (BINS - 1, 1.0)] {
        let mark = display::bin_mark(&current, Headroom::None, bin);
        assert_eq!(mark.channels.len(), 4);
        for channel in &mark.channels {
            assert_eq!(channel.bin, None);
            let rank = channel.rank.expect("ranked");
            assert!((rank - expected).abs() < 1e-6, "{bin}: {channel:?}");
        }
    }
}

/// The Output section ranks what each pixel comes out as: the pixels the
/// screen clips at white are one pile, and every one of them stands at the
/// pile's middle — lower than its own value stands in the Display
/// section — as a pointer on the pile's bar does.
#[test]
fn the_output_section_ranks_a_clipped_pile_at_its_middle() {
    let mut current = picture(true);
    current.display.set_exposure(1.0);
    let binned = output::binned_for(&current, Headroom::None, 1.0);
    let rank = |x: u32| {
        output::pixel_mark(&current, Headroom::None, &binned, x, 0)
            .expect("on the picture")
            .channels[0]
            .rank
            .expect("ranked")
    };
    let (top, clipped) = (rank(63), rank(55));
    assert_eq!(top, clipped);
    let shown = display::pixel_mark(&current, Headroom::None, 63, 0).expect("on the picture");
    assert!(top < shown.channels[0].rank.expect("ranked"));
    assert!(top < 1.0 && top > 0.5, "{top}");
    let pile = output::bin_mark(&current, Headroom::None, &binned, BINS - 1);
    assert_eq!(pile.channels[0].rank, Some(top));
    // Below the clip, a pixel stands where it does before the response.
    let dark =
        output::pixel_mark(&current, Headroom::None, &binned, 10, 0).expect("on the picture");
    assert_eq!(dark.channels[0].rank, Some(10.5 / 64.0));
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

/// The Image section says only the value of the bar the pointer is on,
/// even one cut off at the top of its plot, and ranks it among the
/// channel's pixels for the words beside the rule: those below it and half
/// of those at it.
#[test]
fn a_bar_on_the_image_plot_says_its_value_and_ranks_it() {
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
    let image = stored::plotted_for(&stored::Source::image(&current));
    let plots = Plots {
        output: &binned,
        lift: None,
        image: &image,
        develop: None,
        counts: input_counts(&current),
        output_counts: plot::Counts::of(&binned.plot),
        lift_counts: input_counts(&current),
    };
    let bars = Section::Image.plot(rect_of(&layout, Section::Image));
    let mark_at = |value: u8| {
        let bin = current.stored.planes[0]
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
    };
    let spike = mark_at(200);
    assert_eq!(spike.words, "200");
    assert_eq!(spike.channels[0].rank, Some((200.0 + 257.0 / 2.0) / 512.0));
    let lone = mark_at(100);
    assert_eq!(lone.words, "100");
    assert_eq!(lone.channels[0].rank, Some(100.5 / 512.0));
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
    let mark = stored::pixel_mark(&current, unity, 0).expect("on the picture");
    assert_eq!(mark.channels[0].bin, Some(bin));
    let plane = &current.stored.planes[0];
    assert_eq!(current.stored.axis, Axis::Stops);
    assert_eq!(plane.range, [-14.0, 8.0]);
    assert!(stored::ticks(&stored::Source::image(&current), plane).is_empty());
    assert_eq!(stored::stop_across(plane.range, 0), Some(14.0 / 22.0));
    assert_eq!(stored::stop_across(plane.range, 8), Some(1.0));

    // A stop of exposure moves the Output's plot the same distance wherever
    // it is: the display is a shift along the axis.
    current.display.set_exposure(0.0);
    let plain = output::binned_for(&current, Headroom::None, 1.0);
    current.display.set_exposure(1.0);
    let pushed = output::binned_for(&current, Headroom::None, 1.0);
    assert!(pushed.plot.luma[BINS - 1] > plain.plot.luma[BINS - 1]);
}

/// A linear integer photograph — a developed raw — is plotted in stops
/// below white, as linear floats are, with the row of stops under it; read
/// as data, the same samples are plotted over what they hold on a linear
/// axis, written at its ends; and floats, which have no top, over what they
/// were measured over.
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
    // Linear integer samples are read as data by default; as a photograph,
    // they are plotted over the whole range.
    let mut photo = DecodedImage::new(
        2,
        1,
        Samples::U16 {
            channels: Channels::Gray,
            data: vec![16384, 32768],
        },
        ColorSpace::LINEAR_BT709,
        AlphaMode::Opaque,
    );
    photo.referred = Referred::Display;
    let counts = shown(photo);
    let axis = counts.stored.axis;
    assert_eq!(axis, Axis::Stops);
    let plane = &counts.stored.planes[0];
    assert_eq!(plane.range[1], 0.0, "white at the top");
    assert!(plane.range[0] <= -2.0);
    let plot = plane.plot(axis);
    for value in [16384.0f32, 32768.0] {
        let bin = plane
            .bin_at(axis.place_stored(value / 65535.0))
            .expect("on the axis");
        assert!(plot[bin] > 0, "{value}");
    }
    let past = plane.bin_at(axis.place_stored(0.6)).expect("on the axis");
    assert!(plot[past..].iter().all(|&count| count == 0));
    let mark = stored::pixel_mark(&counts, 1, 0).expect("on the picture");
    assert_eq!(
        mark.channels[0].bin,
        plane.bin_at(axis.place_stored(32768.0 / 65535.0))
    );
    assert_eq!(mark.channels[0].words, "32768", "the file's own count");
    assert!(
        stored::ticks(&stored::Source::image(&counts), plane).is_empty(),
        "a row of stops instead"
    );
    // The Display's readout of the same value is in stops below white.
    assert_eq!(axis_words(&counts, axis.place_stored(0.5)), "\u{2212}1.0");
    assert_eq!(axis_words(&counts, f32::NEG_INFINITY), "0");

    // The same read as data: what it holds, written at its ends.
    let data = linear(Samples::U16 {
        channels: Channels::Gray,
        data: vec![16384, 32768],
    });
    assert_eq!(data.image.referred, Referred::Measured);
    let plane = &data.stored.planes[0];
    assert_eq!(plane.range, [16384.0 / 65535.0, 32768.0 / 65535.0]);
    let ticks: Vec<String> = stored::ticks(&stored::Source::image(&data), plane)
        .into_iter()
        .map(|(_, words)| words)
        .collect();
    assert_eq!(ticks, ["16384", "32768"]);

    let floats = linear(Samples::F32 {
        channels: Channels::Gray,
        data: vec![0.5, 4.0],
    });
    let plane = &floats.stored.planes[0];
    assert_eq!(plane.range, [0.5, 4.0]);
    let ticks: Vec<String> = stored::ticks(&stored::Source::image(&floats), plane)
        .into_iter()
        .map(|(_, words)| words)
        .collect();
    assert_eq!(ticks, ["0.5", "4"]);

    assert_eq!(
        picture(false).stored.shared_range(),
        Some([0.0, 1.0]),
        "on a curve already"
    );
}

/// The same color picture as the development of a mosaic: one count a
/// photosite under RGGB, `64 × x` across the one row, white at 4095 and
/// black at 64, balanced by two stops of red and a stop of blue, as the
/// camera was held.
fn developed() -> Current {
    use crate::image::sensor::{Development, Layout, Mosaic, Pattern, Sensor};
    let mut image = picture(false).image.as_ref().clone();
    let mosaic = Mosaic {
        width: 64,
        height: 1,
        counts: (0..64u16).map(|x| x * 64).collect(),
        pattern: Pattern::Bayer {
            filters: 0x9494_9494,
            planes: [0, 1, 2, 1],
        },
    };
    let stored = std::sync::Arc::new(mosaic.tally(4095.0));
    image.sensor = Some(std::sync::Arc::new(Sensor {
        layout: Layout::Mosaic(mosaic),
        turned: crate::image::orient::Orientation::NoTransforms,
        white: 4095.0,
        black: vec![64.0; 3],
        stored,
        development: Development {
            demosaic: crate::image::sensor::Demosaic::Ahd,
            balance: [4.0, 1.0, 2.0],
            kelvin: Some(5200.0),
        },
    }));
    shown(image)
}

/// The same as a raw stored three counts a pixel, worked back from the
/// developed pixel through the identity.
fn recovered() -> Current {
    use crate::image::sensor::{Development, Layout, Recovery, Sensor};
    let mut image = picture(false).image.as_ref().clone();
    let stored = std::sync::Arc::new(crate::image::stats::Stored::of_codes(
        vec![vec![1; 256]; 3],
        255.0,
    ));
    image.sensor = Some(std::sync::Arc::new(Sensor {
        layout: Layout::Rgb(Recovery {
            inverse: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            black: [0.0; 3],
            white: [255.0; 3],
            black_across: Vec::new(),
            black_down: Vec::new(),
            table: None,
        }),
        turned: crate::image::orient::Orientation::NoTransforms,
        white: 255.0,
        black: vec![0.0; 3],
        stored,
        development: Development {
            demosaic: crate::image::sensor::Demosaic::None,
            balance: [1.0; 3],
            kelvin: None,
        },
    }));
    shown(image)
}

/// A raw has a Develop section between the stages applied to the
/// developed picture and the Image section, which then plots the sensor's
/// counts: a plane to the color under the filter, over the whole of the
/// sensor's range with its levels on the axis. The Develop section's
/// plots are the developed channels, and for a mosaic it ends with the
/// photosites around the pointer.
#[test]
fn a_raw_has_a_develop_section_over_the_sensors_counts() {
    let current = developed();
    let layout = laid_out(&current, 600.0);
    let sections: Vec<Section> = layout.iter().map(|(section, _)| *section).collect();
    assert_eq!(
        sections,
        [
            Section::Output,
            Section::Display,
            Section::Develop,
            Section::Image
        ]
    );
    assert!(!Section::Develop.applies(&picture(false)));
    assert!(!Section::Develop.applies(&gain_mapped()));
    assert!(!Section::Develop.has_button());

    // The Image section is the sensor's: three planes, one axis, the
    // levels along it.
    let image = stored::Layout::of(&current);
    assert_eq!(
        image,
        stored::Layout {
            planes: 3,
            shared: true
        }
    );
    let source = stored::Source::image(&current);
    assert_eq!(source.section, Section::Image);
    assert_eq!(source.full, 4095.0);
    let ticks: Vec<String> = stored::ticks(&source, &source.stored.planes[0])
        .into_iter()
        .map(|(_, words)| words)
        .collect();
    assert_eq!(ticks, ["0", "64", "4095"]);
    let developed_source = stored::Source::developed(&current);
    assert_eq!(developed_source.section, Section::Develop);
    assert_eq!(developed_source.full, 255.0);

    // The Develop section: the developed channels' plots at the Image
    // section's heights, three rows, and the photosites.
    let shape = develop::Shape::of(&current);
    assert_eq!(
        shape,
        develop::Shape {
            layout: stored::Layout {
                planes: 3,
                shared: true
            },
            rows: 4,
            grid: true
        }
    );
    assert!(stored::Source::developed(&current).white_stops);
    assert!(!stored::Source::image(&current).white_stops);
    let rect = rect_of(&layout, Section::Develop);
    assert_eq!(
        rect.height,
        Section::Develop.height(false, false, image, shape)
    );
    let plots = stored::plots(rect, shape.layout);
    assert_eq!(plots.len(), 1, "the channels over one another");
    let first_row = develop::row(rect, shape, 0);
    assert!(first_row.y > plots[0].bottom());
    let (caption, ground) = develop::grid_part(rect, shape);
    assert!(caption.y > develop::row(rect, shape, 3).bottom());
    assert!(ground.y > caption.bottom());
    assert_eq!(ground.bottom() + PLOT_INSET, rect.bottom());
    // Without the mosaic, no photosites.
    let plain = develop::Shape::of(&recovered());
    assert!(!plain.grid);
    assert_eq!(plain.rows, 3, "no light where the file says nothing of it");
    assert!(plain.height() < shape.height());
    assert_eq!(develop::Shape::of(&picture(false)), develop::Shape::NONE);
}

/// The pointer's pixel is traced through the development: the Image
/// section marks the one photosite under it, its count in its plane's
/// cell and the other cells empty, and the Develop section marks the
/// developed pixel as the Image section marks a decoded one. A raw stored
/// three counts a pixel has all three worked back, approximate where the
/// development clamped a channel.
#[test]
fn a_raws_pixel_is_marked_on_the_sensor_and_as_developed() {
    let current = developed();
    // Pixel 5 is under green: the second column of RGGB.
    let mark = stored::image_mark(&current, 5, 0).expect("on the picture");
    assert_eq!(mark.channels.len(), 3);
    assert_eq!(
        mark.channels
            .iter()
            .map(|channel| channel.words.as_str())
            .collect::<Vec<_>>(),
        ["", "320", ""]
    );
    assert_eq!(mark.channels[1].plane, Plane::Green);
    assert!(mark.channels[1].bin.is_some());
    assert!(mark.channels[0].bin.is_none() && mark.channels[2].bin.is_none());
    assert!(mark.channels[1].rank.is_some());
    let red = stored::image_mark(&current, 4, 0).expect("on the picture");
    assert_eq!(red.channels[0].words, "256");
    assert_eq!(red.channels[0].plane, Plane::Red);
    // The developed pixel, in stops below white: 20 of 255.
    let developed = stored::developed_mark(&current, 5, 0).expect("on the picture");
    assert_eq!(
        developed
            .channels
            .iter()
            .map(|channel| channel.words.as_str())
            .collect::<Vec<_>>(),
        ["\u{2212}3.7", "\u{2212}3.7", "\u{2212}3.7"]
    );
    let black = stored::developed_mark(&current, 0, 0).expect("on the picture");
    assert_eq!(black.channels[0].words, "0");
    // Traced down the column: both sections mark the pixel at once.
    let layout = laid_out(&current, 600.0);
    let binned = output::binned_for(&current, Headroom::None, 1.0);
    let image = stored::plotted_for(&stored::Source::image(&current));
    let develop = stored::plotted_for(&stored::Source::developed(&current));
    let plots = Plots {
        output: &binned,
        lift: None,
        image: &image,
        develop: Some(&develop),
        counts: input_counts(&current),
        output_counts: plot::Counts::of(&binned.plot),
        lift_counts: input_counts(&current),
    };
    let traced = marked(
        &current,
        &layout,
        None,
        Some([5, 0]),
        Headroom::None,
        &plots,
    );
    assert_eq!(
        traced.develop.as_ref().map(|mark| mark.channels.len()),
        Some(3)
    );
    assert_eq!(traced.image.as_ref(), Some(&mark));
    // The pointer on the Develop plot marks its bin alone, in stops, with
    // every plane's rank beside the one rule.
    let rect = rect_of(&layout, Section::Develop);
    let bars = stored::plots(rect, develop::Shape::of(&current).layout)[0];
    let traced = marked(
        &current,
        &layout,
        Some([bars.x + bars.width / 2.0, bars.y + 1.0]),
        Some([5, 0]),
        Headroom::None,
        &plots,
    );
    assert!(traced.image.is_none());
    let hovered = traced.develop.expect("the plot under the pointer");
    assert_eq!(hovered.channels.len(), 3);
    assert_eq!(hovered.words, "\u{2212}1.0");

    // Three counts a pixel: the first pixel is black, which sits at the
    // clamp, and the middle one is clear of it.
    let recovered = recovered();
    let mark = stored::image_mark(&recovered, 0, 0).expect("on the picture");
    assert_eq!(mark.channels[0].words, "~0");
    let mark = stored::image_mark(&recovered, 32, 0).expect("on the picture");
    assert_eq!(mark.channels[1].words, "128");
}
