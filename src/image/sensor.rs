//! The sensor's own counts beside a developed raw: what the file holds
//! before anything is done to it, which the developed picture — demosaiced,
//! balanced, carried into Rec. 2020 — no longer is.
//!
//! A raw's `DecodedImage` is the developed picture, since that is what the
//! screen needs; this is what the histogram's Image section plots for it
//! instead, and what the pointer's readout at that stage says. One count a
//! photosite under a color filter, kept whole ([`Layout::Mosaic`]), or three
//! counts a pixel for a raw that is already demosaiced, as an iPhone's
//! ProRAW is, recovered from the developed pixel through the inverse of the
//! development rather than kept, since a second copy of a 48-megapixel
//! picture at sixteen bits is too much to hold ([`Layout::Rgb`]).
//!
//! The counts are tallied once as the picture is developed, a bin to the
//! count ([`Sensor::stored`]), so the Image section's plots cost nothing a
//! frame; the levels the file states stand on the axis as its ticks; and
//! what the developer did — the demosaic, the balance — is kept in words
//! and numbers for the Develop section.

use std::sync::Arc;

use super::color::ColorSpace;
use super::orient::{self, Orientation};
use super::stats::Stored;
use super::{AlphaMode, Channels, DecodedImage, Referred, Samples};

pub type Shared = Arc<Sensor>;

/// The sensor's counts behind a developed picture, and what was done to
/// them.
#[derive(Debug)]
pub struct Sensor {
    pub layout: Layout,
    /// How the developed picture was turned from the sensor's frame to the
    /// way the camera was held: a pixel of the developed picture is found
    /// on the sensor through it.
    pub turned: Orientation,
    /// The count the file states as the sensor's saturation, which is the
    /// counts' full scale.
    pub white: f32,
    /// Each plane's black level, in counts: what the development took off
    /// before anything else.
    pub black: Vec<f32>,
    /// Each plane tallied, a bin to the count, over `0..=white`.
    pub stored: Arc<Stored>,
    /// What the development did to the counts.
    pub development: Development,
}

/// How the counts are kept.
#[derive(Debug)]
pub enum Layout {
    /// One count a photosite under a color filter, in the sensor's own
    /// frame.
    Mosaic(Mosaic),
    /// Three counts a pixel, worked back from the developed pixel.
    Rgb(Recovery),
}

/// The mosaic: the counts as the sensor made them, `width` by `height`
/// in its own frame, with the pattern that says which color each
/// photosite is under.
#[derive(Debug)]
pub struct Mosaic {
    pub width: u32,
    pub height: u32,
    pub counts: Vec<u16>,
    pub pattern: Pattern,
}

/// Which plane each photosite is under.
#[derive(Debug, Clone)]
pub enum Pattern {
    /// No filter: every photosite is the one plane.
    Mono,
    /// A Bayer filter, as dcraw packs a 2-row-by-2-column pattern into
    /// `filters`: two bits a cell for the color's index, and `planes`
    /// which plane each color index is.
    Bayer { filters: u32, planes: [u8; 4] },
    /// Fujifilm's 6 by 6 pattern, each cell already the plane.
    XTrans([[u8; 6]; 6]),
}

impl Pattern {
    /// The plane the photosite at `(x, y)` of the sensor is under.
    pub fn plane(&self, x: u32, y: u32) -> usize {
        match self {
            Pattern::Mono => 0,
            Pattern::Bayer { filters, planes } => {
                let color = (filters >> ((((y << 1) & 14) | (x & 1)) << 1)) & 3;
                usize::from(planes[color as usize])
            }
            Pattern::XTrans(cells) => usize::from(cells[(y % 6) as usize][(x % 6) as usize]),
        }
    }

    /// How many planes the pattern has photosites under.
    pub fn planes(&self) -> usize {
        match self {
            Pattern::Mono => 1,
            Pattern::Bayer { planes, .. } => planes
                .iter()
                .map(|&plane| usize::from(plane) + 1)
                .max()
                .unwrap_or(1),
            Pattern::XTrans(cells) => cells
                .iter()
                .flatten()
                .map(|&plane| usize::from(plane) + 1)
                .max()
                .unwrap_or(1),
        }
    }
}

/// What works a developed pixel back to the counts it was made from, for a
/// raw stored three counts a pixel: the development undone step by step.
#[derive(Debug)]
pub struct Recovery {
    /// From the developed pixel, as fractions of white in linear Rec. 2020,
    /// back to the camera's channels as fractions of their white.
    pub inverse: [[f32; 3]; 3],
    /// Each channel's black and white in the linearized values.
    pub black: [f32; 3],
    pub white: [f32; 3],
    /// What a column's and a row's black differ from `black` by, in the
    /// sensor's frame.
    pub black_across: Vec<f32>,
    pub black_down: Vec<f32>,
    /// The linearization table the counts went through, where the file has
    /// one: a count indexes it.
    pub table: Option<Arc<Vec<f32>>>,
}

impl Recovery {
    /// The counts the developed pixel `developed` — fractions of white in
    /// Rec. 2020 — was made from, at `(x, y)` of the sensor's frame, and
    /// whether they are exact. They are not where any developed channel sits
    /// at the clamp, 0 or 1: what the matrix put past either end is gone,
    /// and every channel it was worked back from with it.
    pub fn counts(&self, developed: [f32; 3], x: u32, y: u32) -> ([f32; 3], bool) {
        let exact = developed.iter().all(|value| *value > 0.0 && *value < 1.0);
        let camera: [f32; 3] = std::array::from_fn(|row| {
            (0..3)
                .map(|column| self.inverse[row][column] * developed[column])
                .sum::<f32>()
                .clamp(0.0, 1.0)
        });
        let across = self.black_across.get(x as usize).copied().unwrap_or(0.0);
        let down = self.black_down.get(y as usize).copied().unwrap_or(0.0);
        let counts = std::array::from_fn(|c| {
            let black = self.black[c] + across + down;
            let linear = camera[c] * (self.white[c] - black) + black;
            match &self.table {
                // The first count the table takes to the value or past it.
                Some(table) => table.partition_point(|&value| value < linear) as f32,
                None => linear,
            }
            .round()
        });
        (counts, exact)
    }
}

/// What the developer did to the counts on the way to the picture.
#[derive(Debug, Clone, PartialEq)]
pub struct Development {
    /// The demosaic the mosaic went through.
    pub demosaic: Demosaic,
    /// What each plane was multiplied by to balance the picture, the green
    /// at 1: the camera's multipliers, or the daylight ones a file that
    /// records none is given. All 1 where the counts are balanced as
    /// stored.
    pub balance: [f32; 3],
    /// The correlated color temperature of the light the balance is for,
    /// in kelvin: what the camera took the scene to be lit by. `None` where
    /// the file says nothing it can be worked out from.
    pub kelvin: Option<f32>,
}

/// How the one count a photosite became three a pixel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Demosaic {
    /// Adaptive homogeneity-directed interpolation: each missing color is
    /// taken along the direction its neighborhood is most uniform in.
    /// dcraw's default, and what every other method is compared to.
    Ahd,
    /// None: the file stores three counts a pixel already.
    None,
}

impl Demosaic {
    /// The name the Develop section writes.
    pub fn label(self) -> &'static str {
        match self {
            Demosaic::Ahd => "AHD",
            Demosaic::None => "none",
        }
    }
}

impl Development {
    /// Whether the balance multiplied anything: within a thousandth of 1
    /// is 1.
    pub fn balanced_as_stored(&self) -> bool {
        self.balance
            .iter()
            .all(|multiplier| (multiplier - 1.0).abs() < 1e-3)
    }
}

/// The pointer's pixel as the sensor holds it: a count to the plane, or
/// `None` for a plane the photosite is not under, and whether the counts
/// are the file's own or worked back approximately.
#[derive(Debug, Clone, PartialEq)]
pub struct Reading {
    pub counts: Vec<Option<f32>>,
    pub exact: bool,
}

impl Sensor {
    /// How many planes the counts fall in.
    pub fn planes(&self) -> usize {
        self.stored.planes.len()
    }

    /// The sensor's size in its own frame.
    fn frame(&self, developed: [u32; 2]) -> [u32; 2] {
        match &self.layout {
            Layout::Mosaic(mosaic) => [mosaic.width, mosaic.height],
            // The developed picture turned back is the sensor's frame: a
            // quarter turn swaps the sides either way round, and anything
            // else leaves them.
            Layout::Rgb(_) => {
                let (width, height) = orient::size(developed[0], developed[1], self.turned);
                [width, height]
            }
        }
    }

    /// Where the pixel `(x, y)` of the developed picture, `developed` in
    /// size, is on the sensor.
    pub fn on_sensor(&self, [x, y]: [u32; 2], developed: [u32; 2]) -> Option<[u32; 2]> {
        let [width, height] = self.frame(developed);
        let turned = orient::size(width, height, self.turned);
        if x >= turned.0 || y >= turned.1 {
            return None;
        }
        let (sx, sy) = orient::stored(
            self.turned,
            x as usize,
            y as usize,
            width as usize,
            height as usize,
        );
        Some([sx as u32, sy as u32])
    }

    /// The counts under the pixel `(x, y)` of the developed picture, which
    /// is `developed` in size and holds `pixel` there, as fractions of
    /// white. `None` off the picture.
    pub fn at(&self, at: [u32; 2], developed: [u32; 2], pixel: &[f32]) -> Option<Reading> {
        let [sx, sy] = self.on_sensor(at, developed)?;
        match &self.layout {
            Layout::Mosaic(mosaic) => {
                let count = mosaic.count(sx, sy)?;
                let plane = mosaic.pattern.plane(sx, sy);
                let mut counts = vec![None; self.planes()];
                counts[plane] = Some(f32::from(count));
                Some(Reading {
                    counts,
                    exact: true,
                })
            }
            Layout::Rgb(recovery) => {
                let color = [0, 1, 2].map(|c| pixel.get(c).copied().unwrap_or(pixel[0]));
                let (counts, exact) = recovery.counts(color, sx, sy);
                Some(Reading {
                    counts: counts.into_iter().map(Some).collect(),
                    exact,
                })
            }
        }
    }

    /// The photosite under the pixel `(x, y)` of the developed picture:
    /// its plane and its count. `None` for a sensor with no mosaic, or a
    /// pixel off the picture.
    pub fn photosite(&self, at: [u32; 2], developed: [u32; 2]) -> Option<(usize, u16)> {
        let Layout::Mosaic(mosaic) = &self.layout else {
            return None;
        };
        let [sx, sy] = self.on_sensor(at, developed)?;
        Some((mosaic.pattern.plane(sx, sy), mosaic.count(sx, sy)?))
    }
}

impl Sensor {
    /// The mosaic as a picture of its own, turned as the developed picture
    /// is: each photosite lit in its own plane alone — red, green or blue —
    /// from its black to white, the others dark, so the filter's pattern
    /// shows as the sensor saw it, before any demosaic or balance. A
    /// monochrome sensor's is gray. `None` for a raw kept three counts a
    /// pixel, which has no mosaic.
    pub fn mosaic_image(&self) -> Option<DecodedImage> {
        let Layout::Mosaic(mosaic) = &self.layout else {
            return None;
        };
        let (width, height) = (mosaic.width as usize, mosaic.height as usize);
        let (turned_width, turned_height) = orient::size(mosaic.width, mosaic.height, self.turned);
        let mono = matches!(mosaic.pattern, Pattern::Mono);
        let channels = if mono { Channels::Gray } else { Channels::Rgb };
        let stride = if mono { 1 } else { 3 };
        // Each plane's count to a sixteen-bit code: its black at nothing,
        // white at full scale.
        let scales: Vec<(f32, f32)> = self
            .black
            .iter()
            .map(|&black| (black, 65535.0 / (self.white - black).max(1.0)))
            .collect();
        let mut data = vec![0u16; turned_width as usize * turned_height as usize * stride];
        for y in 0..turned_height as usize {
            for x in 0..turned_width as usize {
                let (sx, sy) = orient::stored(self.turned, x, y, width, height);
                let Some(&count) = mosaic.counts.get(sy * width + sx) else {
                    continue;
                };
                let plane = mosaic.pattern.plane(sx as u32, sy as u32);
                let (black, scale) = scales.get(plane).copied().unwrap_or((0.0, 1.0));
                let code = ((f32::from(count) - black) * scale)
                    .round()
                    .clamp(0.0, 65535.0) as u16;
                let at = (y * turned_width as usize + x) * stride;
                data[at + if mono { 0 } else { plane.min(2) }] = code;
            }
        }
        let mut image = DecodedImage::new(
            turned_width,
            turned_height,
            Samples::U16 { channels, data },
            ColorSpace::LINEAR_BT709,
            AlphaMode::Opaque,
        );
        image.referred = Referred::Measured;
        image.mosaic = !mono;
        Some(image)
    }
}

impl Mosaic {
    /// The count at `(x, y)` of the sensor, or `None` off it.
    pub fn count(&self, x: u32, y: u32) -> Option<u16> {
        if x >= self.width || y >= self.height {
            return None;
        }
        self.counts
            .get(y as usize * self.width as usize + x as usize)
            .copied()
    }

    /// Every count tallied into its plane, a bin to the count over
    /// `0..=white`: the Image section's plots. A count past `white` is
    /// counted at it.
    pub fn tally(&self, white: f32) -> Stored {
        let codes = white as usize + 1;
        let planes = self.pattern.planes();
        let mut counts = vec![vec![0u32; codes]; planes];
        for (index, &count) in self.counts.iter().enumerate() {
            let x = (index % self.width as usize) as u32;
            let y = (index / self.width as usize) as u32;
            let plane = self.pattern.plane(x, y);
            counts[plane][usize::from(count).min(codes - 1)] += 1;
        }
        Stored::of_codes(counts, white)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// dcraw's packing of RGGB.
    const RGGB: u32 = 0x9494_9494;

    fn rggb() -> Pattern {
        Pattern::Bayer {
            filters: RGGB,
            planes: [0, 1, 2, 1],
        }
    }

    #[test]
    fn a_bayer_pattern_names_each_photosites_plane() {
        let pattern = rggb();
        assert_eq!(pattern.plane(0, 0), 0, "red first");
        assert_eq!(pattern.plane(1, 0), 1);
        assert_eq!(pattern.plane(0, 1), 1);
        assert_eq!(pattern.plane(1, 1), 2);
        assert_eq!(pattern.plane(2, 2), 0, "and it repeats");
        assert_eq!(pattern.planes(), 3);
        assert_eq!(Pattern::Mono.planes(), 1);
    }

    /// The mosaic as a picture: turned as the developed picture is, each
    /// photosite in its own plane's channel alone, from its black to white.
    #[test]
    fn the_mosaic_is_shown_a_photosite_to_a_channel_and_turned() {
        let mosaic = Mosaic {
            width: 4,
            height: 2,
            counts: vec![15, 1015, 15, 4110, 15, 2015, 0, 15],
            pattern: rggb(),
        };
        let stored = Arc::new(mosaic.tally(4110.0));
        let sensor = Sensor {
            layout: Layout::Mosaic(mosaic),
            turned: orient::from_tag(Some(6)),
            white: 4110.0,
            black: vec![15.0; 3],
            stored,
            development: Development {
                demosaic: Demosaic::Ahd,
                balance: [1.0; 3],
                kelvin: None,
            },
        };
        let image = sensor.mosaic_image().expect("a mosaic");
        assert_eq!((image.width, image.height), (2, 4));
        let Samples::U16 { channels, data } = &image.samples else {
            panic!("sixteen bits");
        };
        assert_eq!(*channels, Channels::Rgb);
        let pixel = |x: usize, y: usize| {
            let at = (y * 2 + x) * 3;
            [data[at], data[at + 1], data[at + 2]]
        };
        // Counts above black, over the 4095 from black to white.
        let code = |above: f32| (above * 65535.0 / 4095.0).round() as u16;
        // Turned clockwise: the sensor's first photosite, red at its black,
        // is the picture's top right, and the green beside it below that.
        assert_eq!(pixel(1, 0), [0, 0, 0]);
        assert_eq!(pixel(1, 1), [0, code(1000.0), 0]);
        // The second row's second is blue; a count under black is nothing,
        // and one at white is full scale.
        assert_eq!(pixel(0, 1), [0, 0, code(2000.0)]);
        assert_eq!(pixel(0, 2), [0, 0, 0]);
        assert_eq!(pixel(1, 3), [0, 65535, 0]);
    }

    /// The mosaic's one value a pixel is its photosite's count, whichever
    /// plane it is under: a sensor clipped everywhere is at white in every
    /// pixel, where BT.709's weights would put its red at a fifth of it,
    /// its blue at a fourteenth and nothing above its green's 0.7152.
    #[test]
    fn the_mosaic_is_measured_on_its_photosites() {
        let mosaic = Mosaic {
            width: 2,
            height: 2,
            counts: vec![4110; 4],
            pattern: rggb(),
        };
        let stored = Arc::new(mosaic.tally(4110.0));
        let sensor = Sensor {
            layout: Layout::Mosaic(mosaic),
            turned: orient::from_tag(None),
            white: 4110.0,
            black: vec![15.0; 3],
            stored,
            development: Development {
                demosaic: Demosaic::Ahd,
                balance: [1.0; 3],
                kelvin: None,
            },
        };
        let image = sensor.mosaic_image().expect("a mosaic");
        assert!(image.mosaic);
        let stats = crate::image::stats::Stats::scan(&image);
        assert_eq!((stats.min, stats.max), (1.0, 1.0));
        assert_eq!(stats.percentile(0.5), 1.0);
        for (x, y) in [(0, 0), (1, 0), (1, 1)] {
            let sample = image.sample(x, y, None).expect("a pixel");
            assert_eq!(
                crate::image::stats::luminance_of(&image, &sample),
                1.0,
                "({x}, {y})"
            );
        }
    }

    /// A 4 by 2 mosaic, turned a quarter turn as the camera was held: the
    /// developed picture is 2 by 4, and each of its pixels is found on the
    /// sensor, with the window around one as the picture is shown.
    #[test]
    fn a_pixel_of_the_turned_picture_is_found_on_the_sensor() {
        let mosaic = Mosaic {
            width: 4,
            height: 2,
            counts: vec![10, 11, 12, 13, 20, 21, 22, 23],
            pattern: rggb(),
        };
        let stored = Arc::new(mosaic.tally(4095.0));
        let sensor = Sensor {
            layout: Layout::Mosaic(mosaic),
            turned: orient::from_tag(Some(6)),
            white: 4095.0,
            black: vec![0.0; 3],
            stored,
            development: Development {
                demosaic: Demosaic::Ahd,
                balance: [2.0, 1.0, 1.5],
                kelvin: Some(5200.0),
            },
        };
        assert_eq!(sensor.planes(), 3);
        // Turned clockwise: the first row down the right. The developed
        // picture's top-right pixel is the sensor's first.
        let reading = sensor
            .at([1, 0], [2, 4], &[0.0; 3])
            .expect("on the picture");
        assert_eq!(reading.counts, vec![Some(10.0), None, None]);
        assert!(reading.exact);
        let reading = sensor
            .at([0, 0], [2, 4], &[0.0; 3])
            .expect("on the picture");
        assert_eq!(
            reading.counts,
            vec![None, Some(20.0), None],
            "the second row's first is green"
        );
        assert!(sensor.at([2, 0], [2, 4], &[0.0; 3]).is_none());
        assert_eq!(sensor.photosite([0, 1], [2, 4]), Some((2, 21)));
        assert_eq!(sensor.photosite([1, 1], [2, 4]), Some((1, 11)));
        assert_eq!(sensor.photosite([2, 1], [2, 4]), None, "off the right");
        assert!(!sensor.development.balanced_as_stored());
    }

    /// The tally is a bin to the count, a plane to the color.
    #[test]
    fn the_mosaic_is_tallied_a_plane_to_the_color() {
        let mosaic = Mosaic {
            width: 2,
            height: 2,
            counts: vec![100, 200, 200, 4095],
            pattern: rggb(),
        };
        let stored = mosaic.tally(4095.0);
        assert_eq!(stored.planes.len(), 3);
        assert!(stored.shared);
        assert!((stored.planes[0].min - 100.0 / 4095.0).abs() < 1e-6);
        assert!((stored.planes[1].max - 200.0 / 4095.0).abs() < 1e-6);
        assert_eq!(
            stored.planes[2].rank(1.0),
            Some(0.5),
            "the one blue, at white"
        );
    }

    /// A developed pixel is worked back through the inverse of the
    /// matrix, the levels and the table to the counts it came from, and
    /// said to be approximate where a channel sits at the clamp.
    #[test]
    fn a_developed_pixel_is_worked_back_to_its_counts() {
        let recovery = Recovery {
            inverse: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            black: [64.0; 3],
            white: [1087.0; 3],
            black_across: vec![0.0, 1.0],
            black_down: vec![0.0],
            table: Some(Arc::new(
                (0..2048).map(|count| count as f32 * 0.5).collect(),
            )),
        };
        // Half way to white, under a table that halves each count: the
        // linear value is 64 + 0.5 * 1023, which the table reaches at twice
        // that count.
        let (counts, exact) = recovery.counts([0.5; 3], 0, 0);
        assert_eq!(counts, [1151.0; 3]);
        assert!(exact);
        let (counts, _) = recovery.counts([0.5; 3], 1, 0);
        assert_eq!(counts, [1152.0; 3], "the column's black moves the count");
        let (_, exact) = recovery.counts([1.0, 0.5, 0.5], 0, 0);
        assert!(!exact, "a channel at the clamp");
        let balanced = Development {
            demosaic: Demosaic::None,
            balance: [1.0, 1.0, 1.0004],
            kelvin: None,
        };
        assert_eq!(balanced.demosaic.label(), "none");
        assert!(balanced.balanced_as_stored());
    }
}
