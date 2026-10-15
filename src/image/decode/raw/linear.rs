//! A DNG stored as Linear Raw — three values a pixel, demosaiced already —
//! developed here rather than by LibRaw, so that its profile's rendering
//! and its stored codes come with it. An iPhone writes ProRAW this way: in
//! tiles compressed as JPEG XL since DNG 1.7 gave the format that
//! compression, or as lossless JPEG where the phone was asked for that.
//! LibRaw reads JPEG XL only through Adobe's DNG SDK, which the system's
//! library is not built with, so those tiles are decoded by `jxl-oxide`;
//! the lossless JPEG LibRaw could unpack, but on one thread, tile after
//! tile, which is most of the time a large picture takes, so those tiles
//! are decoded by [`super::ljpeg`] a thread to a tile as the JPEG XL ones
//! are. A Linear Raw of any other compression is unpacked by LibRaw, and
//! its values taken from the library's buffer. There is little left to do
//! any way, since the demosaic is the camera's own.
//!
//! What is done is what LibRaw does to a Linear Raw it develops, by the
//! specification's arithmetic: each value put through the linearization
//! table — which LibRaw has applied already where it unpacked — the black
//! and white levels, and the color matrix [`super::profile`] works out,
//! into sixteen bits of linear Rec. 2020 at the same 1.0 a developed raw
//! has.
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
//! A JPEG XL DNG holding anything else — a mosaic, a picture of other than
//! three channels — is refused by name; where the file carries the
//! camera's JPEG, the loader shows that instead. A Linear Raw of another
//! compression that is not three integer channels of at most sixteen bits
//! — a float DNG, a monochrome one — is left to LibRaw as every mosaic is.

use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, anyhow, bail};
use half::f16;
use half::vec::HalfBitsVecExt;
use jxl_oxide::image::BitDepth;
use jxl_oxide::{AllocTracker, JxlImage, PixelFormat};

use super::profile::{self, Matrix, Profile};
use crate::image::gain_map::{self, Data, GainMap, GainTable, Lift, Tap};
use crate::image::sensor::{self, Sensor};
use crate::image::stats::Stored;
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
    pub const SAMPLE_FORMAT: u16 = 339;
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

/// The compression code DNG 1.7 gives JPEG XL, and TIFF's for JPEG, which
/// in a DNG is lossless JPEG.
const JPEG_XL: f64 = 52546.0;
const LOSSLESS_JPEG: f64 = 7.0;
/// The photometric interpretation of a picture demosaiced already.
const LINEAR_RAW: f64 = 34892.0;
/// `SampleFormat`'s code for an unsigned integer, which is also what a
/// file that says nothing means.
const UNSIGNED: f64 = 1.0;

/// The longest linearization table the specification allows: one entry for
/// every sixteen-bit value.
const TABLE_MAX: usize = 1 << 16;

/// How many rows are developed at a time, a thread to each: a tile is
/// decoded whole, then developed in bands of this many rows, so that the
/// last tiles of a picture are not left to as many threads as there are
/// tiles while the rest of the pool waits, and a slow core's tile is
/// finished by the others. A picture LibRaw unpacked is developed in bands
/// the picture's width, one to a tile.
const BAND_ROWS: usize = 256;

/// Where a Linear Raw's values come from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// Tiles in JPEG XL, which LibRaw cannot unpack: decoded here, into the
    /// codes as the file stores them.
    JpegXl,
    /// Tiles in lossless JPEG, which LibRaw unpacks one after another on one
    /// thread: decoded here, a thread to a tile, into the codes as stored.
    LosslessJpeg,
    /// Any other compression — none, most likely — which LibRaw unpacks,
    /// putting each code through the linearization table as it does.
    LibRaw,
}

/// The picture's values, as [`Linear::develop`] is handed them, in the
/// form its [`Source`] says.
pub enum Values<'a> {
    /// The file itself, whose JPEG XL or lossless JPEG tiles are decoded
    /// here.
    File(&'a [u8]),
    /// The picture as LibRaw unpacked it: `stride` values a pixel, the
    /// first three a channel each, row after row at the picture's width —
    /// and through the linearization table already.
    Unpacked { values: &'a [u16], stride: usize },
}

/// A DNG whose picture is Linear Raw, as its directories describe it.
pub struct Linear {
    source: Source,
    width: u32,
    height: u32,
    /// Each tile's size; a strip is a tile the width of the picture, and a
    /// picture LibRaw unpacked is developed in bands, which are the same.
    tile: [u32; 2],
    /// Where each tile's codestream is in the file, row by row; nothing
    /// for a picture LibRaw unpacks.
    tiles: Vec<std::ops::Range<usize>>,
    /// What each stored value means, where the file says. Shared with the
    /// sensor's recovery of a developed pixel's counts.
    table: Option<Arc<Vec<f32>>>,
    /// Each channel's black and white, in the linearized values.
    black: [f32; 3],
    white: [f32; 3],
    /// What a column's and a row's black differ from `black` by.
    black_across: Vec<f32>,
    black_down: Vec<f32>,
    /// From the camera's values, scaled to 0..1, to linear Rec. 2020.
    matrix: [[f32; 3]; 3],
    /// `AsShotNeutral`: what the camera's channels read for white, which
    /// the matrix carries to white and the Develop section says the balance
    /// from; and the temperature of the light that neutral says the
    /// picture was taken in.
    neutral: Option<[f64; 3]>,
    kelvin: Option<f64>,
    /// How the profile says the picture should look, where it says
    /// anything.
    rendering: Option<Rendering>,
}

/// A profile's rendering, as it is worked out pixel by pixel.
struct Rendering {
    /// `2^BaselineExposure`.
    exposure: f32,
    baseline: f32,
    table: Option<Arc<GainTable>>,
    curve: Option<Arc<Vec<[f32; 2]>>>,
    /// Linear Rec. 2020 into linear ProPhoto, and back.
    into: [[f32; 3]; 3],
    back: [[f32; 3]; 3],
}

/// The tag's header: two counts, four doubles, a count and five floats.
const GAIN_TABLE_HEADER: usize = 64;

/// The most gains a gain table may hold: sixteen million, which is four
/// times what a grid at the picture's every eighth pixel would need of an
/// iPhone's tables.
const GAIN_TABLE_MAX: usize = 1 << 24;

/// Reading the tag, which is the developer's business; what the table
/// means is [`GainTable`]'s own.
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
                read.map_err(|error| warn(&format!("{error:#}")))
                    .ok()
                    .map(Arc::new)
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

    /// Where `share` of the picture's height (`axis` 0) or width (1) falls
    /// on the gain table's grid, for [`Rendering::render`]; nothing where
    /// there is no table.
    fn tap(&self, axis: usize, share: f32) -> Tap {
        self.table
            .as_ref()
            .map_or_else(Tap::default, |table| table.tap(axis, share))
    }

    /// What the profile renders `developed`, linear Rec. 2020, as, at the
    /// place `down` and `across` tap.
    fn render(&self, developed: [f32; 3], down: Tap, across: Tap) -> [f32; 3] {
        let mut color = apply(self.into, developed).map(|value| value * self.exposure);
        if let Some(table) = &self.table {
            let gain = table.gain_between(table.input(color), down, across);
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
            table: self.table.clone(),
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
    /// The file in `bytes`, where it is a DNG whose picture is developed
    /// here: `None` for any other file, which is LibRaw's to read, and an
    /// error for one of these this cannot develop. One compressed with JPEG
    /// XL is taken whatever it holds, since LibRaw cannot unpack it; any
    /// other is taken only where it is Linear Raw of three integer channels
    /// of at most sixteen bits — its lossless JPEG tiles decoded here, and
    /// any other compression unpacked by LibRaw into the form
    /// [`Values::Unpacked`] describes.
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
        let photometric = picture.number(tag::PHOTOMETRIC);
        let source = if picture.number(tag::COMPRESSION) == Some(JPEG_XL) {
            if photometric != Some(LINEAR_RAW) {
                bail!(
                    "the DNG's JPEG XL picture is not Linear Raw (photometric {}), which is all \
                     that can be developed here",
                    photometric.map_or("missing".to_string(), |code| code.to_string())
                );
            }
            Source::JpegXl
        } else {
            let samples = picture.number(tag::SAMPLES_PER_PIXEL).unwrap_or(1.0);
            let format = picture.number(tag::SAMPLE_FORMAT).unwrap_or(UNSIGNED);
            let bits = picture.number(tag::BITS_PER_SAMPLE).unwrap_or(16.0);
            if photometric != Some(LINEAR_RAW)
                || samples != 3.0
                || format != UNSIGNED
                || !(1.0..=16.0).contains(&bits)
            {
                return Ok(None);
            }
            if picture.number(tag::COMPRESSION) == Some(LOSSLESS_JPEG) {
                Source::LosslessJpeg
            } else {
                Source::LibRaw
            }
        };
        Self::of(source, &first, &picture).map(Some)
    }

    /// The picture's size, which what LibRaw unpacks has to be.
    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    pub fn source(&self) -> Source {
        self.source
    }

    fn of(source: Source, first: &Directory, picture: &Directory) -> Result<Self> {
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
        let (tile, tiles) = match source {
            Source::JpegXl | Source::LosslessJpeg => tiles(first, picture, width, height)?,
            // LibRaw hands the picture over whole, and it is developed in
            // bands of rows, a thread to a band.
            Source::LibRaw => ([width, (BAND_ROWS as u32).min(height)], Vec::new()),
        };

        let table = match picture.numbers(tag::LINEARIZATION_TABLE) {
            Some(table) if table.is_empty() || table.len() > TABLE_MAX => {
                bail!("the DNG's linearization table has {} entries", table.len())
            }
            Some(table) => Some(Arc::new(
                table.into_iter().map(|value| value as f32).collect(),
            )),
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

        let camera = color(first)?;
        let matrix = camera.to_bt2020()?;
        let rendering = Rendering::read(first, picture)?;
        Ok(Self {
            source,
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
            neutral: camera.neutral,
            kelvin: camera.kelvin(),
            rendering,
        })
    }

    /// The code the stored values run up to, which is the Image section's
    /// axis: the last entry of the linearization table where the file has
    /// one — an iPhone stores ten-bit codes through a table of 1024 — and
    /// the highest white level of the three channels otherwise.
    fn full(&self) -> f32 {
        match &self.table {
            Some(table) => (table.len() - 1) as f32,
            None => self.white.iter().copied().fold(1.0, f32::max),
        }
    }

    /// Each channel's black as a stored code: the first code the table
    /// takes to the black level or past it, or the level itself where there
    /// is no table.
    fn black_codes(&self) -> Vec<f32> {
        self.black
            .iter()
            .map(|&black| match &self.table {
                Some(table) => table.partition_point(|&value| value < black) as f32,
                None => black,
            })
            .collect()
    }

    /// The code each value LibRaw unpacked was stored as, for every value a
    /// sixteen-bit word holds: the first code the table takes to the value
    /// or past it, which is the one it was where the table only rises.
    /// Nothing where there is no table, the value being the code.
    fn codes_of_values(&self) -> Option<Vec<u16>> {
        let table = self.table.as_ref()?;
        let last = table.len() - 1;
        Some(
            (0..=u16::MAX)
                .map(|value| {
                    table
                        .partition_point(|&linear| linear < f32::from(value))
                        .min(last) as u16
                })
                .collect(),
        )
    }

    /// The picture, as stored: the turn the file asks for is the caller's,
    /// and `turned` says which it will be, for the sensor's counts beside
    /// the picture. With `profile`, its profile's rendering beside it, where
    /// it has one, as the gain map the display lifts it by. `values` is the
    /// picture in the form the source asks for.
    pub fn develop(
        &self,
        values: Values,
        profile: bool,
        turned: crate::image::orient::Orientation,
    ) -> Result<DecodedImage> {
        let (width, height) = (self.width as usize, self.height as usize);
        match (&values, self.source) {
            (Values::File(_), Source::JpegXl | Source::LosslessJpeg) => {}
            (Values::Unpacked { values, stride }, Source::LibRaw) => {
                if *stride < 3 || values.len() != width * height * stride {
                    bail!(
                        "LibRaw unpacked {} values, {stride} a pixel, for a picture of {width}x{height}",
                        values.len()
                    );
                }
            }
            _ => bail!("the DNG's picture was handed over in a form its compression does not take"),
        }
        let codes_of_values = match values {
            Values::Unpacked { .. } => self.codes_of_values(),
            Values::File(_) => None,
        };
        let rendering = self.rendering.as_ref().filter(|_| profile);
        // Both zeroed, so that the pages are the kernel's until the tiles'
        // threads write them, rather than filled here on one thread: a
        // picture of this size is hundreds of megabytes each.
        let mut data = vec![0u16; width * height * 3];
        let mut ratios: Option<Vec<f16>> =
            rendering.map(|_| vec![0u16; width * height * 3].reinterpret_into());
        let range: Mutex<Range> = Mutex::new(None);
        let tallies = Mutex::new(vec![vec![0u32; self.full() as usize + 1]; 3]);
        let [tile_width, tile_height] = self.tile.map(|side| side as usize);
        let across = width.div_ceil(tile_width);
        let count = across * height.div_ceil(tile_height);
        let mut failures: Vec<Option<anyhow::Error>> = (0..count).map(|_| None).collect();
        // Each tile's rows of the picture, its thread's alone to write.
        let outputs = rows_of_tiles(&mut data, width * 3, tile_width * 3, tile_height, across);
        let mut lifted_rows = ratios
            .as_mut()
            .map(|ratios| rows_of_tiles(ratios, width * 3, tile_width * 3, tile_height, across));
        rayon_core::scope(|scope| {
            for (index, (failure, rows)) in failures.iter_mut().zip(outputs).enumerate() {
                let (range, tallies) = (&range, &tallies);
                let (values, codes_of_values) = (&values, codes_of_values.as_deref());
                let lifted = match (rendering, lifted_rows.as_mut()) {
                    (Some(rendering), Some(all)) => {
                        Some((rendering, std::mem::take(&mut all[index])))
                    }
                    _ => None,
                };
                scope.spawn(move |_| {
                    let at = [index % across, index / across];
                    let tile = Tile {
                        index,
                        at,
                        values,
                        codes_of_values,
                        rows,
                        lifted,
                    };
                    match self.tile(tile, tallies) {
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
        let tallies = tallies
            .into_inner()
            .map_err(|_| anyhow!("a tile's thread panicked"))?;
        image.sensor = Some(Arc::new(self.sensor(tallies, turned)?));
        if let (Some(rendering), Some(ratios)) = (rendering, ratios) {
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

    /// The sensor's counts beside the developed picture: the stored values
    /// tallied a plane to the channel, and what works a developed pixel
    /// back to them — the matrix undone, then the levels and the table. The
    /// counts are balanced as stored where the neutral is white already,
    /// and the Develop section's balance is otherwise what the matrix did
    /// to carry the neutral to white, the green at 1.
    fn sensor(
        &self,
        tallies: Vec<Vec<u32>>,
        turned: crate::image::orient::Orientation,
    ) -> Result<Sensor> {
        let full = self.full();
        let wide: Matrix = std::array::from_fn(|row| self.matrix[row].map(f64::from));
        let inverse = profile::invert(wide)?;
        let balance = match self.neutral {
            Some(neutral) if neutral.iter().all(|value| *value > 0.0) => {
                std::array::from_fn(|c| (neutral[1] / neutral[c]) as f32)
            }
            _ => [1.0; 3],
        };
        Ok(Sensor {
            layout: sensor::Layout::Rgb(sensor::Recovery {
                inverse: std::array::from_fn(|row| inverse[row].map(|value| value as f32)),
                black: self.black,
                white: self.white,
                black_across: self.black_across.clone(),
                black_down: self.black_down.clone(),
                table: self.table.clone(),
            }),
            turned,
            white: full,
            black: self.black_codes(),
            stored: Arc::new(Stored::of_codes(tallies, full)),
            development: sensor::Development {
                demosaic: sensor::Demosaic::None,
                balance,
                kelvin: self.kelvin.map(|kelvin| kelvin as f32),
            },
        })
    }

    /// The values of tile `index`, the `at`th across and down, three a
    /// pixel at the tile's width: decoded from the file's JPEG XL or
    /// lossless JPEG, or copied out of what LibRaw unpacked, the rows past
    /// the picture left off.
    fn values(&self, tile: &Tile) -> Result<Vec<u16>> {
        match *tile.values {
            Values::File(bytes) => {
                let codestream = &bytes[self.tiles[tile.index].clone()];
                match self.source {
                    Source::LosslessJpeg => {
                        let samples = self.tile[0] as usize * self.tile[1] as usize * 3;
                        super::ljpeg::decode(codestream, samples)
                    }
                    _ => decode_tile(codestream, self.tile),
                }
            }
            Values::Unpacked { values, stride } => {
                let (width, height) = (self.width as usize, self.height as usize);
                let top = tile.at[1] * self.tile[1] as usize;
                let rows = (self.tile[1] as usize).min(height - top);
                let pixels = &values[top * width * stride..(top + rows) * width * stride];
                Ok(pixels
                    .chunks_exact(stride)
                    .flat_map(|pixel| pixel[..3].iter().copied())
                    .collect())
            }
        }
    }

    /// Decodes tile `index`, the `at`th across and down, and develops the
    /// part of it inside the picture into the tile's rows of the picture, a
    /// band of rows to a thread; with `lifted`, works out what the profile
    /// renders each pixel as, and writes the ratio into the tile's rows of
    /// the map. Says the least and the most it lifted anything by, in
    /// stops.
    fn tile(&self, tile: Tile, tallies: &Mutex<Vec<Vec<u32>>>) -> Result<Range> {
        let [tile_width, tile_height] = self.tile.map(|side| side as usize);
        let at = tile.at;
        let values = self.values(&tile)?;
        // Through the table already, where LibRaw unpacked.
        let linearized = matches!(tile.values, Values::Unpacked { .. });
        let Tile {
            mut rows,
            lifted,
            codes_of_values,
            ..
        } = tile;
        let left = at[0] * tile_width;
        let top = at[1] * tile_height;
        // A tile at the right or the bottom is padded past the picture.
        let columns = tile_width.min(self.width as usize - left);
        if rows.len() != tile_height.min(self.height as usize - top)
            || rows.iter().any(|row| row.len() != columns * 3)
        {
            bail!("the tile's rows of the picture are not the tile's size");
        }
        let (rendering, mut map) = match lifted {
            Some((rendering, map)) => (Some(rendering), Some(map)),
            None => (None, None),
        };
        // Where each column of the tile falls on the gain table's grid,
        // worked out once rather than at every pixel; a row's, likewise,
        // in the band.
        let across: Vec<Tap> = match rendering {
            Some(rendering) => (0..columns)
                .map(|column| rendering.tap(1, ((left + column) as f32 + 0.5) / self.width as f32))
                .collect(),
            None => Vec::new(),
        };
        let bands = rows.len().div_ceil(BAND_ROWS);
        let mut map_bands: Vec<Option<&mut [&mut [f16]]>> = match map.as_mut() {
            Some(map) => map.chunks_mut(BAND_ROWS).map(Some).collect(),
            None => (0..bands).map(|_| None).collect(),
        };
        let mut results: Vec<Option<Result<Developed>>> = (0..bands).map(|_| None).collect();
        rayon_core::scope(|scope| {
            let bands = rows
                .chunks_mut(BAND_ROWS)
                .zip(values.chunks(BAND_ROWS * tile_width * 3))
                .zip(results.iter_mut())
                .enumerate();
            for (index, ((rows, values), result)) in bands {
                let band = Band {
                    top: top + index * BAND_ROWS,
                    left,
                    columns,
                    tile_width,
                    values,
                    linearized,
                    codes_of_values,
                    rows,
                    map: map_bands[index].take(),
                    rendering,
                    across: &across,
                };
                scope.spawn(move |_| *result = Some(self.band(band)));
            }
        });
        let mut range: Range = None;
        let mut tallies = tallies
            .lock()
            .map_err(|_| anyhow!("a tile's thread panicked"))?;
        for result in results {
            let developed = result.ok_or_else(|| anyhow!("a band's thread panicked"))??;
            for (plane, counts) in tallies.iter_mut().zip(&developed.tallied) {
                for (sum, count) in plane.iter_mut().zip(counts) {
                    *sum += count;
                }
            }
            range = widen(range, developed.range);
        }
        Ok(range)
    }

    /// Develops one band of a tile's rows, as a thread is given it.
    fn band(&self, band: Band) -> Result<Developed> {
        let Band {
            top,
            left,
            columns,
            tile_width,
            values,
            linearized,
            codes_of_values,
            rows,
            mut map,
            rendering,
            across,
        } = band;
        // The least and the most ratio made, in stops once the band is done
        // rather than a logarithm a value.
        let mut ratios: Range = None;
        // The band's own tallies of the stored values, merged into the
        // picture's once at the end rather than under a lock a value at a
        // time.
        let codes = self.full() as usize + 1;
        let mut tallied = vec![vec![0u32; codes]; 3];
        for (row, out) in rows.iter_mut().enumerate() {
            let y = top + row;
            let down = rendering.map_or_else(Tap::default, |rendering| {
                rendering.tap(0, (y as f32 + 0.5) / self.height as f32)
            });
            for column in 0..columns {
                let x = left + column;
                let pixel = (row * tile_width + column) * 3;
                let camera: [f32; 3] = std::array::from_fn(|c| {
                    let value = values[pixel + c];
                    let (code, linear) = if linearized {
                        let code = match codes_of_values {
                            Some(codes) => codes[usize::from(value)],
                            None => value,
                        };
                        (code, f32::from(value))
                    } else {
                        let linear = match &self.table {
                            Some(table) => table[usize::from(value).min(table.len() - 1)],
                            None => f32::from(value),
                        };
                        (value, linear)
                    };
                    tallied[c][usize::from(code).min(codes - 1)] += 1;
                    let black = self.black[c] + self.black_across[x] + self.black_down[y];
                    ((linear - black) / (self.white[c] - black)).clamp(0.0, 1.0)
                });
                let developed = apply(self.matrix, camera).map(|value| value.clamp(0.0, 1.0));
                let stored = developed.map(|value| (value * 65535.0 + 0.5) as u16);
                out[column * 3..column * 3 + 3].copy_from_slice(&stored);
                if let (Some(rendering), Some(map)) = (rendering, map.as_mut()) {
                    let rendered = rendering.render(developed, down, across[column]);
                    for c in 0..3 {
                        let lift = f16::from_f32(ratio(rendered[c], stored[c]));
                        map[row][column * 3 + c] = lift;
                        let lift = lift.to_f32();
                        ratios = widen(ratios, Some([lift, lift]));
                    }
                }
            }
        }
        Ok(Developed {
            tallied,
            range: ratios.map(|[least, most]| [least.log2(), most.log2()]),
        })
    }
}

/// One band of a tile's rows, as a thread is given it.
struct Band<'a> {
    /// The band's first row of the picture, and the tile's first column.
    top: usize,
    left: usize,
    /// How many of the tile's columns are inside the picture.
    columns: usize,
    tile_width: usize,
    /// The tile's values for the band's rows, three a pixel at the tile's
    /// width; whether they are through the linearization table already,
    /// and [`Linear::codes_of_values`] where they are.
    values: &'a [u16],
    linearized: bool,
    codes_of_values: Option<&'a [u16]>,
    /// The band's rows of the picture, and of the map of ratios where the
    /// profile's rendering is asked for.
    rows: &'a mut [&'a mut [u16]],
    map: Option<&'a mut [&'a mut [f16]]>,
    rendering: Option<&'a Rendering>,
    /// Where each of the tile's columns falls on the gain table's grid.
    across: &'a [Tap],
}

/// What a band's development found: the stored codes tallied a plane to
/// the channel, and the least and the most it lifted anything by.
struct Developed {
    tallied: Vec<Vec<u32>>,
    range: Range,
}

/// One tile's work, as a thread is given it.
struct Tile<'a> {
    /// Which tile, row by row, and the `at`th across and down.
    index: usize,
    at: [usize; 2],
    values: &'a Values<'a>,
    /// [`Linear::codes_of_values`], for a picture LibRaw unpacked.
    codes_of_values: Option<&'a [u16]>,
    /// The tile's rows of the picture, each as wide as the tile is inside
    /// the picture.
    rows: Vec<&'a mut [u16]>,
    /// The profile's rendering, and the tile's rows of the map of its
    /// ratios, where the rendering is asked for.
    lifted: Option<(&'a Rendering, Vec<&'a mut [f16]>)>,
}

/// `buffer`, a picture of `stride` values a row, cut into each tile's rows
/// — `across` tiles to a row of tiles, `tile_width` values across and
/// `tile_height` rows down, the last across and down as wide and as deep as
/// the picture leaves them — in the tiles' order, row by row, so that each
/// tile's thread writes its own part of the picture with no lock between
/// them.
fn rows_of_tiles<T>(
    buffer: &mut [T],
    stride: usize,
    tile_width: usize,
    tile_height: usize,
    across: usize,
) -> Vec<Vec<&mut [T]>> {
    let mut tiles: Vec<Vec<&mut [T]>> = Vec::new();
    for band in buffer.chunks_mut(stride * tile_height) {
        let first = tiles.len();
        tiles.extend((0..across).map(|_| Vec::with_capacity(tile_height)));
        for row in band.chunks_exact_mut(stride) {
            let mut rest = row;
            for tile in &mut tiles[first..] {
                let (mine, after) = rest.split_at_mut(tile_width.min(rest.len()));
                tile.push(mine);
                rest = after;
            }
        }
    }
    tiles
}

/// Where a JPEG XL picture's tiles are: each tile's size, and each one's
/// codestream in the file, row by row. Tiles, or strips: a strip is a tile
/// as wide as the picture.
fn tiles(
    first: &Directory,
    picture: &Directory,
    width: u32,
    height: u32,
) -> Result<([u32; 2], Vec<std::ops::Range<usize>>)> {
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
    // A tile is decoded whole, so it is held to the same ceiling as the
    // picture: one claiming to be vast is refused before the decoder is
    // asked for it.
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
    Ok((tile, tiles))
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
    use crate::image::orient::Orientation;

    fn fixture(name: &str) -> Vec<u8> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("test_images")
            .join(name);
        std::fs::read(path).expect("the fixture is there")
    }

    /// A DNG whose picture is Linear Raw is taken, from its JPEG XL or
    /// lossless JPEG tiles; a mosaiced DNG, and anything that is not a
    /// DNG, are LibRaw's.
    #[test]
    fn only_a_linear_raw_dng_is_taken() {
        let jxl = Linear::read(&fixture("dng-jxl.dng")).unwrap().unwrap();
        assert_eq!(jxl.source(), Source::JpegXl);
        let ljpeg = Linear::read(&fixture("dng-ljpeg.dng")).unwrap().unwrap();
        assert_eq!(ljpeg.source(), Source::LosslessJpeg);
        assert_eq!(ljpeg.size(), (32, 24));
        assert_eq!(ljpeg.tile, [16, 16]);
        assert_eq!(ljpeg.tiles.len(), 4);
        assert!(Linear::read(&fixture("dng-cfa.dng")).unwrap().is_none());
        assert!(Linear::read(&fixture("tiff-rgb8.tif")).unwrap().is_none());
        assert!(Linear::read(b"not a tiff").unwrap().is_none());
    }

    /// The lossless JPEG fixture is the JPEG XL one's picture under the
    /// other compression, so the two develop to the same picture, profile
    /// and all; and one with a tile cut short will not develop, as the cut
    /// JPEG XL one will not.
    #[test]
    fn lossless_jpeg_tiles_develop_as_the_jpeg_xl_ones_do() {
        let jxl = fixture("dng-jxl.dng");
        let ljpeg = fixture("dng-ljpeg.dng");
        let from_jxl = Linear::read(&jxl)
            .unwrap()
            .unwrap()
            .develop(Values::File(&jxl), true, Orientation::NoTransforms)
            .unwrap();
        let linear = Linear::read(&ljpeg).unwrap().unwrap();
        let from_ljpeg = linear
            .develop(Values::File(&ljpeg), true, Orientation::NoTransforms)
            .unwrap();
        let (Samples::U16 { data: a, .. }, Samples::U16 { data: b, .. }) =
            (&from_jxl.samples, &from_ljpeg.samples)
        else {
            panic!("sixteen bits");
        };
        assert_eq!(a, b);
        let (Some(a), Some(b)) = (&from_jxl.gain_map, &from_ljpeg.gain_map) else {
            panic!("both profiles");
        };
        let (Data::Ratios(a_ratios), Data::Ratios(b_ratios)) = (&a.data, &b.data) else {
            panic!("ratios");
        };
        assert_eq!(a_ratios, b_ratios);
        let (Lift::Profile(a), Lift::Profile(b)) = (&a.lift, &b.lift) else {
            panic!("profiles");
        };
        assert_eq!(a.range, b.range);
        assert_eq!(a.baseline, b.baseline);
        let planes = |image: &DecodedImage| -> Vec<(f32, f32)> {
            let sensor = image.sensor.as_ref().unwrap();
            sensor
                .stored
                .planes
                .iter()
                .map(|plane| (plane.min, plane.max))
                .collect()
        };
        assert_eq!(planes(&from_jxl), planes(&from_ljpeg));
        // Cut short by a marker forty bytes before its end: what is past
        // the marker is not read, and the frame is not finished.
        let mut cut = ljpeg.clone();
        let end = linear.tiles[1].end;
        cut[end - 40..end].fill(0xFF);
        let Err(error) = linear.develop(Values::File(&cut), true, Orientation::NoTransforms) else {
            panic!("refused");
        };
        assert!(format!("{error:#}").contains("tile 1"), "{error:#}");
        assert!(format!("{error:#}").contains("ends before"), "{error:#}");
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
            assert_eq!(table.points(), [2, 1]);
            assert_eq!(table.steps(), 2);
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
        let gain_at = |color: [f32; 3], place: [f32; 2]| table.gain_for(table.input(color), place);
        let left = |red: f32| gain_at([red, 0.0, 0.0], [0.5, 0.0]);
        assert_eq!(left(0.0), 1.0);
        // A third of the way in is the second step.
        assert!((left(1.0 / 3.0) - 2.0).abs() < 1e-5);
        assert!((left(0.5) - 2.5).abs() < 1e-5);
        // The top is held at the last step.
        assert_eq!(left(1.0), 3.0);
        assert_eq!(left(4.0), 3.0, "clamped first");
        // Half way across is half way between the two points.
        let middle = gain_at([0.0; 3], [0.5, 0.5]);
        assert!((middle - 2.5).abs() < 1e-5, "{middle}");
        assert_eq!(gain_at([0.0; 3], [0.5, 1.0]), 4.0);
        assert_eq!(gain_at([0.0; 3], [0.5, 7.0]), 4.0, "past the edge");
    }

    /// The fixture's profile renders white two stops down; a primary is
    /// left alone in the channels it holds nothing of; and asked not to,
    /// the develop leaves the profile behind.
    #[test]
    fn the_fixtures_profile_bakes_the_ratios_it_states() {
        let bytes = fixture("dng-jxl.dng");
        let linear = Linear::read(&bytes).unwrap().unwrap();
        let image = linear
            .develop(Values::File(&bytes), true, Orientation::NoTransforms)
            .unwrap();
        let map = image.gain_map.as_deref().expect("a profile");
        let Lift::Profile(profile) = &map.lift else {
            panic!("a profile's lift");
        };
        assert_eq!(profile.baseline, -2.0);
        let table = profile.table.as_deref().expect("a gain table");
        assert_eq!((table.points(), table.steps()), ([2, 2], 2));
        assert_eq!(profile.curve.as_deref().map(Vec::len), Some(3));
        let white = map.stops_at(24, 18, 32, 24);
        for stops in white {
            assert!((stops + 2.0).abs() < 0.01, "{white:?}");
        }
        let red = map.stops_at(8, 6, 32, 24);
        assert_eq!(red[1..], [0.0, 0.0], "{red:?}");
        assert!(profile.range[0] <= -2.0 && profile.range[1] >= -2.0);

        assert!(
            linear
                .develop(Values::File(&bytes), false, Orientation::NoTransforms)
                .unwrap()
                .gain_map
                .is_none()
        );
    }

    /// The developed picture carries its sensor: the stored codes tallied a
    /// plane to the channel over the linearization table's codes, and a
    /// developed pixel worked back to the codes the tile holds.
    #[test]
    fn the_developed_picture_carries_the_stored_codes() {
        use crate::image::sensor::Layout;
        let bytes = fixture("dng-jxl.dng");
        let linear = Linear::read(&bytes).unwrap().unwrap();
        let image = linear
            .develop(Values::File(&bytes), true, Orientation::NoTransforms)
            .unwrap();
        let sensor = image.sensor.as_deref().expect("the sensor");
        let table = linear.table.as_deref().expect("the fixture's table");
        assert_eq!(sensor.white, (table.len() - 1) as f32);
        assert!(matches!(sensor.layout, Layout::Rgb(_)));
        assert_eq!(sensor.development.demosaic, sensor::Demosaic::None);
        assert!(sensor.development.kelvin.is_some_and(|kelvin| kelvin > 0.0));
        assert!(sensor.development.balanced_as_stored());
        let pixels = (image.width * image.height) as f32;
        for plane in &sensor.stored.planes {
            // Every pixel counted: the share at or below the top is all.
            assert_eq!(plane.rank(1.0).map(|rank| rank <= 1.0), Some(true));
            assert!(plane.max <= 1.0 && plane.min >= 0.0, "{plane:?}");
        }
        // The first tile's pixels, as stored, are what the developed pixels
        // work back to: the fixture's matrix is the identity, so even a
        // primary — a channel at the clamp, which the reading calls
        // approximate — comes back, short of the clamp only where the code
        // itself ran past white.
        let stored = decode_tile(&bytes[linear.tiles[0].clone()], linear.tile).unwrap();
        let Samples::U16 { data, .. } = &image.samples else {
            panic!("sixteen bits");
        };
        let mut approximate = 0;
        for y in 0..image.height.min(linear.tile[1]) {
            for x in 0..image.width.min(linear.tile[0]) {
                let at = ((y * image.width + x) * 3) as usize;
                let developed: Vec<f32> = data[at..at + 3]
                    .iter()
                    .map(|&value| f32::from(value) / 65535.0)
                    .collect();
                let reading = sensor
                    .at([x, y], [image.width, image.height], &developed)
                    .unwrap();
                approximate += usize::from(!reading.exact);
                let tile_at = ((y * linear.tile[0] + x) * 3) as usize;
                for c in 0..3 {
                    let count = reading.counts[c].unwrap();
                    let held = f32::from(stored[tile_at + c]);
                    let agrees = (count - held).abs() <= 1.0
                        || (developed[c] >= 1.0 && held >= count)
                        || (developed[c] <= 0.0 && held <= count);
                    assert!(agrees, "({x}, {y}) channel {c}: {count} for {held}");
                }
            }
        }
        assert!(
            approximate > 0,
            "the primaries sit at the clamp, of {pixels}"
        );
    }

    /// A profile tag that is not what the specification says never stops
    /// the picture developing: a cut gain table is passed over, and the
    /// rest of the profile still applies.
    #[test]
    fn a_profile_tag_never_refuses_the_develop() {
        let bytes = fixture("dng-jxl-profile-cut.dng");
        let linear = Linear::read(&bytes).unwrap().unwrap();
        let image = linear
            .develop(Values::File(&bytes), true, Orientation::NoTransforms)
            .unwrap();
        let Some(Lift::Profile(profile)) = image.gain_map.as_deref().map(|map| &map.lift) else {
            panic!("the rest of the profile");
        };
        assert!(profile.table.is_none());
        assert_eq!(profile.baseline, -2.0);
    }
}
