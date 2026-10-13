//! The DNGs LibRaw cannot unpack, developed here: a picture stored as
//! Linear Raw — three values a pixel, demosaiced already — in tiles
//! compressed as JPEG XL, which is how an iPhone has written ProRAW since
//! DNG 1.7 gave the format that compression. LibRaw reads JPEG XL only
//! through Adobe's DNG SDK, which the system's library is not built with;
//! and there is little left to do once the tiles are decoded, since the
//! demosaic is the camera's own.
//!
//! What is done is what LibRaw does to a Linear Raw it can read, by the
//! specification's arithmetic: each tile decoded by `jxl-oxide`, each value
//! put through the linearization table, the black and white levels, and the
//! color matrix [`super::profile`] works out, into sixteen bits of linear
//! Rec. 2020 at the same 1.0 a developed raw has.
//!
//! The file's other instructions, about how it should *look*, are its
//! profile's: a baseline exposure, a gain table that is Apple's local tone
//! mapping, and a tone curve. Together they are what the camera's JPEG was
//! rendered from, and none of them alone is: the exposure lifts the whole
//! picture stops past white, and the table, indexed by how bright each
//! pixel then is, takes the highlights back down to where they were and
//! leaves the shadows lifted. They are not applied to the picture, which
//! stays as developed, but worked out beside it as the ratio each channel
//! of each pixel is lifted by — a gain map of [`Lift::Profile`], which the
//! display applies or not. They are worked out as the DNG SDK renders, in
//! linear ProPhoto: the exposure as a plain multiply (the SDK's ramp adds a
//! small toe at black, from a shadows setting the file does not carry), the
//! table's gain on every channel alike, and the curve on the largest and
//! smallest channels with the middle kept between them. A profile tag that
//! is not what the specification says is passed over with a warning, and
//! the picture develops without it.
//!
//! A file of this compression holding anything else — a mosaic, a picture
//! of other than three channels — is refused by name; where the file
//! carries the camera's JPEG, the loader shows that instead.

use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, anyhow, bail};
use half::f16;
use jxl_oxide::image::BitDepth;
use jxl_oxide::{AllocTracker, JxlImage, PixelFormat};

use super::profile::{self, Matrix, Profile};
use crate::image::gain_map::{self, Data, GainMap, GainTableFacts, Lift};
use crate::image::tiff::{self, Entry, Order};
use crate::image::{
    AlphaMode, Channels, ColorSpace, DecodedImage, Primaries, Referred, Samples, Transfer,
};

/// The tags read here, by the numbers the TIFF and DNG specifications
/// give them.
mod tag {
    pub const NEW_SUBFILE_TYPE: u16 = 254;
    pub const IMAGE_WIDTH: u16 = 256;
    pub const IMAGE_LENGTH: u16 = 257;
    pub const BITS_PER_SAMPLE: u16 = 258;
    pub const COMPRESSION: u16 = 259;
    pub const PHOTOMETRIC: u16 = 262;
    pub const STRIP_OFFSETS: u16 = 273;
    pub const SAMPLES_PER_PIXEL: u16 = 277;
    pub const ROWS_PER_STRIP: u16 = 278;
    pub const STRIP_BYTE_COUNTS: u16 = 279;
    pub const PLANAR_CONFIGURATION: u16 = 284;
    pub const TILE_WIDTH: u16 = 322;
    pub const TILE_LENGTH: u16 = 323;
    pub const TILE_OFFSETS: u16 = 324;
    pub const TILE_BYTE_COUNTS: u16 = 325;
    pub const DNG_VERSION: u16 = 50706;
    pub const LINEARIZATION_TABLE: u16 = 50712;
    pub const BLACK_LEVEL_REPEAT_DIM: u16 = 50713;
    pub const BLACK_LEVEL: u16 = 50714;
    pub const BLACK_LEVEL_DELTA_H: u16 = 50715;
    pub const BLACK_LEVEL_DELTA_V: u16 = 50716;
    pub const WHITE_LEVEL: u16 = 50717;
    pub const COLOR_MATRIX_1: u16 = 50721;
    pub const COLOR_MATRIX_2: u16 = 50722;
    pub const ANALOG_BALANCE: u16 = 50727;
    pub const AS_SHOT_NEUTRAL: u16 = 50728;
    pub const AS_SHOT_WHITE_XY: u16 = 50729;
    pub const CALIBRATION_ILLUMINANT_1: u16 = 50778;
    pub const CALIBRATION_ILLUMINANT_2: u16 = 50779;
    pub const FORWARD_MATRIX_1: u16 = 50964;
    pub const FORWARD_MATRIX_2: u16 = 50965;
    pub const BASELINE_EXPOSURE: u16 = 50730;
    pub const PROFILE_TONE_CURVE: u16 = 50940;
    /// DNG 1.6's gain table, which an iPhone writes in the picture's
    /// directory. DNG 1.7's `ProfileGainTableMap2`, 52544, adds a version,
    /// other sample types and a gamma, and is not read yet.
    pub const PROFILE_GAIN_TABLE_MAP: u16 = 52525;
}

/// The compression code DNG 1.7 gives JPEG XL.
const JPEG_XL: f64 = 52546.0;
/// The photometric interpretation of a picture demosaiced already.
const LINEAR_RAW: f64 = 34892.0;

/// The longest linearization table the specification allows: one entry for
/// every sixteen-bit value.
const TABLE_MAX: usize = 1 << 16;

/// A DNG whose picture is Linear Raw in JPEG XL, as its directories
/// describe it.
pub struct Linear {
    width: u32,
    height: u32,
    /// Each tile's size; a strip is a tile the width of the picture.
    tile: [u32; 2],
    /// Where each tile's codestream is in the file, row by row.
    tiles: Vec<std::ops::Range<usize>>,
    /// What each stored value means, where the file says.
    table: Option<Vec<f32>>,
    /// Each channel's black and white, in the linearized values.
    black: [f32; 3],
    white: [f32; 3],
    /// What a column's and a row's black differ from `black` by.
    black_across: Vec<f32>,
    black_down: Vec<f32>,
    /// From the camera's values, scaled to 0..1, to linear Rec. 2020.
    matrix: [[f32; 3]; 3],
    /// How the profile says the picture should look, where it says
    /// anything.
    rendering: Option<Rendering>,
}

/// A profile's rendering, as it is worked out pixel by pixel.
struct Rendering {
    /// `2^BaselineExposure`.
    exposure: f32,
    baseline: f32,
    table: Option<GainTable>,
    curve: Option<Arc<Vec<[f32; 2]>>>,
    /// Linear Rec. 2020 into linear ProPhoto, and back.
    into: [[f32; 3]; 3],
    back: [[f32; 3]; 3],
}

/// `ProfileGainTableMap`: a grid over the picture, and at each of its
/// points a table of gains by how bright a pixel is.
#[derive(Debug, PartialEq)]
struct GainTable {
    /// Points down and across.
    points: [usize; 2],
    /// The distance between points, down and across, as a share of the
    /// picture's height and width.
    spacing: [f64; 2],
    /// Where the first point is, down and across, in the same shares.
    origin: [f64; 2],
    /// Each point's table's length.
    steps: usize,
    /// What red, green, blue, the least of the three and the most of them
    /// are each weighted by in the brightness the table is indexed by.
    weights: [f32; 5],
    /// The tables, point by point across each row of the grid in turn.
    gains: Vec<f32>,
}

/// The tag's header: two counts, four doubles, a count and five floats.
const GAIN_TABLE_HEADER: usize = 64;

/// The most gains a gain table may hold: sixteen million, which is four
/// times what a grid at the picture's every eighth pixel would need of an
/// iPhone's tables.
const GAIN_TABLE_MAX: usize = 1 << 24;

impl GainTable {
    /// The tag's bytes, in `order`: the specification gives the tag no
    /// byte order of its own, and an iPhone writes it big-endian inside a
    /// little-endian file, so the caller tries both.
    fn parse(bytes: &[u8], order: Order) -> Result<Self> {
        let u32_at = |at: usize| {
            order
                .read_u32(bytes, at)
                .context("the gain table is cut short")
        };
        let f64_at = |at: usize| -> Result<f64> {
            let (first, second) = (u64::from(u32_at(at)?), u64::from(u32_at(at + 4)?));
            Ok(f64::from_bits(match order {
                Order::Little => first | second << 32,
                Order::Big => first << 32 | second,
            }))
        };
        let f32_at = |at: usize| -> Result<f32> { Ok(f32::from_bits(u32_at(at)?)) };
        let points = [u32_at(0)? as usize, u32_at(4)? as usize];
        let spacing = [f64_at(8)?, f64_at(16)?];
        let origin = [f64_at(24)?, f64_at(32)?];
        let steps = u32_at(40)? as usize;
        let weights: [f32; 5] = [
            f32_at(44)?,
            f32_at(48)?,
            f32_at(52)?,
            f32_at(56)?,
            f32_at(60)?,
        ];
        if points.contains(&0) || steps < 2 {
            bail!("a gain table of {points:?} points and {steps} steps");
        }
        let count = points[0]
            .checked_mul(points[1])
            .and_then(|count| count.checked_mul(steps))
            .filter(|count| *count <= GAIN_TABLE_MAX)
            .context("a gain table larger than any picture needs")?;
        if bytes.len() != GAIN_TABLE_HEADER + count * 4 {
            bail!(
                "a gain table of {count} gains in {} bytes",
                bytes.len() - GAIN_TABLE_HEADER.min(bytes.len())
            );
        }
        if !spacing
            .iter()
            .all(|value| value.is_finite() && *value > 0.0)
            || !origin.iter().all(|value| value.is_finite())
            || !weights.iter().all(|value| value.is_finite())
        {
            bail!("a gain table placed or weighted by numbers that are not");
        }
        let gains = (0..count)
            .map(|index| f32_at(GAIN_TABLE_HEADER + index * 4))
            .collect::<Result<Vec<_>>>()?;
        if !gains.iter().all(|gain| gain.is_finite() && *gain >= 0.0) {
            bail!("a gain table holding gains that are not");
        }
        Ok(Self {
            points,
            spacing,
            origin,
            steps,
            weights,
            gains,
        })
    }

    /// The table read in whichever order makes sense of it: big-endian
    /// first, as an iPhone writes it, then the file's own.
    fn read(bytes: &[u8], file: Order) -> Result<Self> {
        Self::parse(bytes, Order::Big).or_else(|error| match file {
            Order::Big => Err(error),
            Order::Little => Self::parse(bytes, Order::Little).map_err(|_| error),
        })
    }

    /// The gain for `color` at `[down, across]`, each a share of the
    /// picture's height and width: the table indexed by the color's
    /// weighted brightness, linear between its steps, and blended between
    /// the four points of the grid around the place, the edge's held past
    /// it. The weights' order — red, green, blue, the least, the most — and
    /// the index running to `steps` rather than one short of it are as the
    /// one open reading of the tag found has them; the specification was
    /// not to hand.
    fn gain_at(&self, color: [f32; 3], place: [f32; 2]) -> f32 {
        let [r, g, b] = color;
        let w = &self.weights;
        let input =
            (w[0] * r + w[1] * g + w[2] * b + w[3] * r.min(g).min(b) + w[4] * r.max(g).max(b))
                .clamp(0.0, 1.0);
        let position = (input * self.steps as f32).min((self.steps - 1) as f32);
        let step = (position as usize).min(self.steps - 2);
        let along = position - step as f32;
        let tap = |axis: usize| {
            let at = ((f64::from(place[axis]) - self.origin[axis]) / self.spacing[axis])
                .clamp(0.0, (self.points[axis] - 1) as f64);
            let near = (at as usize).min(self.points[axis] - 1);
            let far = (near + 1).min(self.points[axis] - 1);
            (near, far, (at - near as f64) as f32)
        };
        let (top, bottom, down) = tap(0);
        let (left, right, across) = tap(1);
        let point = |row: usize, column: usize| {
            let start = (row * self.points[1] + column) * self.steps + step;
            let pair = &self.gains[start..start + 2];
            pair[0] + (pair[1] - pair[0]) * along
        };
        let upper = point(top, left) + (point(top, right) - point(top, left)) * across;
        let lower = point(bottom, left) + (point(bottom, right) - point(bottom, left)) * across;
        upper + (lower - upper) * down
    }
}

impl Rendering {
    /// What the file's profile says, or `None` where it says nothing of
    /// how the picture should look. A tag that is not what the
    /// specification says is passed over, said on the terminal.
    fn read(first: &Directory, picture: &Directory) -> Result<Option<Self>> {
        let baseline = first.number(tag::BASELINE_EXPOSURE).unwrap_or(0.0) as f32;
        let baseline = if baseline.is_finite() && baseline.abs() <= 16.0 {
            baseline
        } else {
            warn(&format!("a baseline exposure of {baseline} EV"));
            0.0
        };
        let table = [picture, first]
            .iter()
            .find_map(|directory| {
                directory
                    .entries
                    .iter()
                    .find(|entry| entry.tag == tag::PROFILE_GAIN_TABLE_MAP)
                    .map(|entry| (directory, entry))
            })
            .and_then(|(directory, entry)| {
                let read = entry
                    .bytes(directory.bytes)
                    .context("the gain table runs past the end of the file")
                    .and_then(|bytes| GainTable::read(bytes, directory.order));
                read.map_err(|error| warn(&format!("{error:#}"))).ok()
            });
        let curve = first.numbers(tag::PROFILE_TONE_CURVE).and_then(|values| {
            let points: Vec<[f32; 2]> = values
                .as_chunks::<2>()
                .0
                .iter()
                .map(|[input, output]| [*input as f32, *output as f32])
                .collect();
            let rising = points.windows(2).all(|pair| pair[1][0] > pair[0][0]);
            let inside = points
                .iter()
                .flatten()
                .all(|value| value.is_finite() && (0.0..=1.0).contains(value));
            if values.len() % 2 == 0 && points.len() >= 2 && rising && inside {
                Some(Arc::new(points))
            } else {
                warn("a tone curve that does not run from black to white");
                None
            }
        });
        if baseline == 0.0 && table.is_none() && curve.is_none() {
            return Ok(None);
        }
        let (into, back) = profile::prophoto()?;
        Ok(Some(Self {
            exposure: baseline.exp2(),
            baseline,
            table,
            curve,
            into,
            back,
        }))
    }

    /// What the profile renders `developed`, linear Rec. 2020, as, at
    /// `place` — down and across, as shares of the picture.
    fn render(&self, developed: [f32; 3], place: [f32; 2]) -> [f32; 3] {
        let mut color = apply(self.into, developed).map(|value| value * self.exposure);
        if let Some(table) = &self.table {
            let gain = table.gain_at(color, place);
            color = color.map(|value| value * gain);
        }
        let toned = match &self.curve {
            Some(points) => {
                profile::rgb_tone(|value| gain_map::curve_through(points, value), color)
            }
            None => profile::rgb_tone(|value| value, color),
        };
        apply(self.back, toned)
    }

    /// What the information panel and the histogram say of it, with the
    /// least and the most it lifted any channel by.
    fn lift(&self, range: [f32; 2]) -> Lift {
        Lift::Profile(gain_map::Profile {
            baseline: self.baseline,
            table: self.table.as_ref().map(|table| GainTableFacts {
                points: [table.points[1] as u32, table.points[0] as u32],
                steps: table.steps as u32,
            }),
            curve: self.curve.clone(),
            range,
            to_curve: self.into,
        })
    }
}

fn apply(matrix: [[f32; 3]; 3], color: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|row| (0..3).map(|c| matrix[row][c] * color[c]).sum())
}

/// Says on the terminal that the profile's `what` is passed over.
fn warn(what: &str) {
    eprintln!(
        "{}: {}; the DNG's profile is applied without it",
        crate::PROGRAM,
        crate::escape_controls(what)
    );
}

/// The least and the most of what ratios were made, in stops.
type Range = Option<[f32; 2]>;

fn widen(range: Range, other: Range) -> Range {
    match (range, other) {
        (Some([a, b]), Some([c, d])) => Some([a.min(c), b.max(d)]),
        (range, None) | (None, range) => range,
    }
}

/// The ratio a gain map holds for `out` over `stored`, a sixteen-bit code:
/// one where nothing was stored, since nothing lifted is nothing; and kept
/// to what a half float holds with its precision, a stop either side of
/// anything a profile does.
fn ratio(out: f32, stored: u16) -> f32 {
    if stored == 0 {
        return 1.0;
    }
    (out / (f32::from(stored) / 65535.0)).clamp(RATIO_LEAST, RATIO_MOST)
}

const RATIO_LEAST: f32 = 1.0 / 16384.0;
const RATIO_MOST: f32 = 16384.0;

/// One directory's entries, with the block they point into.
struct Directory<'a> {
    bytes: &'a [u8],
    order: Order,
    entries: Vec<Entry>,
}

impl<'a> Directory<'a> {
    fn numbers(&self, tag: u16) -> Option<Vec<f64>> {
        self.entries
            .iter()
            .find(|entry| entry.tag == tag)?
            .numbers(self.bytes, self.order)
    }

    fn number(&self, tag: u16) -> Option<f64> {
        self.numbers(tag)?.first().copied()
    }

    /// A count or a size, which a negative, fractional or vast number is
    /// not.
    fn whole(&self, tag: u16) -> Result<Option<u32>> {
        self.number(tag).map(|value| whole(value, tag)).transpose()
    }

    fn matrix(&self, tag: u16) -> Result<Option<Matrix>> {
        let Some(values) = self.numbers(tag) else {
            return Ok(None);
        };
        // Three rows of three: a camera of other than three channels has
        // no matrix that lands in a picture of three.
        if values.len() != 9 {
            bail!("tag {tag} holds {} numbers, not a 3x3 matrix", values.len());
        }
        Ok(Some(std::array::from_fn(|row| {
            std::array::from_fn(|column| values[row * 3 + column])
        })))
    }

    fn triple(&self, tag: u16) -> Result<Option<[f64; 3]>> {
        let Some(values) = self.numbers(tag) else {
            return Ok(None);
        };
        match values[..] {
            [a, b, c] => Ok(Some([a, b, c])),
            _ => bail!("tag {tag} holds {} numbers, not three", values.len()),
        }
    }
}

fn whole(value: f64, tag: u16) -> Result<u32> {
    if value.fract() != 0.0 || !(0.0..=f64::from(u32::MAX)).contains(&value) {
        bail!("tag {tag} holds {value}, which is not a count");
    }
    Ok(value as u32)
}

impl Linear {
    /// The file in `bytes`, where it is a DNG whose picture is compressed
    /// with JPEG XL: `None` for any other file, which is LibRaw's to read,
    /// and an error for one of these this cannot develop.
    pub fn read(bytes: &[u8]) -> Result<Option<Self>> {
        let Some(order) = Order::of(bytes) else {
            return Ok(None);
        };
        let Some(entries) = tiff::entries(bytes, order, &[]) else {
            return Ok(None);
        };
        let first = Directory {
            bytes,
            order,
            entries,
        };
        if first.number(tag::DNG_VERSION).is_none() {
            return Ok(None);
        }
        let Some(picture) = picture(&first) else {
            return Ok(None);
        };
        if picture.number(tag::COMPRESSION) != Some(JPEG_XL) {
            return Ok(None);
        }
        let photometric = picture.number(tag::PHOTOMETRIC);
        if photometric != Some(LINEAR_RAW) {
            bail!(
                "the DNG's JPEG XL picture is not Linear Raw (photometric {}), which is all \
                 that can be developed here",
                photometric.map_or("missing".to_string(), |code| code.to_string())
            );
        }
        Self::of(&first, &picture).map(Some)
    }

    fn of(first: &Directory, picture: &Directory) -> Result<Self> {
        let width = picture
            .whole(tag::IMAGE_WIDTH)?
            .context("the DNG's picture has no width")?;
        let height = picture
            .whole(tag::IMAGE_LENGTH)?
            .context("the DNG's picture has no height")?;
        if width == 0 || height == 0 {
            bail!("the DNG's picture is {width}x{height}");
        }
        crate::image::decode::check_decoded_size(width, height, 3, 16)?;
        let samples = picture.whole(tag::SAMPLES_PER_PIXEL)?.unwrap_or(1);
        if samples != 3 {
            bail!("the DNG's Linear Raw has {samples} samples a pixel, not three");
        }
        if picture.whole(tag::PLANAR_CONFIGURATION)?.unwrap_or(1) != 1 {
            bail!("the DNG's Linear Raw is stored a plane at a time");
        }

        // Tiles, or strips: a strip is a tile as wide as the picture.
        let (tile, offsets, counts) = match picture.whole(tag::TILE_WIDTH)? {
            Some(across) => (
                [
                    across,
                    picture
                        .whole(tag::TILE_LENGTH)?
                        .context("the DNG's tiles have no height")?,
                ],
                tag::TILE_OFFSETS,
                tag::TILE_BYTE_COUNTS,
            ),
            None => (
                [
                    width,
                    picture
                        .whole(tag::ROWS_PER_STRIP)?
                        .unwrap_or(height)
                        .min(height),
                ],
                tag::STRIP_OFFSETS,
                tag::STRIP_BYTE_COUNTS,
            ),
        };
        if tile[0] == 0 || tile[1] == 0 {
            bail!("the DNG's tiles are {}x{}", tile[0], tile[1]);
        }
        // A tile is decoded whole, so it is held to the same ceiling as
        // the picture: one claiming to be vast is refused before the
        // decoder is asked for it.
        crate::image::decode::check_decoded_size(tile[0], tile[1], 3, 16)?;
        let expected = width.div_ceil(tile[0]) as usize * height.div_ceil(tile[1]) as usize;
        let offsets = picture.numbers(offsets).unwrap_or_default();
        let counts = picture.numbers(counts).unwrap_or_default();
        if offsets.len() != expected || counts.len() != expected {
            bail!(
                "the DNG's picture is {expected} tiles, and it says where {} of them are",
                offsets.len().min(counts.len())
            );
        }
        let tiles = offsets
            .iter()
            .zip(&counts)
            .map(|(&offset, &count)| {
                let start = whole(offset, tag::TILE_OFFSETS)? as usize;
                let end = start + whole(count, tag::TILE_BYTE_COUNTS)? as usize;
                if end > first.bytes.len() {
                    bail!("a tile of the DNG's picture runs past the end of the file");
                }
                Ok(start..end)
            })
            .collect::<Result<Vec<_>>>()?;

        let table = match picture.numbers(tag::LINEARIZATION_TABLE) {
            Some(table) if table.is_empty() || table.len() > TABLE_MAX => {
                bail!("the DNG's linearization table has {} entries", table.len())
            }
            Some(table) => Some(table.into_iter().map(|value| value as f32).collect()),
            None => None,
        };

        let repeat = picture
            .numbers(tag::BLACK_LEVEL_REPEAT_DIM)
            .unwrap_or_else(|| vec![1.0, 1.0]);
        if repeat != [1.0, 1.0] {
            bail!("the DNG's black level repeats over a pattern, which Linear Raw has none of");
        }
        let black = per_channel(picture.numbers(tag::BLACK_LEVEL), 0.0, "black level")?;
        let bits = picture
            .number(tag::BITS_PER_SAMPLE)
            .unwrap_or(16.0)
            .clamp(1.0, 32.0);
        let full = (2.0f64.powf(bits) - 1.0) as f32;
        let white = per_channel(picture.numbers(tag::WHITE_LEVEL), full, "white level")?;
        if (0..3).any(|c| white[c] <= black[c]) {
            bail!("the DNG's white level is not above its black level");
        }
        let delta = |tag: u16, length: u32| -> Result<Vec<f32>> {
            match picture.numbers(tag) {
                Some(values) if values.len() != length as usize => {
                    bail!(
                        "tag {tag} holds {} numbers, for {length} lines",
                        values.len()
                    )
                }
                Some(values) => Ok(values.into_iter().map(|value| value as f32).collect()),
                None => Ok(vec![0.0; length as usize]),
            }
        };
        let black_across = delta(tag::BLACK_LEVEL_DELTA_H, width)?;
        let black_down = delta(tag::BLACK_LEVEL_DELTA_V, height)?;

        let matrix = color(first)?.to_bt2020()?;
        let rendering = Rendering::read(first, picture)?;
        Ok(Self {
            width,
            height,
            tile,
            tiles,
            table,
            black,
            white,
            black_across,
            black_down,
            matrix,
            rendering,
        })
    }

    /// The picture, as stored: the turn the file asks for is the caller's.
    /// With `profile`, its profile's rendering beside it, where it has one,
    /// as the gain map the display lifts it by.
    pub fn develop(&self, bytes: &[u8], profile: bool) -> Result<DecodedImage> {
        let (width, height) = (self.width as usize, self.height as usize);
        let output = Mutex::new(vec![0u16; width * height * 3]);
        let rendering = self.rendering.as_ref().filter(|_| profile);
        let ratios = rendering.map(|_| Mutex::new(vec![f16::ONE; width * height * 3]));
        let range: Mutex<Range> = Mutex::new(None);
        let across = self.width.div_ceil(self.tile[0]) as usize;
        let mut failures: Vec<Option<anyhow::Error>> =
            (0..self.tiles.len()).map(|_| None).collect();
        rayon_core::scope(|scope| {
            for (index, failure) in failures.iter_mut().enumerate() {
                let (output, ratios, range) = (&output, ratios.as_ref(), &range);
                scope.spawn(move |_| {
                    let at = [index % across, index / across];
                    let lifted = rendering.zip(ratios);
                    match self.tile(bytes, index, at, output, lifted) {
                        Ok(found) => {
                            if let Ok(mut range) = range.lock() {
                                *range = widen(*range, found);
                            }
                        }
                        Err(error) => *failure = Some(error),
                    }
                });
            }
        });
        if let Some((index, error)) = failures
            .into_iter()
            .enumerate()
            .find_map(|(index, failure)| Some((index, failure?)))
        {
            return Err(error.context(format!("decoding tile {index} of the DNG's picture")));
        }
        let data = output
            .into_inner()
            .map_err(|_| anyhow!("a tile's thread panicked"))?;
        let mut image = DecodedImage::new(
            self.width,
            self.height,
            Samples::U16 {
                channels: Channels::Rgb,
                data,
            },
            ColorSpace {
                transfer: Transfer::Linear,
                primaries: Primaries::Bt2020,
            },
            AlphaMode::Opaque,
        );
        // As LibRaw's development is: linear, balanced, and 1.0 where the
        // sensor saturated.
        image.referred = Referred::Display;
        if let (Some(rendering), Some(ratios)) = (rendering, ratios) {
            let ratios = ratios
                .into_inner()
                .map_err(|_| anyhow!("a tile's thread panicked"))?;
            let range = range
                .into_inner()
                .map_err(|_| anyhow!("a tile's thread panicked"))?
                .unwrap_or([0.0; 2]);
            image.gain_map = Some(Arc::new(GainMap {
                width: self.width,
                height: self.height,
                channels: 3,
                data: Data::Ratios(ratios),
                lift: rendering.lift(range),
            }));
        }
        Ok(image)
    }

    /// Decodes tile `index`, the `at`th across and down, develops it in
    /// place, and copies the part of it inside the picture into `output`;
    /// with `lifted`, works out what the profile renders each pixel as, and
    /// copies the ratio into its map. Says the least and the most it lifted
    /// anything by, in stops.
    fn tile(
        &self,
        bytes: &[u8],
        index: usize,
        at: [usize; 2],
        output: &Mutex<Vec<u16>>,
        lifted: Option<(&Rendering, &Mutex<Vec<f16>>)>,
    ) -> Result<Range> {
        let [tile_width, tile_height] = self.tile.map(|side| side as usize);
        let mut values = decode_tile(&bytes[self.tiles[index].clone()], self.tile)?;
        let left = at[0] * tile_width;
        let top = at[1] * tile_height;
        // A tile at the right or the bottom is padded past the picture.
        let columns = tile_width.min(self.width as usize - left);
        let rows = tile_height.min(self.height as usize - top);
        let mut ratios = lifted.map(|_| vec![f16::ONE; columns * rows * 3]);
        let mut range: Range = None;
        for row in 0..rows {
            let y = top + row;
            for column in 0..columns {
                let x = left + column;
                let pixel = (row * tile_width + column) * 3;
                let camera: [f32; 3] = std::array::from_fn(|c| {
                    let stored = values[pixel + c];
                    let linear = match &self.table {
                        Some(table) => table[usize::from(stored).min(table.len() - 1)],
                        None => f32::from(stored),
                    };
                    let black = self.black[c] + self.black_across[x] + self.black_down[y];
                    ((linear - black) / (self.white[c] - black)).clamp(0.0, 1.0)
                });
                let developed = apply(self.matrix, camera).map(|value| value.clamp(0.0, 1.0));
                for (c, value) in developed.iter().enumerate() {
                    values[pixel + c] = (value * 65535.0 + 0.5) as u16;
                }
                if let (Some((rendering, _)), Some(ratios)) = (lifted, ratios.as_mut()) {
                    let place = [
                        (y as f32 + 0.5) / self.height as f32,
                        (x as f32 + 0.5) / self.width as f32,
                    ];
                    let out = rendering.render(developed, place);
                    let at = (row * columns + column) * 3;
                    for c in 0..3 {
                        let lift = ratio(out[c], values[pixel + c]);
                        ratios[at + c] = f16::from_f32(lift);
                        let stops = f16::from_f32(lift).to_f32().log2();
                        range = widen(range, Some([stops, stops]));
                    }
                }
            }
        }
        let width = self.width as usize;
        let mut output = output
            .lock()
            .map_err(|_| anyhow!("a tile's thread panicked"))?;
        for row in 0..rows {
            let from = row * tile_width * 3;
            let to = ((top + row) * width + left) * 3;
            output[to..to + columns * 3].copy_from_slice(&values[from..from + columns * 3]);
        }
        drop(output);
        if let (Some((_, map)), Some(ratios)) = (lifted, ratios) {
            let mut map = map
                .lock()
                .map_err(|_| anyhow!("a tile's thread panicked"))?;
            for row in 0..rows {
                let from = row * columns * 3;
                let to = ((top + row) * width + left) * 3;
                map[to..to + columns * 3].copy_from_slice(&ratios[from..from + columns * 3]);
            }
        }
        Ok(range)
    }
}

/// The tile's values as stored, three to a pixel, `tile` in size.
fn decode_tile(codestream: &[u8], tile: [u32; 2]) -> Result<Vec<u16>> {
    let tracker = AllocTracker::with_limit(crate::image::decode::MAX_DECODED_BYTES as usize);
    let image = JxlImage::builder()
        .alloc_tracker(tracker)
        .read(codestream)
        .map_err(|error| anyhow!("{error}"))?;
    if [image.width(), image.height()] != tile {
        bail!(
            "the tile is {}x{}, not the {}x{} the DNG says",
            image.width(),
            image.height(),
            tile[0],
            tile[1]
        );
    }
    if image.pixel_format() != PixelFormat::Rgb {
        bail!("the tile is {:?}, not three channels", image.pixel_format());
    }
    // An integer of at most sixteen bits, so that `u16` holds the values
    // as they were stored: `write_to_buffer` scales to the type's full
    // range from the depth the codestream states, which is then the
    // stored integer itself.
    match image.image_header().metadata.bit_depth {
        BitDepth::IntegerSample {
            bits_per_sample: 16,
        } => {}
        other => bail!("the tile's samples are {other:?}, not sixteen-bit integers"),
    }
    let render = image.render_frame(0).map_err(|error| anyhow!("{error}"))?;
    let mut stream = render.stream();
    let mut values = vec![0u16; tile[0] as usize * tile[1] as usize * 3];
    let written = stream.write_to_buffer(&mut values);
    if written != values.len() {
        bail!("the tile gave {written} values, not {}", values.len());
    }
    Ok(values)
}

/// The directory the picture itself is in: the first, or among its
/// SubIFDs, whichever is marked the full-size picture rather than a
/// preview of it.
fn picture<'a>(first: &Directory<'a>) -> Option<Directory<'a>> {
    let full_size =
        |directory: &Directory| directory.number(tag::NEW_SUBFILE_TYPE).unwrap_or(0.0) == 0.0;
    if full_size(first) {
        return Some(Directory {
            entries: tiff::entries(first.bytes, first.order, &[])?,
            ..*first
        });
    }
    first
        .numbers(tiff::tag::SUB_IFDS)?
        .into_iter()
        .filter_map(|offset| {
            let entries = tiff::entries_at(first.bytes, first.order, offset as usize, &[])?;
            Some(Directory { entries, ..*first })
        })
        .find(full_size)
}

/// What the first directory says about the camera's color.
fn color(first: &Directory) -> Result<Profile> {
    let light = |tag: u16| {
        first
            .number(tag)
            .and_then(|code| profile::illuminant(code as u16))
    };
    let mut profile = Profile::default();
    for (matrix, illuminant) in [
        (tag::COLOR_MATRIX_1, tag::CALIBRATION_ILLUMINANT_1),
        (tag::COLOR_MATRIX_2, tag::CALIBRATION_ILLUMINANT_2),
    ] {
        if let Some(matrix) = first.matrix(matrix)? {
            profile.color.push((matrix, light(illuminant)));
        }
    }
    for tag in [tag::FORWARD_MATRIX_1, tag::FORWARD_MATRIX_2] {
        if let Some(matrix) = first.matrix(tag)? {
            profile.forward.push(matrix);
        }
    }
    // A forward matrix stands beside a color matrix, one for one; a file
    // with them unpaired is read by its color matrices alone.
    if profile.forward.len() != profile.color.len() {
        profile.forward.clear();
    }
    profile.analog = first.triple(tag::ANALOG_BALANCE)?;
    profile.neutral = first.triple(tag::AS_SHOT_NEUTRAL)?;
    profile.white_xy = match first.numbers(tag::AS_SHOT_WHITE_XY).as_deref() {
        Some(&[x, y]) => Some([x, y]),
        _ => None,
    };
    Ok(profile)
}

/// A level given once for every channel or once for each, as its tag
/// allows; `missing` where the file gives none.
fn per_channel(values: Option<Vec<f64>>, missing: f32, name: &str) -> Result<[f32; 3]> {
    Ok(match values.as_deref() {
        None => [missing; 3],
        Some(&[all]) => [all as f32; 3],
        Some(&[a, b, c]) => [a as f32, b as f32, c as f32],
        Some(other) => bail!("the DNG's {name} holds {} numbers", other.len()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> Vec<u8> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("test_images")
            .join(name);
        std::fs::read(path).expect("the fixture is there")
    }

    /// Only a DNG whose picture is in JPEG XL is taken: a mosaiced DNG,
    /// and anything that is not a DNG, are LibRaw's.
    #[test]
    fn only_a_jpeg_xl_dng_is_taken() {
        assert!(Linear::read(&fixture("dng-jxl.dng")).unwrap().is_some());
        assert!(Linear::read(&fixture("dng-cfa.dng")).unwrap().is_none());
        assert!(Linear::read(&fixture("tiff-rgb8.tif")).unwrap().is_none());
        assert!(Linear::read(b"not a tiff").unwrap().is_none());
    }

    /// A JPEG XL picture that is a mosaic rather than Linear Raw is
    /// refused by name, rather than handed to LibRaw to fail at.
    #[test]
    fn a_mosaic_in_jpeg_xl_is_refused_by_name() {
        let mut bytes = fixture("dng-jxl.dng");
        // The SubIFD's PhotometricInterpretation, Linear Raw, made CFA.
        let linear_raw = [0x06, 0x01, 0x03, 0x00, 0x01, 0x00, 0x00, 0x00, 0x4c, 0x88];
        let at = bytes
            .windows(linear_raw.len())
            .position(|window| window == linear_raw)
            .expect("the fixture's picture is Linear Raw");
        bytes[at + 8..at + 10].copy_from_slice(&32803u16.to_le_bytes());
        let error = Linear::read(&bytes).err().expect("refused");
        assert!(format!("{error:#}").contains("not Linear Raw"), "{error:#}");
    }

    /// A gain table as the tag holds it, in `order`: a header and the
    /// gains, row by row of the grid, a table to a point.
    fn table_bytes(order: Order, points: [u32; 2], steps: u32, gains: &[f32]) -> Vec<u8> {
        let mut bytes = Vec::new();
        let u32s = |value: u32| match order {
            Order::Little => value.to_le_bytes(),
            Order::Big => value.to_be_bytes(),
        };
        let f64s = |value: f64| match order {
            Order::Little => value.to_le_bytes(),
            Order::Big => value.to_be_bytes(),
        };
        let f32s = |value: f32| u32s(value.to_bits());
        bytes.extend(u32s(points[0]));
        bytes.extend(u32s(points[1]));
        for value in [1.0, 1.0, 0.0, 0.0] {
            bytes.extend(f64s(value));
        }
        bytes.extend(u32s(steps));
        for weight in [1.0, 0.0, 0.0, 0.0, 0.0] {
            bytes.extend(f32s(weight));
        }
        for gain in gains {
            bytes.extend(f32s(*gain));
        }
        bytes
    }

    /// The tag has no byte order of its own: big-endian, as an iPhone
    /// writes it inside a little-endian file, is tried first, and the
    /// file's own order after.
    #[test]
    fn a_gain_table_is_read_in_either_byte_order() {
        let gains = [1.0, 2.0, 1.0, 2.0];
        for order in [Order::Big, Order::Little] {
            let table = GainTable::read(&table_bytes(order, [1, 2], 2, &gains), Order::Little)
                .expect("a table");
            assert_eq!(table.points, [1, 2]);
            assert_eq!(table.steps, 2);
            assert_eq!(table.gains, gains);
        }
        let mut cut = table_bytes(Order::Big, [1, 2], 2, &gains);
        cut.pop();
        assert!(GainTable::read(&cut, Order::Little).is_err());
        assert!(GainTable::read(&table_bytes(Order::Big, [0, 2], 2, &[]), Order::Big).is_err());
    }

    /// The table is read along its steps by the weighted brightness, and
    /// blended between the grid's points by the place, held at its edges.
    #[test]
    fn the_gain_table_is_sampled_along_its_steps_and_across_its_grid() {
        // One row of two points, three steps each: the left point's gains
        // rise 1, 2, 3 and the right point's are 4 throughout.
        let bytes = table_bytes(Order::Big, [1, 2], 3, &[1.0, 2.0, 3.0, 4.0, 4.0, 4.0]);
        let table = GainTable::read(&bytes, Order::Little).unwrap();
        let left = |red: f32| table.gain_at([red, 0.0, 0.0], [0.5, 0.0]);
        assert_eq!(left(0.0), 1.0);
        // A third of the way in is the second step.
        assert!((left(1.0 / 3.0) - 2.0).abs() < 1e-5);
        assert!((left(0.5) - 2.5).abs() < 1e-5);
        // The top is held at the last step.
        assert_eq!(left(1.0), 3.0);
        assert_eq!(left(4.0), 3.0, "clamped first");
        // Half way across is half way between the two points.
        let middle = table.gain_at([0.0; 3], [0.5, 0.5]);
        assert!((middle - 2.5).abs() < 1e-5, "{middle}");
        assert_eq!(table.gain_at([0.0; 3], [0.5, 1.0]), 4.0);
        assert_eq!(table.gain_at([0.0; 3], [0.5, 7.0]), 4.0, "past the edge");
    }

    /// The fixture's profile renders white two stops down; a primary is
    /// left alone in the channels it holds nothing of; and asked not to,
    /// the develop leaves the profile behind.
    #[test]
    fn the_fixtures_profile_bakes_the_ratios_it_states() {
        let bytes = fixture("dng-jxl.dng");
        let linear = Linear::read(&bytes).unwrap().unwrap();
        let image = linear.develop(&bytes, true).unwrap();
        let map = image.gain_map.as_deref().expect("a profile");
        let Lift::Profile(profile) = &map.lift else {
            panic!("a profile's lift");
        };
        assert_eq!(profile.baseline, -2.0);
        assert_eq!(
            profile.table,
            Some(GainTableFacts {
                points: [2, 2],
                steps: 2
            })
        );
        assert_eq!(profile.curve.as_deref().map(Vec::len), Some(3));
        let white = map.stops_at(24, 18, 32, 24);
        for stops in white {
            assert!((stops + 2.0).abs() < 0.01, "{white:?}");
        }
        let red = map.stops_at(8, 6, 32, 24);
        assert_eq!(red[1..], [0.0, 0.0], "{red:?}");
        assert!(profile.range[0] <= -2.0 && profile.range[1] >= -2.0);

        assert!(linear.develop(&bytes, false).unwrap().gain_map.is_none());
    }

    /// A profile tag that is not what the specification says never stops
    /// the picture developing: a cut gain table is passed over, and the
    /// rest of the profile still applies.
    #[test]
    fn a_profile_tag_never_refuses_the_develop() {
        let bytes = fixture("dng-jxl-profile-cut.dng");
        let linear = Linear::read(&bytes).unwrap().unwrap();
        let image = linear.develop(&bytes, true).unwrap();
        let Some(Lift::Profile(profile)) = image.gain_map.as_deref().map(|map| &map.lift) else {
            panic!("the rest of the profile");
        };
        assert_eq!(profile.table, None);
        assert_eq!(profile.baseline, -2.0);
    }
}
