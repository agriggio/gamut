//! Value statistics, used to choose a sensible window for images whose
//! numbers do not conveniently fill 0..1 — 12-bit sensor data stored in
//! 16-bit containers, or HDR frames with a few very bright highlights — and
//! the counts the histogram panel plots.
//!
//! Every pixel is counted, in one pass divided between the threads: the
//! file's own channels into a tally of their stored values, which for
//! integer samples is a bin to the code and so exact, and for floats a bin
//! to every thousandth of an octave ([`Tally`]); the luminance, and the
//! channels as the working space has them where that moves them, into the
//! same float tallies. Everything else here — the extremes, the key, the
//! percentiles, the fine bins on the plot's axis, the Image section's plot
//! of each stored channel — is read off those tallies afterwards, which is a
//! walk over some thousands of bins rather than over the picture again.
//! Nothing is sampled: a tally of every pixel is what makes the extremes the
//! file's own rather than a stride's, and it is cheaper than the sampled
//! scan it replaces, since a tally is an index and an increment where the
//! scan decoded, converted and took the logarithm of each pixel in turn.

use std::sync::Arc;

use super::gain_map::{GainMap, Table};
use super::{Channels, DecodedImage, Primaries, Referred, Sample, Samples, Transfer};

/// Bins are plenty for percentile work and cheap to keep around; the UI draws
/// this directly as a histogram.
pub const BINS: usize = 256;

/// The finer bins the same pixels are counted in beside [`BINS`], on the
/// same axis: what a plot of the picture as some later stage makes of it is
/// rebinned from — see [`Fine::rebinned`] — so that a change to that stage
/// costs a walk over these rather than over the picture. Sixteen to a
/// coarse bin, and laid out so that every coarse bin's middle is a fine
/// bin's middle: an 8-bit file's codes on an axis of 0..1 then stand each
/// on the middle of its own coarse bin, rather than up to half a fine bin
/// off it, and a stage that does nothing puts every one wholly back in it.
pub const FINE_BINS: usize = (BINS - 1) * 16 + 1;

/// Number of color components a pixel carries, alpha aside.
pub const COLOR: usize = 3;

/// The share of the lit pixels left out at each end when the key of a scene
/// is read: the darkest, so that a floor of near-nothing — a render's
/// residue where a bounce all but died, a merge's darkest exposure divided
/// out — does not pull the meter down by however much of the frame it
/// covers, and the brightest, which are the light sources the curve is
/// for. A meter reads the scene between them.
const KEY_TRIM: f64 = 0.05;

/// How a plot of the picture is spaced along its axis: what a position on
/// [`Plot`]'s axis, and on [`Fine`]'s, stands for.
///
/// Everything that puts a value on the plot or reads one off it — the scan,
/// the band and its handles, the response curve, the row of stops, the
/// pointer's rule — goes through [`Axis::place`] and [`Axis::value`], so
/// that the plot and what is drawn over it cannot be spaced two ways.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Axis {
    /// The file's own curve: a value stands where the file encodes it,
    /// which for a linear file is the value itself. See [`Plot`] for why.
    Curve(Transfer),
    /// Stops: a value stands at its base-2 logarithm, and a value with no
    /// light in it — zero, or less — below every floor. For linear light,
    /// which has no curve of its own: a Radiance picture runs from nothing
    /// to a few hundred, and almost all of it is in the bottom hundredth,
    /// where a linear axis would have it in the first bin, and a raw
    /// developed to sixteen bits is the same light in integers, a dark frame
    /// of it in the first tenth. Spaced in stops, the shadows get the room
    /// the eye gives them, a stop of exposure moves every value the same
    /// distance, and a window is a stretch of the axis rather than a sliver
    /// of it. Not for a measurement, whose values are quantities rather than
    /// light and may be negative, and whose stops no one counts.
    Stops,
}

impl Axis {
    /// The axis `image` is plotted on. The scan settles on a linear axis
    /// instead where [`Axis::Stops`] has nothing to stand on — a picture
    /// with no light in it — so what is read from is [`Stats::axis`].
    pub fn of(image: &DecodedImage) -> Self {
        let transfer = image.color.transfer;
        if transfer.is_linear() && image.referred != Referred::Measured {
            Axis::Stops
        } else {
            Axis::Curve(transfer)
        }
    }

    /// Where a linear value stands on the axis.
    pub fn place(self, linear: f32) -> f32 {
        match self {
            Axis::Curve(transfer) => encode(transfer, linear),
            Axis::Stops => stops(linear),
        }
    }

    /// The linear value standing at `place` on the axis.
    pub fn value(self, place: f32) -> f32 {
        match self {
            Axis::Curve(transfer) => transfer.to_linear(place),
            Axis::Stops => place.exp2(),
        }
    }

    /// Where a sample as the file stores it — on its own curve, normalized
    /// to 0..1 for integer samples — stands on the axis: where it is, on a
    /// curve, and its stops otherwise, a linear file's stored sample being
    /// the linear value.
    pub fn place_stored(self, stored: f32) -> f32 {
        match self {
            Axis::Curve(_) => stored,
            Axis::Stops => stops(stored),
        }
    }

    /// Where one pixel read back out of `image` stands on a plot of it: its
    /// luminance, worked out as the scan works it out, on this axis. `None`
    /// for a value the scan would have thrown out as nodata. See
    /// [`Plot::bin_of`] for why it is read from [`Sample::linear`].
    pub fn place_of(self, image: &DecodedImage, sample: &Sample) -> Option<f32> {
        let value = luminance(sample.linear(), sample.channels);
        if !value.is_finite() || image.nodata.is_some_and(|sentinel| value == sentinel) {
            return None;
        }
        Some(self.place(value))
    }
}

/// A linear value in stops. Nothing, and less than nothing, has no
/// logarithm: minus infinity, which is below every floor, rather than the
/// not-a-number `log2` makes of a negative.
fn stops(linear: f32) -> f32 {
    linear.max(0.0).log2()
}

/// How far down a stops axis runs below its top at most, in stops: what a
/// plot of [`BINS`] has room to say anything about at ten or so bins to the
/// stop, and three times the range a photograph holds. A render's tail of
/// near-nothing below it piles into the first bin.
const STOPS_REACH: f32 = 24.0;

/// The share of the lit pixels left out at the bottom when a stops axis's
/// floor is read from them: a handful of stray pixels at next to nothing
/// would otherwise put the floor tens of stops down and the picture back
/// into a sliver of the plot. Far less than [`KEY_TRIM`], which is there
/// to keep a residue out of a mean, where this is only to keep strays off
/// an axis: the share is what piles into the first bin.
const FLOOR_TRIM: f64 = 0.001;

/// The range a stops axis spans — its floor and its top, in whole stops —
/// for a picture whose plotted channels reach `highest` and whose lit
/// pixels are tallied in `lit`: the top is the first whole stop at or above
/// the brightest channel, and the floor is the darkest lit stop after the
/// bottom [`FLOOR_TRIM`] is left off, no more than [`STOPS_REACH`] under the
/// top and at least a stop under it, so that the axis always has a span.
/// `None` where nothing reaches above zero, which has no top.
fn stops_axis(highest: f32, lit: &Lit) -> Option<(f32, f32)> {
    // Written so that a `highest` that is not a number has no top either.
    let top = highest.log2().ceil();
    if !top.is_finite() {
        return None;
    }
    let trim = (lit.total as f64 * FLOOR_TRIM) as u64;
    let mut seen = 0u64;
    let darkest = lit
        .bins
        .iter()
        .find(|bin| {
            seen += bin.count;
            seen > trim
        })
        .map_or(f32::NEG_INFINITY, |bin| bin.low);
    let floor = darkest.floor().max(top - STOPS_REACH).min(top - 1.0);
    Some((floor, top))
}

/// The histogram the interface draws.
///
/// Binned on the curve the file stores its samples with, rather than on the
/// linear values the rest of [`Stats`] is measured in. Two reasons, and the
/// first is a correctness one:
///
/// Uniform bins over decoded values cannot be filled evenly by a quantized
/// file. A code step near white is several times wider in linear terms than
/// one near black — for 8-bit sRGB, 0.0089 against a bin width of 0.0039 — so
/// the highlights come out as a comb of spikes with empty bins between them
/// while a dozen shadow codes pile into bin zero. On the storage curve the
/// codes land one to a bin, or denser, and the comb cannot arise.
///
/// The second is that the eye's response is close to the curve a
/// display-referred file is encoded with, so plotting against it gives equal
/// width to equal perceived steps: mid gray sits in the middle rather than a
/// fifth of the way along. Linear integer files — sensor counts, and
/// measurements — store linear samples, so their plot stays linear, which is
/// what measurement work wants. Light stored linear as floats is spaced in
/// stops instead: see [`Axis::Stops`].
///
/// The axis follows the same split. A curved file is display-referred, so it
/// spans the range such a file can hold — 0..1, widened by any over-range
/// float samples — which puts clipping at either end where you can see it and
/// lands 8-bit codes one to a bin. Linear samples have no nominal range to
/// speak of, since the case that matters is 12-bit data in a 16-bit
/// container, so their axis is the range actually measured; in stops, from
/// the darkest the picture holds to say anything about up to its brightest
/// ([`stops_axis`]). Which it is, and what a position on it stands for, is
/// [`Stats::axis`].
///
/// The channels plotted are the working space's, on the file's own curve: a
/// P3 or BT.2020 file's color is carried into BT.709 before it is binned,
/// as the shader carries it before the window goes on, so a vivid red the
/// working space cannot hold stands past white on the plot — and its green,
/// which the matrix takes below zero, below black — where the window and
/// the surface will clip it. Encoded back through the file's curve, the
/// axis is still the file's, and a BT.709 file's codes still land one to a
/// bin, since its matrix is the identity and the codes are read as stored.
#[derive(Clone, Debug)]
pub struct Plot {
    /// What the bins span, on the file's own curve.
    pub min: f32,
    pub max: f32,
    /// Counts over `min..max` for the luminance of each pixel.
    pub luma: [u32; BINS],
    /// Red, green and blue, for an image that carries color. Their range is
    /// what sets the axis; luminance is a weighted average of them and so
    /// always falls inside it.
    pub color: Option<[[u32; BINS]; COLOR]>,
}

impl Plot {
    /// The two ends of the axis, as [`Display::step_black`] and
    /// [`Display::step_white`] take them.
    ///
    /// [`Display::step_black`]: super::display::Display::step_black
    /// [`Display::step_white`]: super::display::Display::step_white
    pub fn range(&self) -> [f32; 2] {
        [self.min, self.max]
    }

    /// Which bin of the luminance plane one pixel read back out of `image`
    /// was counted in, so that a marker can point at the bar its own pixel is
    /// part of.
    ///
    /// The twin of the luminance pass in [`Stats::scan`], and it has to be
    /// worked out from [`Sample::linear`] rather than from
    /// [`Sample::color`]: the scan bins what the transfer function, the
    /// lift and the primaries matrix make of the file's components, where
    /// that color has also had its premultiplication undone. On a file with
    /// premultiplied alpha, a bin taken from the color would name a bar the
    /// pixel is not in.
    ///
    /// `None` where there is no bar to point at: an axis with no span, a
    /// value the scan would have thrown out as nodata, or one outside the
    /// range the plot covers.
    pub fn bin_of(&self, axis: Axis, image: &DecodedImage, sample: &Sample) -> Option<usize> {
        self.bin_at(axis.place_of(image, sample)?)
    }

    /// Which bin a value already on the plot's own axis falls in: the same
    /// arithmetic the scan bins with, so the two cannot drift apart.
    ///
    /// Out of range is `None` rather than the end bin the scan clamps to. The
    /// scan clamps because every value it sees is one the axis was measured
    /// from and a clamp there is only guarding the arithmetic; a marker asked
    /// about a value off the axis has genuinely nowhere to stand, and one
    /// held at the edge would claim a bar that is not the pixel's.
    pub fn bin_at(&self, stored: f32) -> Option<usize> {
        bin_in([self.min, self.max], stored)
    }

    /// The share of the samples the display is throwing away at each end:
    /// at or below `black`, and at or above `white`, both on the plot's own
    /// axis: how much of a picture is clipped as a number to read rather
    /// than a spike to guess the height of. Nothing on screen writes it
    /// yet.
    ///
    /// Each plane on its own, and the worst of them: a red flower blows its
    /// red channel long before its luminance goes, and the plane that has
    /// piled into the end bin is the one the eye finds on the plot. On a gray
    /// image the one plane is the answer.
    ///
    /// A bin is counted as clipped when its own value is at or past the
    /// bound, so on a quantized file, whose codes land one to a bin, a bound
    /// sitting on a code counts exactly that code and everything beyond it.
    /// A bound off the axis clips nothing at that end, and so does one that
    /// is not a number, every comparison against which is false.
    #[cfg_attr(
        not(test),
        allow(dead_code, reason = "a readout of the clipping is to come")
    )]
    pub fn clipped(&self, black: f32, white: f32) -> [f32; 2] {
        // Written so that a non-finite axis fails as well, as `bin` does.
        let span = self.max - self.min;
        if span.partial_cmp(&f32::MIN_POSITIVE) != Some(std::cmp::Ordering::Greater) {
            return [0.0; 2];
        }
        let position = |value: f32| (value - self.min) / span * (BINS - 1) as f32;
        let (black, white) = (position(black), position(white));
        let planes: &[[u32; BINS]] = match &self.color {
            Some(planes) => planes,
            None => std::slice::from_ref(&self.luma),
        };
        let mut worst = [0.0f32; 2];
        for plane in planes {
            let total: u64 = plane.iter().map(|&count| u64::from(count)).sum();
            if total == 0 {
                continue;
            }
            let (mut below, mut above) = (0u64, 0u64);
            for (index, &count) in plane.iter().enumerate() {
                let at = index as f32;
                if at <= black {
                    below += u64::from(count);
                }
                if at >= white {
                    above += u64::from(count);
                }
            }
            let share = |part: u64| part as f32 / total as f32;
            worst = [worst[0].max(share(below)), worst[1].max(share(above))];
        }
        worst
    }

    fn empty() -> Self {
        Self {
            min: 0.0,
            max: 1.0,
            luma: [0; BINS],
            color: None,
        }
    }
}

/// Which of [`BINS`] bins spread as sample points over `range` a value `at`
/// on the same axis falls in: the bins are points from one end to the
/// other, and a value goes to the nearest. `None` off either end, or on an
/// axis with no span — written so that a non-finite axis fails the test
/// as well, since every comparison against a NaN is false.
pub fn bin_in(range: [f32; 2], at: f32) -> Option<usize> {
    let [min, max] = range;
    let span = max - min;
    if !span.is_finite() || span <= f32::MIN_POSITIVE {
        return None;
    }
    let bin = (at - min) * ((BINS - 1) as f32 / span) + 0.5;
    (0.0..BINS as f32).contains(&bin).then_some(bin as usize)
}

/// The pixels [`Plot`] counts, counted again in [`FINE_BINS`] over the same
/// axis, in the same pass and with the same rounding.
///
/// Kept for what a stage downstream of the file makes of the plot: the
/// panel's plot of what the screen shows is these pushed through the
/// display's response and binned again, which is a walk over four thousand
/// bins rather than over two million pixels, and so cheap enough to do on
/// every frame a handle is dragged.
#[derive(Clone, Debug)]
pub struct Fine {
    /// What the bins span: [`Plot`]'s own axis.
    pub min: f32,
    pub max: f32,
    pub luma: Vec<u32>,
    /// Red, green and blue, for an image that carries color.
    pub color: Option<[Vec<u32>; COLOR]>,
    /// How far apart a file's codes stand on this axis, for one that stores
    /// whole numbers: a value counted here stands for the stretch from
    /// halfway to the code below it to halfway to the code above. `None`
    /// for floats, which stand for themselves.
    ///
    /// The step as stored, which is what it is on the axis wherever the
    /// axis is the file's own curve. Where the picture is lifted through a
    /// gain map the codes are pulled further apart than this, and a plot of
    /// them may keep a little of the comb this is here to take out.
    pub code: Option<f32>,
}

impl Fine {
    fn empty(min: f32, max: f32, color: bool) -> Self {
        Self {
            min,
            max,
            luma: vec![0; FINE_BINS],
            color: color.then(|| std::array::from_fn(|_| vec![0; FINE_BINS])),
            code: None,
        }
    }

    /// The plot of what `through` makes of every value on this axis, binned
    /// in [`BINS`] over `out`: each fine bin's own value pushed through it,
    /// and its count spread evenly over the stretch of coarse bins it stands
    /// for once `through` has had it — a quantized file's code over the
    /// stretch to halfway to its neighbors ([`Fine::code`]) where that comes
    /// out narrower than a coarse bin, and over a coarse bin's width
    /// centered where it lands otherwise; never narrower than the fine bin
    /// itself comes out. What
    /// lands off either end of `out` piles into the end bin it went out of,
    /// which is the clip a stage that stops there makes; a value `through`
    /// makes no number of is left out. Counted in [`SHARES`] of a pixel.
    ///
    /// Spread, rather than dropped whole into the one bin each value lands
    /// in, because the two grids beat against each other otherwise: fine
    /// bins eight and a half to a coarse one put eight and nine into
    /// neighbors by turns, and a quantized file's codes a bin and a half
    /// apart put one and two, a comb on a plot of data with none in it.
    /// Codes spread each over its own stretch tile the axis, and come out
    /// even. The spread takes the beat out and nothing more: a code is
    /// never spread wider than a bin, so a stage that pulls a quantized
    /// file's codes apart further than that still leaves the empty bins
    /// between them, which is the posterization the plot is there to show;
    /// and a value an end clips stands wholly in the end bin.
    ///
    /// The color planes go through the same scalar function as the
    /// luminance, each channel on its own. That is exact where the stage
    /// acts on each channel alike — a window, an exposure, a clip, a curve
    /// that maps each channel through the same shape — and an approximation
    /// under a curve that moves a channel by what the others are, as the
    /// neutral curve's desaturation does: a plane is then plotted where the
    /// channel would go were it gray. Only the luminance plane is what the
    /// panel reads the clip from at that point, and the planes are for
    /// their shape.
    pub fn rebinned(&self, out: [f32; 2], through: impl Fn(f32) -> f32) -> Plot {
        let [low, high] = out;
        let mut plot = Plot {
            min: low,
            max: high,
            luma: [0; BINS],
            color: self.color.as_ref().map(|_| [[0; BINS]; COLOR]),
        };
        let span = self.max - self.min;
        let out_span = high - low;
        if !(span.is_finite() && span > f32::MIN_POSITIVE)
            || !(out_span.is_finite() && out_span > f32::MIN_POSITIVE)
        {
            return plot;
        }
        // Where each fine bin goes, worked out once for every plane: the
        // stretch of coarse bins it is spread over, in coarse bins, each
        // centered on its own index.
        let scale = (BINS - 1) as f32 / out_span;
        let fine_step = span / (FINE_BINS - 1) as f32;
        let at = |value: f32| (through(value) - low) * scale;
        let last = (BINS - 1) as f32;
        let code = self.code.map(|code| code.max(fine_step));
        let lands: Vec<Option<[f32; 2]>> = (0..FINE_BINS)
            .map(|index| {
                let value = self.min + index as f32 * fine_step;
                let middle = at(value);
                // A NaN fails the test and is left out.
                if middle.is_nan() {
                    return None;
                }
                // The fine bin's own edges, and the code's, where they are
                // numbers: `min` and `max` pass over a NaN.
                let edges = |half: f32| {
                    let (below, above) = (at(value - half), at(value + half));
                    (below.min(above).min(middle), below.max(above).max(middle))
                };
                let (fine_from, fine_to) = edges(fine_step / 2.0);
                let (from, to) = match code.map(|code| edges(code / 2.0)) {
                    Some((from, to)) if to - from < 1.0 => (from, to),
                    _ => (middle - 0.5, middle + 0.5),
                };
                let (from, to) = (from.min(fine_from), to.max(fine_to));
                // An infinity piles into the end it went out of, and a
                // stretch with an end at one is the bin its middle is in.
                Some(if (to - from).is_finite() {
                    [from, to]
                } else {
                    let bin = middle.clamp(0.0, last).round();
                    [bin - 0.5, bin + 0.5]
                })
            })
            .collect();
        let pour = |into: &mut [u32; BINS], from: &[u32]| {
            let mut spread = [0.0f64; BINS];
            for (&count, land) in from.iter().zip(&lands) {
                let Some([from, to]) = *land else {
                    continue;
                };
                if count == 0 {
                    continue;
                }
                spread_over(&mut spread, from, to, f64::from(count));
            }
            *into = spread.map(|count| (count * SHARES).round() as u32);
        };
        pour(&mut plot.luma, &self.luma);
        if let (Some(planes), Some(fine)) = (plot.color.as_mut(), &self.color) {
            for (plane, fine) in planes.iter_mut().zip(fine) {
                pour(plane, fine);
            }
        }
        plot
    }
}

/// `count` spread over the stretch `from..to` of bins, in bin units, where
/// bin `k` is the stretch from `k - 0.5` to `k + 0.5` and the two end bins
/// reach out past the axis, so that what lands off it piles into the end
/// it went out of. A stretch flattened to a point — the run past a window's
/// end — is the one bin it lands in.
fn spread_over(into: &mut [f64], from: f32, to: f32, count: f64) {
    let last = (into.len() - 1) as f32;
    let width = to - from;
    if width <= 0.0 {
        into[(from.round().clamp(0.0, last)) as usize] += count;
        return;
    }
    let first = (from + 0.5).floor().clamp(0.0, last) as usize;
    let final_bin = (to + 0.5).floor().clamp(0.0, last) as usize;
    for (bin, share) in into.iter_mut().enumerate().take(final_bin + 1).skip(first) {
        let left = if bin == 0 { from } else { bin as f32 - 0.5 };
        let right = if bin as f32 >= last {
            to
        } else {
            bin as f32 + 0.5
        };
        let overlap = right.min(to) - left.max(from);
        if overlap > 0.0 {
            *share += count * f64::from(overlap / width);
        }
    }
}

/// `count` whole pixels spread over the stretch `from..to` of bins, as
/// [`spread_over`] spreads a share, but kept whole: each bin gets the whole
/// pixels its share comes to, and the pixels left over go one each to the
/// bins with the largest remainders, so that the stretch holds exactly
/// `count` afterwards. One pixel goes wholly to the bin it mostly lies in.
fn spread_whole(into: &mut [u32], from: f32, to: f32, count: u32) {
    let last = (into.len() - 1) as f32;
    let width = to - from;
    if width <= 0.0 || count == 0 {
        into[(from.round().clamp(0.0, last)) as usize] += count;
        return;
    }
    let first = (from + 0.5).floor().clamp(0.0, last) as usize;
    let final_bin = (to + 0.5).floor().clamp(0.0, last) as usize;
    if first == final_bin {
        into[first] += count;
        return;
    }
    let mut given = 0u32;
    let mut remainders: Vec<(f64, usize)> = Vec::with_capacity(final_bin - first + 1);
    for (bin, slot) in into.iter_mut().enumerate().take(final_bin + 1).skip(first) {
        let left = if bin == 0 { from } else { bin as f32 - 0.5 };
        let right = if bin as f32 >= last {
            to
        } else {
            bin as f32 + 0.5
        };
        let overlap = (right.min(to) - left.max(from)).max(0.0);
        let share = f64::from(count) * f64::from(overlap / width);
        let whole = share.floor() as u32;
        *slot += whole;
        given += whole;
        remainders.push((share - f64::from(whole), bin));
    }
    remainders.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
    for (_, bin) in remainders.into_iter().take((count - given) as usize) {
        into[bin] += 1;
    }
}

/// What [`Fine::rebinned`] counts a pixel as: sixteen shares, so that a
/// pixel spread over two or three bins keeps a share in each. A plot is read
/// by its bins against one another and against their sum, which the unit
/// leaves alone.
pub const SHARES: f64 = 16.0;

// ---------------------------------------------------------------------------
// Tallies: what one pass over the pixels counts.

/// How finely a float is tallied: its exponent and this many bits of its
/// mantissa, so that every value within a thousandth of an octave of
/// another is counted with it. Finer than the fine bins of a plot anywhere
/// on a stops axis, and about as fine at the top of a curve's.
const MANTISSA_BITS: u32 = 10;
/// The octaves a float is tallied over: `2^EXPONENT_LOW` up to but not
/// including `2^EXPONENT_HIGH`, with a bin below the range for everything
/// smaller, zero among it, and one above it for everything larger. Sixty-
/// four octaves either side of one is more than any picture holds light
/// over, and a measurement outside it is in the end bins, whose exact
/// extremes are kept beside the tally.
const EXPONENT_LOW: i32 = -32;
const EXPONENT_HIGH: i32 = 32;
const OCTAVES: usize = (EXPONENT_HIGH - EXPONENT_LOW) as usize;
/// The bins one sign's magnitudes are tallied in: below the range, each
/// octave's `2^MANTISSA_BITS` bins, and above it.
const MAGNITUDES: usize = 2 + (OCTAVES << MANTISSA_BITS);
/// Both signs: the negatives first, largest magnitude first, so that the
/// bins run in value order from the most negative to the most positive.
const KEY_BINS: usize = 2 * MAGNITUDES;

/// Which magnitude bin a finite float with these `bits` falls in.
#[inline]
fn magnitude_of(bits: u32) -> usize {
    let exponent = ((bits >> 23) & 0xFF) as i32 - 127;
    if exponent < EXPONENT_LOW {
        0
    } else if exponent >= EXPONENT_HIGH {
        MAGNITUDES - 1
    } else {
        let mantissa = (bits >> (23 - MANTISSA_BITS)) & ((1 << MANTISSA_BITS) - 1);
        1 + (((exponent - EXPONENT_LOW) as usize) << MANTISSA_BITS) + mantissa as usize
    }
}

/// The bin a finite `value` is tallied in.
#[inline]
fn key_of(value: f32) -> usize {
    let bits = value.to_bits();
    let magnitude = magnitude_of(bits);
    if bits & 0x8000_0000 != 0 {
        MAGNITUDES - 1 - magnitude
    } else {
        MAGNITUDES + magnitude
    }
}

/// The values the magnitude bin `magnitude` spans, `low..high`.
fn magnitude_edges(magnitude: usize) -> (f32, f32) {
    if magnitude == 0 {
        (0.0, (EXPONENT_LOW as f32).exp2())
    } else if magnitude == MAGNITUDES - 1 {
        ((EXPONENT_HIGH as f32).exp2(), f32::MAX)
    } else {
        let within = magnitude - 1;
        let exponent = EXPONENT_LOW + (within >> MANTISSA_BITS) as i32;
        let step = 1.0 / (1u32 << MANTISSA_BITS) as f32;
        let fraction = (within & ((1 << MANTISSA_BITS) - 1)) as f32 * step;
        let scale = (exponent as f32).exp2();
        (scale * (1.0 + fraction), scale * (1.0 + fraction + step))
    }
}

/// The values the tally bin `key` spans, `low..high`, in value order.
fn key_edges(key: usize) -> (f32, f32) {
    if key >= MAGNITUDES {
        magnitude_edges(key - MAGNITUDES)
    } else {
        let (low, high) = magnitude_edges(MAGNITUDES - 1 - key);
        (-high, -low)
    }
}

/// The counts of one quantity over every pixel: a bin to the code for an
/// integer channel as stored, which is exact, and a bin to every
/// [`MANTISSA_BITS`] of an octave for a float — a stored float channel, a
/// luminance, a channel the working space has moved — with the exact
/// extremes kept beside the bins.
#[derive(Clone, Debug)]
enum Tally {
    Codes {
        /// A count to the code.
        counts: Vec<u32>,
        /// The code that is full scale, which a count is divided by to be a
        /// fraction of it.
        full: f32,
    },
    Keys {
        /// A count to the [`key_of`] bin.
        counts: Vec<u32>,
        min: f32,
        max: f32,
    },
}

/// One bin of a tally with something in it: the values it spans, the one
/// value it stands for — a code's own, or the middle of a float bin — and
/// how many pixels are in it.
#[derive(Clone, Copy, Debug)]
struct Bin {
    low: f32,
    high: f32,
    value: f32,
    count: u32,
}

impl Tally {
    fn codes(full: f32, codes: usize) -> Self {
        Tally::Codes {
            counts: vec![0; codes],
            full,
        }
    }

    fn keys() -> Self {
        Tally::Keys {
            counts: vec![0; KEY_BINS],
            min: f32::INFINITY,
            max: f32::NEG_INFINITY,
        }
    }

    fn add(&mut self, other: &Self) {
        fn add_counts(into: &mut [u32], from: &[u32]) {
            for (sum, count) in into.iter_mut().zip(from) {
                *sum += count;
            }
        }
        match (&mut *self, other) {
            (Tally::Codes { counts, .. }, Tally::Codes { counts: from, .. }) => {
                add_counts(counts, from);
            }
            (
                Tally::Keys { counts, min, max },
                Tally::Keys {
                    counts: from,
                    min: low,
                    max: high,
                },
            ) => {
                add_counts(counts, from);
                *min = min.min(*low);
                *max = max.max(*high);
            }
            _ => unreachable!("a tally is merged with one of its own kind"),
        }
    }

    /// Every pixel counted.
    fn total(&self) -> u64 {
        let counts = match self {
            Tally::Codes { counts, .. } | Tally::Keys { counts, .. } => counts,
        };
        counts.iter().map(|&count| u64::from(count)).sum()
    }

    /// The smallest and largest value counted, in the tally's own units — a
    /// fraction of full scale for codes — and infinities either way round
    /// where nothing was.
    fn extremes(&self) -> (f32, f32) {
        match self {
            Tally::Codes { counts, full } => {
                let first = counts.iter().position(|&count| count > 0);
                let last = counts.iter().rposition(|&count| count > 0);
                match (first, last) {
                    (Some(first), Some(last)) => (first as f32 / full, last as f32 / full),
                    _ => (f32::INFINITY, f32::NEG_INFINITY),
                }
            }
            Tally::Keys { min, max, .. } => (*min, *max),
        }
    }

    /// The bins with anything in them, in value order.
    fn bins(&self) -> impl Iterator<Item = Bin> + '_ {
        /// The values a bin spans and the one it stands for, by its index.
        type Edges<'a> = Box<dyn Fn(usize) -> (f32, f32, f32) + 'a>;
        let (counts, edges): (&[u32], Edges) = match self {
            Tally::Codes { counts, full } => (
                counts,
                Box::new(move |code| {
                    let value = code as f32 / full;
                    let half = 0.5 / full;
                    (value - half, value + half, value)
                }),
            ),
            Tally::Keys { counts, min, max } => (
                counts,
                Box::new(move |key| {
                    // The bins at the ends reach to the exact extremes rather
                    // than to the ends of the float's range.
                    let (low, high) = key_edges(key);
                    let (low, high) = (low.max(*min), high.min(*max).max(low));
                    (low, high, (low + high) / 2.0)
                }),
            ),
        };
        counts
            .iter()
            .enumerate()
            .filter(|(_, count)| **count > 0)
            .map(move |(index, &count)| {
                let (low, high, value) = edges(index);
                Bin {
                    low,
                    high,
                    value,
                    count,
                }
            })
    }

    /// The value below which `fraction` of the pixels fall, `fraction` in
    /// 0..=1, in the tally's own units: the low edge of the bin the
    /// `fraction`th pixel is in, held within the extremes. Returns the
    /// extremes themselves at the ends.
    fn percentile(&self, fraction: f32) -> f32 {
        let total = self.total();
        let (min, max) = self.extremes();
        // Written so that extremes that are not numbers fail the test too.
        if total == 0 || max.partial_cmp(&min) != Some(std::cmp::Ordering::Greater) {
            return if fraction <= 0.0 { min } else { max };
        }
        let target = (f64::from(fraction.clamp(0.0, 1.0)) * total as f64) as u64;
        let mut running = 0u64;
        for bin in self.bins() {
            running += u64::from(bin.count);
            if running >= target {
                return match self {
                    Tally::Codes { .. } => bin.value,
                    Tally::Keys { .. } => bin.low.clamp(min, max),
                };
            }
        }
        max
    }

    /// The counts, a bin to the code or to the [`key_of`] bin, in value
    /// order.
    fn counts(&self) -> &[u32] {
        match self {
            Tally::Codes { counts, .. } | Tally::Keys { counts, .. } => counts,
        }
    }

    /// The bin `value`, in the tally's own units, is counted in, or `None`
    /// for a value that is not a number.
    fn index_of(&self, value: f32) -> Option<usize> {
        if value.is_nan() {
            return None;
        }
        Some(match self {
            Tally::Codes { counts, full } => {
                (value * full).round().clamp(0.0, (counts.len() - 1) as f32) as usize
            }
            Tally::Keys { .. } => key_of(value),
        })
    }

    /// The one value the bin `index` stands for, in the tally's own units:
    /// a code's own, or the middle of a float bin held within the exact
    /// extremes, as [`Tally::bins`] has it.
    fn value_at(&self, index: usize) -> f32 {
        match self {
            Tally::Codes { full, .. } => index as f32 / full,
            Tally::Keys { min, max, .. } => {
                let (low, high) = key_edges(index);
                let (low, high) = (low.max(*min), high.min(*max).max(low));
                (low + high) / 2.0
            }
        }
    }

    /// The share of the pixels in the bins `range`.
    fn share_in(&self, range: std::ops::Range<usize>) -> u64 {
        self.counts()[range]
            .iter()
            .map(|&count| u64::from(count))
            .sum()
    }

    /// Where `value`, in the tally's own units, stands among the pixels, as a
    /// fraction of them: those below it and half of those in its own bin,
    /// so that a value every pixel shares stands in the middle rather than
    /// at either end. `None` where nothing was counted, or for a value that
    /// is not a number.
    fn rank(&self, value: f32) -> Option<f32> {
        let at = self.index_of(value)?;
        let total = self.total();
        if total == 0 {
            return None;
        }
        let below = self.share_in(0..at);
        let within = self.share_in(at..at + 1);
        Some(((below as f64 + within as f64 / 2.0) / total as f64) as f32)
    }

    /// Where `out` stands among what `through` makes of each bin's value,
    /// as a fraction of the pixels: those it puts below `out` and half of
    /// those it puts exactly at it, so that a pile `through` clips together
    /// stands in its middle. `through` takes a value in the tally's own
    /// units and must never fall, so that the bins below `out` are one run
    /// of them from the bottom, found by halving rather than by putting
    /// every bin through it. `None` where nothing was counted, or for an
    /// `out` that is not a number.
    fn rank_through(&self, out: f32, through: impl Fn(f32) -> f32) -> Option<f32> {
        let total = self.total();
        if total == 0 || out.is_nan() {
            return None;
        }
        let bins = self.counts().len();
        let at = |index: usize| through(self.value_at(index));
        // The first bin from which `holds` is true, `holds` being false and
        // then true along the bins.
        let first = |holds: &dyn Fn(usize) -> bool| {
            let (mut low, mut high) = (0, bins);
            while low < high {
                let middle = (low + high) / 2;
                if holds(middle) {
                    high = middle;
                } else {
                    low = middle + 1;
                }
            }
            low
        };
        let reaching = first(&|index| at(index) >= out);
        let past = first(&|index| at(index) > out);
        let (below, within) = (self.share_in(0..reaching), self.share_in(reaching..past));
        Some(((below as f64 + within as f64 / 2.0) / total as f64) as f32)
    }

    /// Takes every pixel whose value `decode` makes `sentinel` of out of a
    /// tally of codes: the no-data value, which is no measurement.
    fn without(&mut self, sentinel: f32, decode: impl Fn(f32) -> f32) {
        if let Tally::Codes { counts, full } = self {
            for (code, count) in counts.iter_mut().enumerate() {
                if decode(code as f32 / *full) == sentinel {
                    *count = 0;
                }
            }
        }
    }

    /// The counts of `self` with `other`'s taken off, bin for bin: the lit
    /// pixels, where `other` is the tally of the transparent ones.
    fn less(&self, other: &Self) -> Self {
        let mut left = self.clone();
        match (&mut left, other) {
            (Tally::Codes { counts, .. }, Tally::Codes { counts: from, .. })
            | (Tally::Keys { counts, .. }, Tally::Keys { counts: from, .. }) => {
                for (count, taken) in counts.iter_mut().zip(from) {
                    *count = count.saturating_sub(*taken);
                }
            }
            _ => unreachable!("a tally is taken off one of its own kind"),
        }
        left
    }

    /// The tally's pixels poured into `bins` sample points over `range` on
    /// an axis `place` puts a value on: a code wholly into the bin nearest
    /// its own place, as the scan always binned a sample, and a float bin
    /// spread over the stretch its edges come to, whole pixels at a time.
    /// What lands off either end piles into the end bin, as a clamp would
    /// have put it. A value `place` makes no number of is left out.
    fn pour_whole(&self, into: &mut [u32], range: [f32; 2], place: impl Fn(f32) -> f32) {
        let [low, high] = range;
        let span = high - low;
        if !(span.is_finite() && span > f32::MIN_POSITIVE) {
            return;
        }
        let last = (into.len() - 1) as f32;
        let scale = last / span;
        let at = |value: f32| (place(value) - low) * scale;
        for bin in self.bins() {
            match self {
                Tally::Codes { .. } => {
                    let middle = at(bin.value);
                    if middle.is_nan() {
                        continue;
                    }
                    into[(middle + 0.5).floor().clamp(0.0, last) as usize] += bin.count;
                }
                Tally::Keys { .. } => {
                    let (from, to) = (at(bin.low), at(bin.high));
                    if from.is_nan() || to.is_nan() {
                        continue;
                    }
                    let (from, to) = (from.min(to), from.max(to));
                    if !(to - from).is_finite() {
                        // Off an end entirely, as nothing is on a stops axis.
                        let middle = if from.is_finite() { from } else { to };
                        into[middle.clamp(0.0, last).round() as usize] += bin.count;
                        continue;
                    }
                    spread_whole(into, from, to, bin.count);
                }
            }
        }
    }

    /// The tally's pixels poured into [`BINS`] over `range` on an axis
    /// `place` puts a value on, in [`SHARES`] of a pixel, every bin spread
    /// over the stretch its edges come to — a code over the stretch to
    /// halfway to its neighbors, so that codes on an axis wider than a bin
    /// to the code do not comb, as [`Fine::rebinned`] spreads them — and
    /// never narrower than a bin. What lands off either end piles into it.
    fn pour_shares(&self, range: [f32; 2], place: impl Fn(f32) -> f32) -> [u32; BINS] {
        let mut spread = [0.0f64; BINS];
        let [low, high] = range;
        let span = high - low;
        if !(span.is_finite() && span > f32::MIN_POSITIVE) {
            return [0; BINS];
        }
        let last = (BINS - 1) as f32;
        let scale = last / span;
        let at = |value: f32| (place(value) - low) * scale;
        for bin in self.bins() {
            let middle = at(bin.value);
            if middle.is_nan() {
                continue;
            }
            let (below, above) = (at(bin.low), at(bin.high));
            let (mut from, mut to) = (below.min(above).min(middle), below.max(above).max(middle));
            if !(to - from).is_finite() {
                let bin_at = middle.clamp(0.0, last).round();
                (from, to) = (bin_at - 0.5, bin_at + 0.5);
            } else if to - from < 1.0 {
                // Never narrower than a bin: a code or a float bin finer
                // than the plot stands a bin wide where it lands, as a
                // sample point does.
                let widen = (1.0 - (to - from)) / 2.0;
                (from, to) = (from - widen, to + widen);
            }
            spread_over(&mut spread, from, to, f64::from(bin.count));
        }
        spread.map(|count| (count * SHARES).round() as u32)
    }
}

/// The lit pixels of a luminance tally — those that measured light, above
/// zero and not transparent — bin by bin in value order, each with the
/// stops its low edge stands at, and what they come to.
struct Lit {
    bins: Vec<LitBin>,
    total: u64,
}

struct LitBin {
    /// The low edge of the bin, in stops.
    low: f32,
    /// The mean logarithm of the bin: of its one value for a code, and of
    /// the middle of its stretch in stops for a float bin.
    log: f64,
    count: u64,
}

impl Lit {
    /// The lit bins of `luma`, whose values `decode` takes to linear light,
    /// with the transparent pixels `clear` counts taken off where there are
    /// any. Nothing below the tally's lowest octave is lit: zero is there,
    /// and a value under one part in four billion measured as good as none.
    fn of(luma: &Tally, clear: Option<&Tally>, decode: impl Fn(f32) -> f32) -> Self {
        let lit = match clear {
            Some(clear) => luma.less(clear),
            None => luma.clone(),
        };
        let mut bins = Vec::new();
        let mut total = 0u64;
        for bin in lit.bins() {
            let (low, high) = (decode(bin.low), decode(bin.high));
            let value = decode(bin.value);
            let is_lit = match &lit {
                Tally::Codes { .. } => value > 0.0,
                Tally::Keys { .. } => low > 0.0,
            };
            if !is_lit {
                continue;
            }
            let log = match &lit {
                Tally::Codes { .. } => f64::from(value.log2()),
                Tally::Keys { .. } => (f64::from(low.log2()) + f64::from(high.log2())) / 2.0,
            };
            total += u64::from(bin.count);
            bins.push(LitBin {
                low: match &lit {
                    Tally::Codes { .. } => value.log2(),
                    Tally::Keys { .. } => low.log2(),
                },
                log,
                count: u64::from(bin.count),
            });
        }
        Self { bins, total }
    }

    /// The mean of the logarithms between the two trims: the darkest
    /// [`KEY_TRIM`] of the pixels and the brightest are left out, by count,
    /// and a bin the trim falls inside contributes the share of itself that
    /// is kept, taken at the bin's own mean. `None` where nothing was lit.
    fn trimmed_log_mean(&self) -> Option<f64> {
        if self.total == 0 {
            return None;
        }
        let total = self.total as f64;
        let (keep_from, keep_to) = (total * KEY_TRIM, total * (1.0 - KEY_TRIM));
        let mut below = 0.0;
        let mut kept_sum = 0.0;
        let mut kept_count = 0.0;
        for bin in &self.bins {
            let count = bin.count as f64;
            let above = below + count;
            let kept = (above.min(keep_to) - below.max(keep_from)).max(0.0);
            if kept > 0.0 {
                kept_sum += bin.log * kept;
                kept_count += kept;
            }
            below = above;
        }
        // The trims leave nine tenths of the pixels between them whatever the
        // count, so there is always something kept.
        Some(kept_sum / kept_count)
    }
}

// ---------------------------------------------------------------------------
// The pass.

/// A sample as a file stores it, read in the one pass over the pixels: a
/// code for an integer, and the value itself for a float.
trait Raw: Copy + Send + Sync {
    /// Whether the samples are whole numbers, and so have codes.
    const INTEGER: bool;
    fn code(self) -> usize;
    fn value(self) -> f32;
}

impl Raw for u8 {
    const INTEGER: bool = true;
    #[inline]
    fn code(self) -> usize {
        usize::from(self)
    }
    #[inline]
    fn value(self) -> f32 {
        f32::from(self)
    }
}

impl Raw for u16 {
    const INTEGER: bool = true;
    #[inline]
    fn code(self) -> usize {
        usize::from(self)
    }
    #[inline]
    fn value(self) -> f32 {
        f32::from(self)
    }
}

impl Raw for f32 {
    const INTEGER: bool = false;
    #[inline]
    fn code(self) -> usize {
        0
    }
    #[inline]
    fn value(self) -> f32 {
        self
    }
}

/// What a pass does with one pixel, given its components as stored, decoded
/// to linear in the file's own primaries, and as the working space has
/// them — lifted and carried into BT.709, which is `linear` itself where
/// nothing moves it — and whether the pixel is transparent.
trait Visit<R: Raw>: Send {
    fn pixel(&mut self, x: u32, y: u32, raw: &[R], linear: &[f32], shown: &[f32], clear: bool);
    fn merge(self, other: Self) -> Self;
}

/// The pipeline every pixel is read through, set up once for the pass: the
/// curve as a table over every code for integer samples, the lift and the
/// matrix where the picture has them. The CPU twin of what the shader does
/// to a texel before the window goes on, and of [`super::Reader::decode`],
/// which reads one pixel the same way; a test holds the two together.
struct Pass<'a> {
    image: &'a DecodedImage,
    channels: Channels,
    count: usize,
    colors: usize,
    alpha: Option<usize>,
    transfer: Transfer,
    /// Each code decoded, for integer samples.
    table: Vec<f32>,
    lift: Option<(&'a GainMap, &'a Table)>,
    matrix: Option<[[f32; 3]; 3]>,
    nodata: Option<f32>,
}

impl<'a> Pass<'a> {
    fn new(image: &'a DecodedImage, lift: Option<&'a Table>) -> Self {
        let channels = image.channels();
        let transfer = image.color.transfer;
        let full = image.samples.full_scale();
        let table = match image.samples {
            Samples::U8 { .. } => (0..=u8::MAX as usize)
                .map(|code| transfer.to_linear(code as f32 / full))
                .collect(),
            Samples::U16 { .. } => (0..=u16::MAX as usize)
                .map(|code| transfer.to_linear(code as f32 / full))
                .collect(),
            Samples::F32 { .. } => Vec::new(),
        };
        let primaries = image.color.primaries;
        Self {
            image,
            channels,
            count: channels.count(),
            colors: channels.color_count(),
            alpha: channels.alpha_index(),
            transfer,
            table,
            lift: lift.and_then(|table| Some((image.gain_map.as_deref()?, table))),
            matrix: (!channels.is_gray() && primaries != Primaries::Bt709)
                .then(|| primaries.to_bt709()),
            nodata: image.nodata,
        }
    }

    /// Whether the working space has the color anywhere but where the curve
    /// alone would put it: a lift, or a matrix.
    fn moves(&self) -> bool {
        self.lift.is_some() || self.matrix.is_some()
    }

    #[inline]
    fn is_data(&self, value: f32) -> bool {
        value.is_finite() && self.nodata.is_none_or(|sentinel| value != sentinel)
    }

    /// The rows `rows` of `data`, each pixel handed to `visit`.
    fn walk<R: Raw, V: Visit<R>>(&self, data: &[R], rows: std::ops::Range<u32>, visit: &mut V) {
        let image = self.image;
        let (width, height) = (image.width, image.height);
        let count = self.count;
        let colors = self.colors;
        let transfer = self.transfer;
        let linear_transfer = transfer.is_linear();
        let table = &self.table[..];
        let full = image.samples.full_scale();
        let mut linear = [0.0f32; 4];
        let mut shown = [0.0f32; 4];
        for y in rows {
            let row = y as usize * width as usize * count;
            for x in 0..width {
                let base = row + x as usize * count;
                let raw = &data[base..base + count];
                if R::INTEGER {
                    for (slot, sample) in linear.iter_mut().zip(raw) {
                        *slot = table[sample.code()];
                    }
                } else if linear_transfer {
                    for (slot, sample) in linear.iter_mut().zip(raw) {
                        *slot = sample.value();
                    }
                } else {
                    for (slot, sample) in linear.iter_mut().zip(raw) {
                        *slot = transfer.to_linear(sample.value());
                    }
                }
                let clear = self.alpha.is_some_and(|index| {
                    let alpha = if R::INTEGER {
                        raw[index].code() as f32 / full
                    } else {
                        raw[index].value()
                    };
                    // Nothing, less than nothing, or not a number: none of
                    // them is any coverage.
                    alpha.partial_cmp(&0.0) != Some(std::cmp::Ordering::Greater)
                });
                shown[..count].copy_from_slice(&linear[..count]);
                // The lift, in the base's own color space, before anything
                // else, then the primaries: the order the shader takes.
                if let Some((map, lift)) = self.lift {
                    let gain = map.gain_at(lift, x, y, width, height);
                    lift.apply(&mut shown[..colors], gain);
                }
                if let Some(matrix) = self.matrix {
                    let color = [shown[0], shown[1], shown[2]];
                    for (slot, row) in shown.iter_mut().zip(matrix) {
                        *slot = row[0] * color[0] + row[1] * color[1] + row[2] * color[2];
                    }
                }
                visit.pixel(x, y, raw, &linear[..count], &shown[..count], clear);
            }
        }
    }

    /// The whole picture walked for `visit`, the rows divided between
    /// rayon's threads and each band's visitor merged into one. Every pixel
    /// is counted on its own, so a band's numbers depend on nothing outside
    /// it, and the bands fold together into exactly what one thread would
    /// have counted.
    fn run<V>(&self, make: impl Fn() -> V + Sync) -> V
    where
        V: Visit<u8> + Visit<u16> + Visit<f32>,
    {
        match &self.image.samples {
            Samples::U8 { data, .. } => self.run_on::<u8, V>(data, make),
            Samples::U16 { data, .. } => self.run_on::<u16, V>(data, make),
            Samples::F32 { data, .. } => self.run_on::<f32, V>(data, make),
        }
    }

    fn run_on<R: Raw, V: Visit<R>>(&self, data: &[R], make: impl Fn() -> V + Sync) -> V {
        let height = self.image.height;
        let bands = bands_for(self.image.width, height);
        if bands <= 1 {
            let mut visitor = make();
            self.walk(data, 0..height, &mut visitor);
            return visitor;
        }
        let rows = height.div_ceil(bands);
        let bands = height.div_ceil(rows);
        let mut results: Vec<Option<V>> = (0..bands).map(|_| None).collect();
        rayon_core::scope(|scope| {
            for (index, slot) in results.iter_mut().enumerate() {
                let first = index as u32 * rows;
                let last = (first + rows).min(height);
                let make = &make;
                scope.spawn(move |_| {
                    let mut visitor = make();
                    self.walk(data, first..last, &mut visitor);
                    *slot = Some(visitor);
                });
            }
        });
        results
            .into_iter()
            .map(|result| result.expect("every band was walked"))
            .reduce(V::merge)
            .expect("at least one band")
    }
}

/// Pixels below which a pass is not worth dividing: handing bands to the
/// pool costs more than a picture this small takes to walk.
const PARALLEL_FROM: usize = 1 << 16;

/// The fewest pixels a band is given, so that a picture just over
/// [`PARALLEL_FROM`] goes to two or three threads rather than thirty-two.
const BAND_PIXELS: usize = 1 << 15;

/// How many ways to cut the walk: one for a picture too small to be worth
/// the threads, otherwise one per thread the pool will run at once, and
/// never more than there are rows to give them.
fn bands_for(width: u32, height: u32) -> u32 {
    let pixels = width as usize * height as usize;
    if pixels < PARALLEL_FROM {
        return 1;
    }
    (pixels / BAND_PIXELS)
        .clamp(1, rayon_core::current_num_threads().max(1))
        .min(height as usize) as u32
}

/// Everything the first pass counts: the stored channels, the luminance,
/// the channels as the working space has them where it moves them, and the
/// extremes.
struct Tallies {
    /// The color channels as stored, one each.
    stored: Vec<Tally>,
    /// The luminance in linear light, as the working space has it: `None`
    /// on a gray picture nothing moves, whose luminance is its one stored
    /// channel.
    luma: Option<Tally>,
    /// The luminance of the transparent pixels alone, on a picture with
    /// alpha: taken off `luma` — or off the stored channel standing for it
    /// — for what was lit. Of the same kind as the tally it is taken off.
    clear: Option<Tally>,
    /// The three channels as the working space has them, on a color picture
    /// it moves.
    planes: Option<[Tally; COLOR]>,
    /// How many pixels measured a luminance, and its extremes, where `luma`
    /// is tallied; and the highest any plotted channel reaches in the
    /// working space.
    counted: u64,
    min: f32,
    max: f32,
    peak: f32,
}

impl Tallies {
    fn new(pass: &Pass) -> Self {
        let image = pass.image;
        let moves = pass.moves();
        let gray = pass.channels.is_gray();
        let tally = || match &image.samples {
            Samples::U8 { .. } => Tally::codes(f32::from(u8::MAX), 1 << 8),
            Samples::U16 { .. } => Tally::codes(f32::from(u16::MAX), 1 << 16),
            Samples::F32 { .. } => Tally::keys(),
        };
        let luma = (!gray || moves).then(Tally::keys);
        let clear = pass.alpha.map(|_| match &luma {
            Some(_) => Tally::keys(),
            None => tally(),
        });
        Self {
            stored: (0..pass.colors).map(|_| tally()).collect(),
            luma,
            clear,
            planes: (!gray && moves).then(|| std::array::from_fn(|_| Tally::keys())),
            counted: 0,
            min: f32::INFINITY,
            max: f32::NEG_INFINITY,
            peak: f32::NEG_INFINITY,
        }
    }
}

/// The pass's visitor: what it needs of the pass to count, and what it has
/// counted.
struct Counting<'a> {
    pass: &'a Pass<'a>,
    tallies: Tallies,
}

#[inline]
fn add_key(tally: &mut Tally, value: f32) {
    if let Tally::Keys { counts, min, max } = tally {
        counts[key_of(value)] += 1;
        *min = min.min(value);
        *max = max.max(value);
    }
}

#[inline]
fn add_code(tally: &mut Tally, code: usize) {
    if let Tally::Codes { counts, .. } = tally {
        counts[code] += 1;
    }
}

impl<R: Raw> Visit<R> for Counting<'_> {
    #[inline]
    fn pixel(&mut self, _x: u32, _y: u32, raw: &[R], linear: &[f32], shown: &[f32], clear: bool) {
        let pass = self.pass;
        let tallies = &mut self.tallies;
        let colors = pass.colors;
        // The channels as stored. An integer's code goes in whatever it
        // decodes to — the no-data code is taken out afterwards — where a
        // float is checked as it goes.
        if R::INTEGER {
            for (tally, sample) in tallies.stored.iter_mut().zip(raw) {
                add_code(tally, sample.code());
            }
        } else {
            for (channel, (tally, sample)) in tallies.stored.iter_mut().zip(raw).enumerate() {
                if pass.is_data(linear[channel]) {
                    add_key(tally, sample.value());
                }
            }
        }
        // The channels as the working space has them, where it moves them.
        if let Some(planes) = tallies.planes.as_mut() {
            for (plane, &value) in planes.iter_mut().zip(shown) {
                if pass.is_data(value) {
                    add_key(plane, value);
                }
            }
        }
        match tallies.luma.as_mut() {
            Some(luma) => {
                for &value in &shown[..colors] {
                    if pass.is_data(value) {
                        tallies.peak = tallies.peak.max(value);
                    }
                }
                let value = luminance(shown, pass.channels);
                if pass.is_data(value) {
                    tallies.counted += 1;
                    tallies.min = tallies.min.min(value);
                    tallies.max = tallies.max.max(value);
                    add_key(luma, value);
                    if clear && let Some(tally) = tallies.clear.as_mut() {
                        add_key(tally, value);
                    }
                }
            }
            // A gray picture nothing moves: its stored channel is its
            // luminance, and only the transparent pixels need counting
            // beside it.
            None => {
                if clear && let Some(tally) = tallies.clear.as_mut() {
                    if R::INTEGER {
                        add_code(tally, raw[0].code());
                    } else if pass.is_data(linear[0]) {
                        add_key(tally, raw[0].value());
                    }
                }
            }
        }
    }

    fn merge(mut self, other: Self) -> Self {
        let (mine, theirs) = (&mut self.tallies, other.tallies);
        for (tally, other) in mine.stored.iter_mut().zip(&theirs.stored) {
            tally.add(other);
        }
        if let (Some(tally), Some(other)) = (mine.luma.as_mut(), &theirs.luma) {
            tally.add(other);
        }
        if let (Some(tally), Some(other)) = (mine.clear.as_mut(), &theirs.clear) {
            tally.add(other);
        }
        if let (Some(planes), Some(others)) = (mine.planes.as_mut(), &theirs.planes) {
            for (plane, other) in planes.iter_mut().zip(others) {
                plane.add(other);
            }
        }
        mine.counted += theirs.counted;
        mine.min = mine.min.min(theirs.min);
        mine.max = mine.max.max(theirs.max);
        mine.peak = mine.peak.max(theirs.peak);
        self
    }
}

/// The second pass, for floats on a linear axis: each plotted plane
/// counted into [`FINE_BINS`] over the axis it was given, a pixel wholly
/// into the bin nearest its value, as the fine bins of every other picture
/// come out. A float tally's bins are a thousandth of an octave wide, which
/// over a measurement standing well off zero — temperatures in kelvin,
/// heights above the sea — is wider than a fine bin over the range it
/// spans, and a plot poured from them would comb; these are the fine bins
/// counted from the pixels instead.
struct Binning<'a> {
    pass: &'a Pass<'a>,
    /// The luminance's axis, and its bins.
    axis: Axis,
    range: [f32; 2],
    luma: Option<Vec<u32>>,
    /// The plotted color planes on the luminance's axis, where the picture
    /// has them: the channels as the working space has them.
    planes: Option<[Vec<u32>; COLOR]>,
    /// Each stored channel on its own range, as the Image section plots it.
    stored: Vec<([f32; 2], Vec<u32>)>,
}

impl Binning<'_> {
    #[inline]
    fn bin(range: [f32; 2], at: f32) -> Option<usize> {
        let [low, high] = range;
        let position = (at - low) * ((FINE_BINS - 1) as f32 / (high - low)) + 0.5;
        if position.is_nan() {
            return None;
        }
        Some(position.clamp(0.0, (FINE_BINS - 1) as f32) as usize)
    }
}

impl<R: Raw> Visit<R> for Binning<'_> {
    #[inline]
    fn pixel(&mut self, _x: u32, _y: u32, raw: &[R], linear: &[f32], shown: &[f32], _clear: bool) {
        let pass = self.pass;
        let axis = self.axis;
        for (channel, (range, bins)) in self.stored.iter_mut().enumerate() {
            if pass.is_data(linear[channel])
                && let Some(bin) = Self::bin(*range, axis.place_stored(raw[channel].value()))
            {
                bins[bin] += 1;
            }
        }
        if let Some(planes) = self.planes.as_mut() {
            for (bins, &value) in planes.iter_mut().zip(shown) {
                if pass.is_data(value)
                    && let Some(bin) = Self::bin(self.range, axis.place(value))
                {
                    bins[bin] += 1;
                }
            }
        }
        if let Some(bins) = self.luma.as_mut() {
            let value = luminance(shown, pass.channels);
            if pass.is_data(value)
                && let Some(bin) = Self::bin(self.range, axis.place(value))
            {
                bins[bin] += 1;
            }
        }
    }

    fn merge(mut self, other: Self) -> Self {
        fn add(into: &mut [u32], from: &[u32]) {
            for (sum, count) in into.iter_mut().zip(from) {
                *sum += count;
            }
        }
        if let (Some(mine), Some(theirs)) = (self.luma.as_mut(), &other.luma) {
            add(mine, theirs);
        }
        if let (Some(mine), Some(theirs)) = (self.planes.as_mut(), &other.planes) {
            for (mine, theirs) in mine.iter_mut().zip(theirs) {
                add(mine, theirs);
            }
        }
        for ((_, mine), (_, theirs)) in self.stored.iter_mut().zip(&other.stored) {
            add(mine, theirs);
        }
        self
    }
}

/// What the one pass over the pixels counted, with the no-data code taken
/// out, from which [`Stats`] and [`Stored`] are both read.
struct Scan<'a> {
    pass: Pass<'a>,
    tallies: Tallies,
    /// The stored channels' tallies, taken out of `tallies` once counted
    /// and shared: the Image section ranks against them, and so does the
    /// Display section where nothing moves the channels.
    stored: Vec<Arc<Tally>>,
}

impl<'a> Scan<'a> {
    fn of(image: &'a DecodedImage, lift: Option<&'a Table>) -> Self {
        let pass = Pass::new(image, lift);
        let counting = pass.run(|| Counting {
            pass: &pass,
            tallies: Tallies::new(&pass),
        });
        let mut tallies = counting.tallies;
        if let Some(sentinel) = pass.nodata {
            let transfer = pass.transfer;
            for tally in &mut tallies.stored {
                tally.without(sentinel, |fraction| transfer.to_linear(fraction));
            }
            if let Some(clear) = tallies.clear.as_mut() {
                clear.without(sentinel, |fraction| transfer.to_linear(fraction));
            }
        }
        let stored = std::mem::take(&mut tallies.stored)
            .into_iter()
            .map(Arc::new)
            .collect();
        Self {
            pass,
            tallies,
            stored,
        }
    }

    /// Whether the samples are floats, which have no codes.
    fn floats(&self) -> bool {
        matches!(self.pass.image.samples, Samples::F32 { .. })
    }

    /// Whether a second pass counts the fine bins from the pixels rather
    /// than from the tallies: floats on an axis that is not in stops.
    fn rebins(&self, axis: Axis) -> bool {
        self.floats() && axis != Axis::Stops
    }
}

// ---------------------------------------------------------------------------
// Stats: the picture as the display starts from it.

#[derive(Clone, Debug)]
pub struct Stats {
    /// Smallest and largest linear value seen, in working-space units.
    pub min: f32,
    pub max: f32,
    /// The largest value any plotted channel reaches, in working-space
    /// units: the luminance's `max` on a gray image, and on a color one the
    /// channel that would be first to clip. A pure red is a fifth of white
    /// in luminance while its red channel is at white, and a P3 red carried
    /// into BT.709 is past it — which is what a surface that stops at white
    /// clips, and what [`super::display::Display::exceeds_white`] asks.
    pub peak: f32,
    /// How many pixels measured a luminance: every one that is a number
    /// and not the no-data value.
    pub counted: u64,
    /// The key of the scene, in the file's own units: the geometric mean of
    /// the luminance — the mean of its logarithm, taken back out — over the
    /// pixels that measured any light at all, less the darkest and the
    /// brightest [`KEY_TRIM`] of them. What a meter puts at middle gray.
    /// Pixels at zero are left out rather than floored: a render's black
    /// background and the transparent surround of a premultiplied element
    /// measured nothing, and a floor under them would set the exposure by
    /// how much of the frame they cover. `None` where no pixel is above
    /// zero.
    pub key: Option<f32>,
    /// How the plot and the fine bins are spaced: what a position on
    /// their axis stands for.
    pub axis: Axis,
    /// The same pixels binned for drawing. See [`Plot`] for why it is on
    /// another axis than the extremes.
    pub plot: Plot,
    /// The same pixels on the same axis in finer bins, for what a stage
    /// after the file makes of them. Shared, since a clone of the stats is
    /// made at every arrival and is otherwise a copy of 64 KiB; what is
    /// rebinned from it is cached by the address of this.
    pub fine: Arc<Fine>,
    /// The luminance tallied, for the percentiles and the ranks. Shared
    /// for the same reason.
    luma: Ranked,
    /// Each color channel as plotted, tallied, for the ranks: `None` on a
    /// gray picture, which plots its luminance alone.
    planes: Option<[Ranked; COLOR]>,
}

/// One of the quantities the Display plot draws, to rank a value among.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Quantity {
    Luminance,
    /// Red, green or blue, as the working space has them.
    Channel(usize),
}

/// A tally of one of [`Stats`]' quantities, and where its values are not in
/// linear light, the curve they are on: a stored channel's codes, or a gray
/// picture's.
#[derive(Clone, Debug)]
struct Ranked {
    tally: Arc<Tally>,
    curve: Option<Transfer>,
}

impl Ranked {
    /// A linear value in the tally's own units.
    fn units(&self, linear: f32) -> f32 {
        match self.curve {
            Some(transfer) => encode(transfer, linear),
            None => linear,
        }
    }

    /// A value in the tally's own units in linear light.
    fn linear(&self, units: f32) -> f32 {
        match self.curve {
            Some(transfer) => transfer.to_linear(units),
            None => units,
        }
    }
}

impl Stats {
    /// Scans `image`, converting to the linear working space as it goes —
    /// the transfer function, the lift and the primaries matrix — so that
    /// the numbers line up with what the shader has in hand when it applies
    /// the window.
    ///
    /// Gray images are measured on their single channel; color images on
    /// relative luminance, which is what an exposure control should track.
    /// Color images additionally get per-channel counts, which the UI draws
    /// as a four-channel histogram.
    pub fn scan(image: &DecodedImage) -> Self {
        Self::scan_with(image, None)
    }

    /// [`Stats::scan`], with the picture lifted through `lift` where it has
    /// a gain map: what the screen shows on a surface with room above
    /// white, where the base is what it shows on one without. Every pixel's
    /// luminance and channels are read through the lift, so the extremes,
    /// the key and the plot are of the lifted picture.
    pub fn scan_with(image: &DecodedImage, lift: Option<&Table>) -> Self {
        Self::from_scan(&Scan::of(image, lift))
    }

    /// [`Stats::scan`] and the [`Stored`] from the one pass over the
    /// pixels, which is what a file arriving asks for.
    pub fn scan_both(image: &DecodedImage) -> (Self, Stored) {
        let scan = Scan::of(image, None);
        (Self::from_scan(&scan), Stored::from_scan(&scan))
    }

    fn from_scan(scan: &Scan) -> Self {
        let pass = &scan.pass;
        let image = pass.image;
        let transfer = pass.transfer;
        let gray = pass.channels.is_gray();
        let moves = pass.moves();
        let tallies = &scan.tallies;
        let color = !gray;

        // The luminance: tallied in linear light, or a gray picture's one
        // stored channel, decoded.
        let (luma, luma_curve) = match &tallies.luma {
            Some(luma) => (luma, None),
            None => (&*scan.stored[0], Some(transfer)),
        };
        let decode = |value: f32| match luma_curve {
            Some(transfer) => transfer.to_linear(value),
            None => value,
        };
        // The same, and each plotted plane, kept to rank a value against:
        // the stored channels shared with [`Stored`] where they are what
        // is plotted, so that the one count is not held twice.
        let ranked_luma = Ranked {
            tally: match &tallies.luma {
                Some(luma) => Arc::new(luma.clone()),
                None => Arc::clone(&scan.stored[0]),
            },
            curve: luma_curve,
        };
        let ranked_planes: Option<[Ranked; COLOR]> = match &tallies.planes {
            Some(planes) => Some(planes.clone().map(|plane| Ranked {
                tally: Arc::new(plane),
                curve: None,
            })),
            None if color => Some(std::array::from_fn(|channel| Ranked {
                tally: Arc::clone(&scan.stored[channel]),
                curve: Some(transfer),
            })),
            None => None,
        };
        let (min, max, counted) = match &tallies.luma {
            Some(_) => (tallies.min, tallies.max, tallies.counted),
            None => {
                let (min, max) = luma.extremes();
                (decode(min), decode(max), luma.total())
            }
        };
        let empty = |axis| Self {
            min: 0.0,
            max: 1.0,
            peak: 1.0,
            counted: 0,
            key: None,
            axis,
            plot: Plot::empty(),
            fine: Arc::new(Fine::empty(0.0, 1.0, color)),
            luma: ranked_luma.clone(),
            planes: ranked_planes.clone(),
        };
        if counted == 0 || !min.is_finite() || !max.is_finite() {
            // Nothing measurable at all; fall back to unit range so
            // the rest of the pipeline still has something usable.
            return empty(Axis::Curve(transfer));
        }

        // The plotted planes: the channels as the working space has them,
        // which are the stored channels decoded where nothing moves them.
        // Each with where it stands — as stored, placed on the axis as a
        // stored value is, or in linear light, placed as a value is.
        let plotted: Vec<(&Tally, Placed)> = match (&tallies.planes, gray) {
            (Some(planes), _) => planes.iter().map(|plane| (plane, Placed::Linear)).collect(),
            (None, false) => scan
                .stored
                .iter()
                .map(|channel| (&**channel, Placed::Stored))
                .collect(),
            (None, true) => Vec::new(),
        };
        // The highest any plotted channel reaches, in linear light.
        let peak = if tallies.luma.is_some() && (moves || color) {
            tallies.peak
        } else if color {
            plotted
                .iter()
                .map(|(tally, _)| transfer.to_linear(tally.extremes().1))
                .fold(f32::NEG_INFINITY, f32::max)
        } else {
            max
        };
        let peak = if peak.is_finite() { peak } else { max };
        // A photograph with a white — a developed raw, graded light — has
        // the top of its stops axis at that white at least, so that a dark
        // frame stands short of it rather than being stretched to its own
        // brightest, which is the normalizing of each frame to itself that
        // a photograph is not shown under. The peak itself stays the
        // picture's.
        let axis_top = if image.referred == Referred::Display {
            peak.max(1.0)
        } else {
            peak
        };

        let lit = Lit::of(luma, tallies.clear.as_ref(), decode);
        let key = lit.trimmed_log_mean().map(|log| log.exp2() as f32);

        // The axis: a curved file's starts at the range such a file
        // nominally holds and only widens; a linear one's is whatever is
        // measured over the plotted channels; and a stops axis is read off
        // the lit pixels, from the darkest the picture holds to say anything
        // about up to its brightest.
        let mut axis = Axis::of(image);
        let (axis_min, axis_max) = {
            let (mut low, mut high) = if transfer.is_linear() {
                (f32::INFINITY, f32::NEG_INFINITY)
            } else {
                (0.0, 1.0)
            };
            let on_axis = |tally: &Tally, placed: Placed| {
                let (min, max) = tally.extremes();
                match placed {
                    Placed::Stored => (min, max),
                    Placed::Linear => (encode(transfer, min), encode(transfer, max)),
                }
            };
            let channels: Vec<(f32, f32)> = if gray {
                let (min, max) = match &tallies.luma {
                    Some(luma) => on_axis(luma, Placed::Linear),
                    None => luma.extremes(),
                };
                vec![(min, max)]
            } else {
                plotted
                    .iter()
                    .map(|(tally, placed)| on_axis(tally, *placed))
                    .collect()
            };
            for (min, max) in channels {
                if min.is_finite() && max.is_finite() {
                    low = low.min(min);
                    high = high.max(max);
                }
            }
            match axis {
                Axis::Stops => match stops_axis(axis_top, &lit) {
                    Some(range) => range,
                    None => {
                        axis = Axis::Curve(transfer);
                        (low, high)
                    }
                },
                Axis::Curve(_) => (low, high),
            }
        };
        if !(axis_min.is_finite() && axis_max.is_finite()) {
            return empty(axis);
        }

        let mut fine = Fine::empty(axis_min, axis_max, color);
        fine.code = match &image.samples {
            Samples::U8 { .. } => Some(1.0 / f32::from(u8::MAX)),
            Samples::U16 { .. } => Some(1.0 / f32::from(u16::MAX)),
            Samples::F32 { .. } => None,
        };
        if scan.rebins(axis) {
            // Counted from the pixels, on the axis now known.
            let binning = pass.run(|| Binning {
                pass,
                axis,
                range: [axis_min, axis_max],
                luma: Some(vec![0; FINE_BINS]),
                planes: color.then(|| std::array::from_fn(|_| vec![0; FINE_BINS])),
                stored: Vec::new(),
            });
            fine.luma = binning.luma.expect("asked for");
            fine.color = binning.planes;
        } else {
            let range = [axis_min, axis_max];
            match &tallies.luma {
                Some(tally) => tally.pour_whole(&mut fine.luma, range, |value| axis.place(value)),
                None => luma.pour_whole(&mut fine.luma, range, |value| axis.place_stored(value)),
            }
            if let Some(planes) = fine.color.as_mut() {
                for (bins, (tally, placed)) in planes.iter_mut().zip(&plotted) {
                    match placed {
                        Placed::Stored => {
                            tally.pour_whole(bins, range, |value| axis.place_stored(value));
                        }
                        Placed::Linear => tally.pour_whole(bins, range, |value| axis.place(value)),
                    }
                }
            }
        }

        Self {
            min,
            max,
            peak,
            counted,
            key,
            axis,
            // The plot is the fine bins on the same axis, spread as any
            // other stage's plot of them is, so that a quantized file's codes
            // do not comb against bins of another width.
            plot: fine.rebinned([axis_min, axis_max], |value| value),
            fine: Arc::new(fine),
            luma: ranked_luma,
            planes: ranked_planes,
        }
    }

    /// The value below which `fraction` of the pixels' luminance falls,
    /// `fraction` in 0..=1, in linear working-space units. Returns
    /// `min`/`max` at the extremes.
    pub fn percentile(&self, fraction: f32) -> f32 {
        if self.counted == 0 || self.max <= self.min {
            return if fraction <= 0.0 { self.min } else { self.max };
        }
        let value = self.luma.linear(self.luma.tally.percentile(fraction));
        value.clamp(self.min, self.max)
    }

    fn ranked(&self, quantity: Quantity) -> Option<&Ranked> {
        match quantity {
            Quantity::Luminance => Some(&self.luma),
            Quantity::Channel(channel) => self.planes.as_ref()?.get(channel),
        }
    }

    /// Where the linear value `linear` stands among the pixels' `quantity`,
    /// as a fraction of them: see [`Tally::rank`].
    pub fn rank(&self, quantity: Quantity, linear: f32) -> Option<f32> {
        let ranked = self.ranked(quantity)?;
        ranked.tally.rank(ranked.units(linear))
    }

    /// The value the bin `linear` is tallied in stands for, in linear
    /// light: what a pixel is counted as, so that a function of it lands
    /// where the function of its bin does.
    pub fn binned(&self, quantity: Quantity, linear: f32) -> Option<f32> {
        let ranked = self.ranked(quantity)?;
        let index = ranked.tally.index_of(ranked.units(linear))?;
        Some(ranked.linear(ranked.tally.value_at(index)))
    }

    /// Where `out` stands among what `through` makes of the pixels'
    /// `quantity`, as a fraction of them: those it puts below `out` and
    /// half of those it puts at it. `through` takes a linear value and must
    /// never fall, as the display's response to a gray does not, which is
    /// what lets this search the tally rather than walk it through
    /// `through`. See [`Tally::rank_through`].
    pub fn rank_through(
        &self,
        quantity: Quantity,
        out: f32,
        through: impl Fn(f32) -> f32,
    ) -> Option<f32> {
        let ranked = self.ranked(quantity)?;
        ranked
            .tally
            .rank_through(out, |units| through(ranked.linear(units)))
    }
}

/// Which side of the file's curve a tally's values are on: as stored, a
/// fraction of full scale or the float itself, or decoded to linear light.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Placed {
    Stored,
    Linear,
}

/// The luminance of one pixel read back, in linear light, as the scan
/// works it out: what [`Stats::rank`] ranks for [`Quantity::Luminance`].
pub fn luminance_of(sample: &Sample) -> f32 {
    luminance(sample.linear(), sample.channels)
}

/// Gray uses its one channel; color collapses to relative luminance,
/// weighted for the BT.709 primaries the working space uses.
fn luminance(pixel: &[f32], channels: Channels) -> f32 {
    if channels.is_gray() {
        pixel[0]
    } else {
        0.2126 * pixel[0] + 0.7152 * pixel[1] + 0.0722 * pixel[2]
    }
}

/// Puts a linear value back on the storage curve, for plotting alongside
/// samples that never left it.
fn encode(transfer: Transfer, value: f32) -> f32 {
    if transfer.is_linear() {
        value
    } else {
        transfer.to_encoded(value)
    }
}

// ---------------------------------------------------------------------------
// Stored: the picture as the file stores it.

/// The picture as the file stores it: each color channel tallied as stored
/// — no lift, and in the file's own primaries — with its exact extremes and
/// the range it is plotted over. What the histogram's Image section plots,
/// a plot to the channel; luminance is not here, since the file stores
/// none, and a weighting of channels in the file's own primaries would not
/// be one.
#[derive(Clone, Debug)]
pub struct Stored {
    /// How the plots are spaced: the file's own curve, or stops for light
    /// stored linear as floats, as [`Stats::axis`] is.
    pub axis: Axis,
    /// One per color channel: one for gray, three for color, in the order
    /// the file keeps them.
    pub planes: Vec<Plane>,
    /// Whether every plane is plotted over the same range — a photograph's
    /// channels, which are compared against one another — or each over its
    /// own, as a measurement's are, whose channels may be different
    /// quantities.
    pub shared: bool,
}

/// One stored channel: its exact extremes as stored — a fraction of full
/// scale for integer samples, the value itself for floats — the range of
/// the axis its plot spans, and the tally the plot is poured from.
#[derive(Clone, Debug)]
pub struct Plane {
    /// The exact extremes, of every pixel: what a readout of them is to
    /// read, and what the tests hold the tally to.
    #[cfg_attr(
        not(test),
        allow(dead_code, reason = "a readout of the extremes is to come")
    )]
    pub min: f32,
    #[cfg_attr(
        not(test),
        allow(dead_code, reason = "a readout of the extremes is to come")
    )]
    pub max: f32,
    /// What the plot spans, on the axis.
    pub range: [f32; 2],
    tally: Arc<Tally>,
    /// The channel counted from the pixels into [`FINE_BINS`] over `range`
    /// by the second pass, where the tally's bins are too coarse for the
    /// range: floats on a linear axis.
    binned: Option<Vec<u32>>,
}

impl Plane {
    /// The plane's plot over its range on `axis`, in [`SHARES`] of a pixel.
    pub fn plot(&self, axis: Axis) -> [u32; BINS] {
        match &self.binned {
            Some(binned) => {
                let fine = Fine {
                    min: self.range[0],
                    max: self.range[1],
                    luma: binned.clone(),
                    color: None,
                    code: None,
                };
                fine.rebinned(self.range, |value| value).luma
            }
            None => self
                .tally
                .pour_shares(self.range, |value| axis.place_stored(value)),
        }
    }

    /// Which bin of the plot a value at `place` on the axis falls in, or
    /// `None` off the range.
    pub fn bin_at(&self, place: f32) -> Option<usize> {
        bin_in(self.range, place)
    }

    /// Where a value as stored — a fraction of full scale for integer
    /// samples, the value itself for floats — stands among the channel's
    /// pixels, as a fraction of them: see [`Tally::rank`].
    pub fn rank(&self, stored: f32) -> Option<f32> {
        self.tally.rank(stored)
    }
}

/// The share of a measurement's pixels left off each end of the range a
/// channel is plotted over, so that one stray value — a sentinel nobody
/// declared, a spike — does not put the whole of the picture into one bin:
/// the trims the display's own percentile window uses, so that the plot's
/// range is the one that window would put on it.
const PLOT_TRIM: f32 = 0.001;

/// How much of the exact range the trimmed one has to cover for the exact
/// one to be plotted instead: a picture with no strays in it is plotted
/// whole.
const WHOLE_FROM: f32 = 0.9;

impl Stored {
    /// Tallies `image` as stored, in one pass over every pixel. A file
    /// arriving is tallied by [`Stats::scan_both`], which hands both back
    /// from the one pass; this is for a test that wants the stored side
    /// alone.
    #[cfg(test)]
    pub fn scan(image: &DecodedImage) -> Self {
        Self::from_scan(&Scan::of(image, None))
    }

    fn from_scan(scan: &Scan) -> Self {
        let pass = &scan.pass;
        let image = pass.image;
        let transfer = pass.transfer;
        let floats = scan.floats();
        let data = image.referred == Referred::Measured;
        let mut axis = Axis::of(image);
        let extremes: Vec<(f32, f32)> = scan.stored.iter().map(|tally| tally.extremes()).collect();
        let highest = extremes
            .iter()
            .map(|(_, max)| *max)
            .fold(f32::NEG_INFINITY, f32::max);
        // White at the top at least, for a photograph with one: see the
        // same in `Stats::from_scan`.
        let highest = if image.referred == Referred::Display {
            highest.max(1.0)
        } else {
            highest
        };

        // The ranges: light in stops shares an axis read off every channel
        // together; a photograph's integer channels share the whole range
        // their samples can hold, a raw's from black to the sensor's
        // saturation, so that how far a picture stands short of clipping is
        // the room to the right of it; a photograph stored as curved floats
        // the range such a file holds, widened by what runs over; and a
        // measurement's channels each their own, trimmed of strays.
        let shared: Option<[f32; 2]> = match axis {
            Axis::Stops => {
                // Every channel's lit pixels together, for the floor: the
                // tallies pooled into one of their own kind, codes or keys.
                let pooled = scan.stored.split_first().map(|(first, rest)| {
                    let mut pooled = (**first).clone();
                    for tally in rest {
                        pooled.add(tally);
                    }
                    pooled
                });
                let range = pooled
                    .and_then(|pooled| stops_axis(highest, &Lit::of(&pooled, None, |value| value)));
                match range {
                    Some((floor, top)) => Some([floor, top]),
                    None => {
                        axis = Axis::Curve(transfer);
                        None
                    }
                }
            }
            Axis::Curve(_) if data => None,
            Axis::Curve(_) if !floats => Some([0.0, 1.0]),
            Axis::Curve(_) => {
                let lowest = extremes
                    .iter()
                    .map(|(min, _)| *min)
                    .fold(f32::INFINITY, f32::min);
                Some([lowest.min(0.0), highest.max(1.0)])
            }
        };
        let own = |tally: &Tally, (min, max): (f32, f32)| -> [f32; 2] {
            if !(min.is_finite() && max.is_finite()) {
                return [0.0, 1.0];
            }
            // Trimmed only where the trim is a whole pixel or more: on a
            // picture of a few pixels every one is the picture.
            let trims = (f64::from(PLOT_TRIM) * tally.total() as f64) as u64 >= 1;
            if data && trims {
                let (low, high) = (
                    tally.percentile(PLOT_TRIM),
                    tally.percentile(1.0 - PLOT_TRIM),
                );
                if high > low && high - low < WHOLE_FROM * (max - min) {
                    return [low, high];
                }
            }
            [min, max]
        };
        let ranges: Vec<[f32; 2]> = scan
            .stored
            .iter()
            .zip(&extremes)
            .map(|(tally, &extremes)| match shared {
                Some(range) => range,
                None => own(tally, extremes),
            })
            .collect();

        let binned: Vec<Option<Vec<u32>>> = if scan.rebins(axis) {
            let binning = pass.run(|| Binning {
                pass,
                axis,
                range: [0.0, 1.0],
                luma: None,
                planes: None,
                stored: ranges
                    .iter()
                    .map(|&range| (range, vec![0; FINE_BINS]))
                    .collect(),
            });
            binning
                .stored
                .into_iter()
                .map(|(_, bins)| Some(bins))
                .collect()
        } else {
            scan.stored.iter().map(|_| None).collect()
        };

        let planes = scan
            .stored
            .iter()
            .zip(extremes)
            .zip(ranges)
            .zip(binned)
            .map(|(((tally, (min, max)), range), binned)| Plane {
                min,
                max,
                range,
                tally: Arc::clone(tally),
                binned,
            })
            .collect();
        Self {
            axis,
            planes,
            shared: shared.is_some(),
        }
    }

    /// `counts`, a plane to the channel and a bin to the code over
    /// `0..=full`, as the planes of a picture whose codes they are: on a
    /// linear axis, every plane over the whole of the range, so that how far
    /// a channel stands short of `full` is the room to the right of it.
    /// What a raw's sensor is tallied into — see [`crate::image::sensor`].
    pub fn of_codes(counts: Vec<Vec<u32>>, full: f32) -> Self {
        let planes = counts
            .into_iter()
            .map(|counts| {
                let tally = Tally::Codes { counts, full };
                let (min, max) = tally.extremes();
                Plane {
                    min,
                    max,
                    range: [0.0, 1.0],
                    tally: Arc::new(tally),
                    binned: None,
                }
            })
            .collect();
        Self {
            axis: Axis::Curve(Transfer::Linear),
            planes,
            shared: true,
        }
    }

    /// The range every plane is plotted over where they share one.
    pub fn shared_range(&self) -> Option<[f32; 2]> {
        self.shared
            .then(|| self.planes.first().map_or([0.0, 1.0], |plane| plane.range))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One pixel, as a plot counts it.
    const ONE: u32 = SHARES as u32;
    use crate::image::{AlphaMode, ColorSpace, Primaries, Referred, Samples, Transfer};

    fn linear_gray(data: Vec<u16>) -> DecodedImage {
        DecodedImage {
            width: data.len() as u32,
            height: 1,
            samples: Samples::U16 {
                channels: Channels::Gray,
                data,
            },
            color: ColorSpace::LINEAR_BT709,
            alpha: AlphaMode::Opaque,
            referred: Referred::Scene,
            exposure: None,
            nodata: None,
            gain_map: None,
            depth: None,
            opens_on: None,
            sensor: None,
        }
    }

    /// The case the whole windowing feature exists for: 12-bit data in a
    /// 16-bit container occupies a sixteenth of the nominal range, so showing
    /// it unwindowed would be a nearly black rectangle.
    #[test]
    fn finds_the_range_of_twelve_bit_data_in_a_sixteen_bit_container() {
        let stats = Stats::scan(&linear_gray(vec![0, 1000, 2000, 4095]));
        assert!(stats.min.abs() < 1e-6);
        assert!((stats.max - 4095.0 / 65535.0).abs() < 1e-5, "{}", stats.max);
        assert!(stats.max < 0.07);
    }

    /// The key of a scene is the geometric mean of its light, which is
    /// what a meter reads: a dim room and one light meter to the room, and
    /// the same room a thousand times brighter meters a thousand times
    /// higher, so the meter's exposure comes out the same either way.
    #[test]
    fn the_key_is_the_geometric_mean_of_the_scene() {
        // A room lit unevenly, with one light: a hundred pixels, so that
        // the trims fall on whole ones — the darkest five and the
        // brightest five, the light among them.
        let mut room: Vec<f32> = (0..99).map(|i| 0.01 + 0.0004 * i as f32).collect();
        room.push(50.0);
        let key = Stats::scan(&linear_gray_f32(room.clone())).key.unwrap();
        let mut sorted = room.clone();
        sorted.sort_by(f32::total_cmp);
        let expected = sorted[5..95].iter().map(|value| value.log2()).sum::<f32>() / 90.0;
        assert!(
            (key.log2() - expected).abs() < 0.07,
            "{key} against {}",
            expected.exp2()
        );
        assert!(key < 0.05, "the one light does not carry the meter: {key}");

        let brighter: Vec<f32> = room.iter().map(|value| value * 1000.0).collect();
        let brighter = Stats::scan(&linear_gray_f32(brighter)).key.unwrap();
        assert!(
            ((brighter / key).log2() - 1000f32.log2()).abs() < 0.02,
            "{brighter}"
        );
    }

    /// A floor of near-nothing under part of the frame — a render's residue
    /// where a bounce all but died — is not the scene, and a meter that
    /// averaged it in would open the picture stops too bright. The darkest
    /// few percent are left out, as are the brightest, so a light source
    /// is not the scene either.
    #[test]
    fn the_key_leaves_out_the_floor_and_the_lights() {
        let mut room = vec![0.1; 96];
        room.extend([1e-7; 4]);
        let key = Stats::scan(&linear_gray_f32(room)).key.unwrap();
        assert!((key - 0.1).abs() < 0.01, "{key}");

        let mut room = vec![0.1; 96];
        room.extend([1e5; 4]);
        let key = Stats::scan(&linear_gray_f32(room)).key.unwrap();
        assert!((key - 0.1).abs() < 0.01, "{key}");

        // A picture so small that the trims leave nothing is metered on
        // all of it.
        let key = Stats::scan(&linear_gray_f32(vec![0.02, 8.0])).key.unwrap();
        assert!((key - 0.4).abs() < 0.02, "{key}");
    }

    /// Pixels that measured nothing are not part of the key: a render's
    /// black background, or the transparent surround of a premultiplied
    /// element, would otherwise set the exposure by how much of the frame
    /// they cover. Where nothing at all is lit there is no key.
    #[test]
    fn the_key_leaves_out_pixels_that_measured_nothing() {
        let lit = Stats::scan(&linear_gray_f32(vec![0.5; 4])).key.unwrap();
        let mut over_black = vec![0.0; 12];
        over_black.extend([0.5; 4]);
        let with_black = Stats::scan(&linear_gray_f32(over_black)).key.unwrap();
        assert!(
            (with_black - lit).abs() < 1e-6,
            "{with_black} against {lit}"
        );

        // Premultiplied RGBA: three transparent pixels and one lit one.
        let mut data = vec![0.0; 12];
        data.extend([0.5, 0.5, 0.5, 1.0]);
        let element = DecodedImage {
            width: 4,
            height: 1,
            samples: Samples::F32 {
                channels: Channels::Rgba,
                data,
            },
            color: ColorSpace::LINEAR_BT709,
            alpha: AlphaMode::Premultiplied,
            referred: Referred::Scene,
            exposure: None,
            nodata: None,
            gain_map: None,
            depth: None,
            opens_on: None,
            sensor: None,
        };
        let key = Stats::scan(&element).key.unwrap();
        assert!((key - 0.5).abs() < 1e-3, "{key}");

        // A transparent pixel with a color in it, on straight alpha, is
        // not lit either: it is taken off the lit pixels bin for bin.
        let mut data = vec![0.9, 0.9, 0.9, 0.0];
        data.extend([0.5, 0.5, 0.5, 1.0]);
        let straight = DecodedImage {
            samples: Samples::F32 {
                channels: Channels::Rgba,
                data,
            },
            width: 2,
            alpha: AlphaMode::Straight,
            ..element
        };
        let key = Stats::scan(&straight).key.unwrap();
        assert!((key - 0.5).abs() < 1e-3, "{key}");

        assert_eq!(Stats::scan(&linear_gray_f32(vec![0.0; 4])).key, None);
    }

    fn linear_gray_f32(data: Vec<f32>) -> DecodedImage {
        DecodedImage {
            width: data.len() as u32,
            height: 1,
            samples: Samples::F32 {
                channels: Channels::Gray,
                data,
            },
            color: ColorSpace::LINEAR_BT709,
            alpha: AlphaMode::Opaque,
            referred: Referred::Scene,
            exposure: None,
            nodata: None,
            gain_map: None,
            depth: None,
            opens_on: None,
            sensor: None,
        }
    }

    #[test]
    fn percentiles_ignore_a_lone_hot_pixel() {
        // A thousand samples near the bottom plus one at full scale.
        let mut data = vec![100u16; 1000];
        data.push(u16::MAX);
        let stats = Stats::scan(&linear_gray(data));

        assert!((stats.max - 1.0).abs() < 1e-6, "min/max still sees it");
        // The upper percentile does not, which is the point.
        assert!(stats.percentile(0.999) < 0.5, "{}", stats.percentile(0.999));
    }

    /// A stray far off the rest of a float measurement — a sentinel nobody
    /// declared, a spike — is in the extremes and nowhere else: the
    /// percentiles are read off the tally of every value, not off bins
    /// spread between the extremes, which one stray would leave empty but
    /// for the one everything else piled into.
    #[test]
    fn percentiles_are_exact_however_far_a_stray_is() {
        let mut data: Vec<f32> = (0..10_000).map(|i| 290.0 + i as f32 / 1000.0).collect();
        data.push(1e30);
        data.push(-9999.0);
        let stats = Stats::scan(&measured(linear_gray_f32(data)));
        assert_eq!(stats.min, -9999.0);
        assert_eq!(stats.max, 1e30);
        let (low, high) = (stats.percentile(0.001), stats.percentile(0.999));
        assert!((low - 290.0).abs() < 0.5, "{low}");
        assert!((high - 300.0).abs() < 0.5, "{high}");
    }

    #[test]
    fn a_flat_image_gives_a_degenerate_but_usable_range() {
        let stats = Stats::scan(&linear_gray(vec![500; 16]));
        assert!((stats.min - stats.max).abs() < 1e-9);
        // Percentiles must not divide by the zero-width span.
        assert!(stats.percentile(0.5).is_finite());
    }

    #[test]
    fn statistics_are_gathered_after_the_transfer_decode() {
        let mut image = linear_gray(vec![u16::MAX / 2]);
        let linear = Stats::scan(&image).max;

        image.color.transfer = Transfer::Srgb;
        let decoded = Stats::scan(&image).max;

        assert!((linear - 0.5).abs() < 1e-3, "{linear}");
        assert!((decoded - 0.2140).abs() < 1e-3, "{decoded}");
    }

    /// Something with structure in every band: a gradient across, a ramp
    /// down, and a stripe of NaNs the scan has to skip.
    fn structured(width: u32, height: u32) -> DecodedImage {
        let mut data = Vec::with_capacity((width * height * 3) as usize);
        for y in 0..height {
            for x in 0..width {
                let (x, y) = (x as f32 / width as f32, y as f32 / height as f32);
                if y > 0.4 && y < 0.45 {
                    data.extend([f32::NAN; 3]);
                } else {
                    data.extend([x, y, (x * 7.0 + y * 3.0).fract() * 1.5]);
                }
            }
        }
        DecodedImage {
            width,
            height,
            samples: Samples::F32 {
                channels: Channels::Rgb,
                data,
            },
            color: ColorSpace::SRGB,
            alpha: AlphaMode::Opaque,
            referred: Referred::Display,
            exposure: None,
            nodata: None,
            gain_map: None,
            depth: None,
            opens_on: None,
            sensor: None,
        }
    }

    /// Dividing the scan between threads must not change a count: a
    /// picture large enough to be divided folds to exactly what the same
    /// picture walked on one thread counts — bins, extremes and the pixel
    /// count — however many threads there are.
    #[test]
    fn a_divided_scan_agrees_with_a_plain_one() {
        let image = structured(400, 200);
        assert!(bands_for(image.width, image.height) > 1 || rayon_core::current_num_threads() == 1);
        let pass = Pass::new(&image, None);
        let Samples::F32 { data, .. } = &image.samples else {
            unreachable!()
        };
        let mut plain = Counting {
            pass: &pass,
            tallies: Tallies::new(&pass),
        };
        pass.walk(data, 0..image.height, &mut plain);
        let divided = pass.run(|| Counting {
            pass: &pass,
            tallies: Tallies::new(&pass),
        });
        let (plain, divided) = (plain.tallies, divided.tallies);
        assert_eq!(plain.counted, divided.counted);
        assert_eq!(plain.min, divided.min);
        assert_eq!(plain.max, divided.max);
        assert_eq!(plain.peak, divided.peak);
        for (mine, theirs) in plain.stored.iter().zip(&divided.stored) {
            assert_eq!(mine.total(), theirs.total());
            assert_eq!(mine.extremes(), theirs.extremes());
            assert_eq!(mine.percentile(0.37), theirs.percentile(0.37));
        }
        let (mine, theirs) = (plain.luma.unwrap(), divided.luma.unwrap());
        assert_eq!(mine.total(), theirs.total());
        assert_eq!(mine.percentile(0.5), theirs.percentile(0.5));

        let (stats, stored) = Stats::scan_both(&image);
        assert_eq!(stats.counted, divided.counted);
        assert_eq!(stored.planes.len(), 3);
    }

    /// The twin: the one pass reads every pixel through the same table,
    /// lift and matrix as [`DecodedImage::sample`] reads one, so what it
    /// counts is what a walk over the pixels through the reader counts,
    /// for every kind of sample, primaries and alpha, with and without a
    /// lift.
    #[test]
    fn the_pass_agrees_with_the_reader_on_every_pixel() {
        use crate::image::gain_map::{GainMap, Lift};
        let (width, height) = (13u32, 7u32);
        let pixels = (width * height) as usize;
        let mut seed = 0x9E37_79B9u32;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed
        };
        let layouts = [
            Channels::Gray,
            Channels::GrayAlpha,
            Channels::Rgb,
            Channels::Rgba,
        ];
        let primaries = [Primaries::Bt709, Primaries::DisplayP3, Primaries::Bt2020];
        let transfers = [Transfer::Srgb, Transfer::Linear];
        for channels in layouts {
            for primaries in primaries {
                for transfer in transfers {
                    for lifted in [false, true] {
                        for depth in 0..3 {
                            let count = channels.count() * pixels;
                            let samples = match depth {
                                0 => Samples::U8 {
                                    channels,
                                    data: (0..count).map(|_| next() as u8).collect(),
                                },
                                1 => Samples::U16 {
                                    channels,
                                    data: (0..count).map(|_| (next() >> 16) as u16).collect(),
                                },
                                _ => Samples::F32 {
                                    channels,
                                    data: (0..count)
                                        .map(|_| (next() >> 8) as f32 / (1u32 << 24) as f32 * 1.5)
                                        .collect(),
                                },
                            };
                            let mut image = DecodedImage {
                                width,
                                height,
                                samples,
                                color: ColorSpace {
                                    transfer,
                                    primaries,
                                },
                                alpha: if channels.alpha_index().is_some() {
                                    AlphaMode::Premultiplied
                                } else {
                                    AlphaMode::Opaque
                                },
                                referred: Referred::Display,
                                exposure: None,
                                nodata: None,
                                gain_map: None,
                                depth: None,
                                opens_on: None,
                                sensor: None,
                            };
                            let table = lifted.then(|| {
                                let map = GainMap {
                                    width: 3,
                                    height: 2,
                                    channels: 1,
                                    data: crate::image::gain_map::Data::Codes(vec![
                                        0, 64, 128, 192, 255, 32,
                                    ]),
                                    lift: Lift::Apple { headroom: 4.0 },
                                };
                                let table = map.table(1.0);
                                image.gain_map = Some(Arc::new(map));
                                table
                            });
                            let stats = Stats::scan_with(&image, table.as_ref());
                            let label = format!(
                                "{channels:?} {primaries:?} {transfer:?} lift {lifted} depth {depth}"
                            );

                            // The reference: every pixel through the reader.
                            let (mut min, mut max, mut peak) =
                                (f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY);
                            let mut lumas = Vec::with_capacity(pixels);
                            let mut logs = Vec::new();
                            for y in 0..height {
                                for x in 0..width {
                                    let sample = image.sample(x, y, table.as_ref()).unwrap();
                                    let linear = sample.linear();
                                    let value = luminance(linear, channels);
                                    min = min.min(value);
                                    max = max.max(value);
                                    lumas.push(value);
                                    for &channel in &linear[..channels.color_count()] {
                                        peak = peak.max(channel);
                                    }
                                    if value > 0.0 && sample.alpha > 0.0 {
                                        logs.push(f64::from(value.log2()));
                                    }
                                }
                            }
                            let close = |a: f32, b: f32| (a - b).abs() <= 1e-5 * a.abs().max(1.0);
                            assert!(close(stats.min, min), "{label}: min {} vs {min}", stats.min);
                            assert!(close(stats.max, max), "{label}: max {} vs {max}", stats.max);
                            assert!(
                                close(stats.peak, peak),
                                "{label}: peak {} vs {peak}",
                                stats.peak
                            );
                            assert_eq!(stats.counted as usize, pixels, "{label}");
                            lumas.sort_by(f32::total_cmp);
                            for fraction in [0.1f32, 0.5, 0.9] {
                                // The pixel the `fraction`th one is, counted
                                // from one, as the percentile counts them.
                                let target = (fraction * pixels as f32) as usize;
                                let at = lumas[target.saturating_sub(1).min(pixels - 1)];
                                let got = stats.percentile(fraction);
                                // Within a tally bin of the sorted value, which
                                // is a thousandth of an octave, or a code.
                                assert!(
                                    (got - at).abs() <= 2e-3 * at.abs().max(1e-3) + 1.0 / 255.0,
                                    "{label}: percentile {fraction}: {got} vs {at}"
                                );
                            }
                            if !logs.is_empty() {
                                logs.sort_by(f64::total_cmp);
                                let trim = (logs.len() as f64 * KEY_TRIM) as usize;
                                let kept = &logs[trim..logs.len() - trim];
                                let expected = kept.iter().sum::<f64>() / kept.len() as f64;
                                let key = stats.key.unwrap_or_else(|| panic!("{label}: a key"));
                                assert!(
                                    (f64::from(key.log2()) - expected).abs() < 0.05,
                                    "{label}: key {key} vs {}",
                                    expected.exp2()
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    /// A value's rank is the share of the pixels below it and half of
    /// those it shares its bin with: an even spread of codes ranks each at
    /// its own middle, a value every pixel shares in the middle of all of
    /// them, and a float between the pixels' values after those below it.
    #[test]
    fn a_tally_ranks_a_value_among_its_pixels() {
        let mut codes = Tally::codes(255.0, 256);
        if let Tally::Codes { counts, .. } = &mut codes {
            for code in [0, 64, 128, 255] {
                counts[code] = 1;
            }
        }
        assert_eq!(codes.rank(0.0), Some(0.125));
        assert_eq!(codes.rank(64.0 / 255.0), Some(0.375));
        assert_eq!(codes.rank(100.0 / 255.0), Some(0.5));
        assert_eq!(codes.rank(1.0), Some(0.875));
        assert_eq!(codes.rank(f32::NAN), None);
        assert_eq!(Tally::codes(255.0, 256).rank(0.5), None);

        let mut keys = Tally::keys();
        if let Tally::Keys { counts, .. } = &mut keys {
            for value in [-2.0, 0.5, 0.5, 300.0] {
                counts[key_of(value)] += 1;
            }
        }
        assert_eq!(keys.rank(-5.0), Some(0.0));
        assert_eq!(keys.rank(0.5), Some(0.5));
        assert_eq!(keys.rank(1.0), Some(0.75));
        assert_eq!(keys.rank(1e30), Some(1.0));
    }

    /// A value is ranked among what a function makes of the pixels: those
    /// it puts below the value and half of those it puts at it, so that a
    /// pile the function clips together stands in the pile's middle.
    #[test]
    fn a_tally_ranks_a_value_through_a_function() {
        let mut codes = Tally::codes(255.0, 256);
        if let Tally::Codes { counts, .. } = &mut codes {
            counts.fill(1);
        }
        let doubled = |value: f32| (2.0 * value).min(1.0);
        // Codes 128 and up are doubled past 1 and clipped to it: half of
        // them stand below the pile's middle, and all 128 below them.
        assert_eq!(codes.rank_through(1.0, doubled), Some(0.75));
        assert_eq!(
            codes.rank_through(2.0 * (64.0 / 255.0), doubled),
            Some(64.5 / 256.0)
        );
        assert_eq!(codes.rank_through(-1.0, doubled), Some(0.0));
        assert_eq!(codes.rank_through(f32::NAN, doubled), None);

        let mut keys = Tally::keys();
        if let Tally::Keys { counts, min, max } = &mut keys {
            for value in [-2.0, 0.5, 0.5, 300.0] {
                counts[key_of(value)] += 1;
            }
            (*min, *max) = (-2.0, 300.0);
        }
        let floored = |value: f32| value.max(0.0);
        assert_eq!(keys.rank_through(0.0, floored), Some(0.125));
        assert_eq!(keys.rank_through(1.0, floored), Some(0.75));
    }

    /// The tally's bins are in value order, each spanning the values that
    /// fall in it and no others, from the most negative to the most
    /// positive, with the end bins reaching past the range.
    #[test]
    fn a_tally_bins_every_float_in_value_order() {
        let values = [
            -1e30,
            -300.0,
            -1.5,
            -1.0,
            -0.999,
            -1e-20,
            -0.0,
            0.0,
            1e-20,
            0.5,
            0.999,
            1.0,
            1.0009,
            1.5,
            290.0,
            300.0,
            1e30,
            f32::MAX,
        ];
        let mut last = 0;
        for (index, &value) in values.iter().enumerate() {
            let key = key_of(value);
            assert!(key < KEY_BINS, "{value}");
            if index > 0 {
                assert!(key >= last, "{value} is tallied before the value below it");
            }
            last = key;
            let (low, high) = key_edges(key);
            assert!(low <= value && value <= high, "{value} in {low}..{high}");
            if key > 0 {
                let (_, below) = key_edges(key - 1);
                assert!(below <= low, "{value}: the bin below ends at {below}");
            }
        }
        // Distinct values a thousandth of an octave apart are distinct bins.
        assert_ne!(key_of(1.0), key_of(1.001));
        assert_eq!(key_of(1.0), key_of(1.0005));
        // The exact edges of 1.
        assert_eq!(key_edges(key_of(1.0)).0, 1.0);
        assert_eq!(key_edges(key_of(0.25)).0, 0.25);
    }

    /// A wide-gamut file is measured in the working space, as the shader
    /// has it when the window goes on: a P3 red carried into BT.709 is
    /// past white in red and below zero in green, so the peak is above
    /// white and the axis runs past both ends of the file's own range,
    /// where a surface that stops at them will clip it, with the red plane
    /// piled at the top of the plot and the green at the bottom. An sRGB
    /// red is white in red and no further, and its codes are plotted
    /// exactly as stored.
    #[test]
    fn a_wide_gamut_color_is_measured_where_the_working_space_puts_it() {
        let red = |primaries| DecodedImage {
            width: 1,
            height: 1,
            samples: Samples::U8 {
                channels: Channels::Rgb,
                data: vec![255, 0, 0],
            },
            color: ColorSpace {
                transfer: Transfer::Srgb,
                primaries,
            },
            alpha: AlphaMode::Opaque,
            referred: Referred::Display,
            exposure: None,
            nodata: None,
            gain_map: None,
            depth: None,
            opens_on: None,
            sensor: None,
        };

        let p3 = Stats::scan(&red(Primaries::DisplayP3));
        assert!((1.2..1.25).contains(&p3.peak), "{}", p3.peak);
        assert!(p3.plot.max > 1.0, "{}", p3.plot.max);
        assert!(p3.plot.min < 0.0, "{}", p3.plot.min);
        let planes = p3.plot.color.expect("three planes");
        assert_eq!(planes[0][BINS - 1], ONE, "red at the top of the plot");
        assert_eq!(planes[1][0], ONE, "green at the bottom");
        // The luminance is the color's own, which BT.709's weights give
        // only once the color is in BT.709: a P3 red is a little brighter
        // than an sRGB one.
        assert!((p3.max - 0.229).abs() < 0.002, "{}", p3.max);

        let srgb = Stats::scan(&red(Primaries::Bt709));
        assert_eq!(srgb.peak, 1.0);
        assert_eq!((srgb.plot.min, srgb.plot.max), (0.0, 1.0));
        assert!((srgb.max - 0.2126).abs() < 1e-4, "{}", srgb.max);

        // As the file stores it, the P3 red is at the top of the file's own
        // range and its green at the bottom, and nothing is past either.
        let stored = Stored::scan(&red(Primaries::DisplayP3));
        assert_eq!(stored.shared_range(), Some([0.0, 1.0]));
        assert_eq!(stored.planes.len(), 3);
        let plots: Vec<[u32; BINS]> = stored
            .planes
            .iter()
            .map(|plane| plane.plot(stored.axis))
            .collect();
        assert_eq!(plots[0][BINS - 1], ONE, "red at the top");
        assert_eq!(plots[1][0], ONE, "green at the bottom");
        assert_eq!((stored.planes[0].min, stored.planes[0].max), (1.0, 1.0));
        assert_eq!((stored.planes[1].min, stored.planes[1].max), (0.0, 0.0));
    }

    /// Every pixel's marker lands on a bar that actually has the pixel in it.
    ///
    /// The one that would go wrong quietly: this image is P3 with
    /// premultiplied alpha, so the color a readout gets back from
    /// [`DecodedImage::sample`] has been through a primaries matrix and a
    /// divide by alpha that the scan never applied. A bin taken from that
    /// would point at an empty bar beside the pixel's own.
    #[test]
    fn a_pixels_bin_is_the_bar_that_counted_it() {
        let image = DecodedImage {
            width: 4,
            height: 1,
            samples: Samples::U8 {
                channels: Channels::Rgba,
                data: vec![
                    10, 20, 30, 255, // opaque, so the divide is a no-op
                    9, 40, 60, 128, // and these three are not
                    60, 30, 15, 128, //
                    128, 128, 128, 128,
                ],
            },
            color: ColorSpace {
                transfer: Transfer::Srgb,
                primaries: Primaries::DisplayP3,
            },
            alpha: AlphaMode::Premultiplied,
            referred: Referred::Display,
            exposure: None,
            nodata: None,
            gain_map: None,
            depth: None,
            opens_on: None,
            sensor: None,
        };
        let stats = Stats::scan(&image);
        let plot = &stats.plot;

        for x in 0..image.width {
            let sample = image.sample(x, 0, None).expect("inside the image");
            let bin = plot
                .bin_of(stats.axis, &image, &sample)
                .expect("on the axis");
            assert!(
                plot.luma[bin] > 0,
                "pixel {x} marked at bin {bin}, which counted nothing"
            );
        }
    }

    /// A value the plot does not cover has no bar to stand on, and is told so
    /// rather than being pushed onto the bar at the end.
    #[test]
    fn a_value_off_the_axis_gets_no_bin() {
        let plot = Stats::scan(&linear_gray(vec![1000, 2000])).plot;
        assert_eq!(plot.bin_at(plot.min), Some(0));
        assert_eq!(plot.bin_at(plot.max), Some(BINS - 1));
        assert_eq!(plot.bin_at(plot.min - (plot.max - plot.min)), None);
        assert_eq!(plot.bin_at(plot.max + (plot.max - plot.min)), None);
        assert_eq!(plot.bin_at(f32::NAN), None);

        let flat = Stats::scan(&linear_gray(vec![500; 4])).plot;
        assert_eq!(flat.bin_at(500.0 / 65535.0), None, "no span, no bars");
    }

    #[test]
    fn color_images_are_measured_on_luminance() {
        let green = DecodedImage {
            width: 1,
            height: 1,
            samples: Samples::F32 {
                channels: Channels::Rgb,
                data: vec![0.0, 1.0, 0.0],
            },
            color: ColorSpace::LINEAR_BT709,
            alpha: AlphaMode::Opaque,
            referred: Referred::Scene,
            exposure: None,
            nodata: None,
            gain_map: None,
            depth: None,
            opens_on: None,
            sensor: None,
        };
        // BT.709 luminance weights green at 0.7152.
        assert!((Stats::scan(&green).max - 0.7152).abs() < 1e-4);
    }

    fn rgb_f32(data: Vec<f32>) -> DecodedImage {
        DecodedImage {
            width: (data.len() / 3) as u32,
            height: 1,
            samples: Samples::F32 {
                channels: Channels::Rgb,
                data,
            },
            color: ColorSpace::LINEAR_BT709,
            alpha: AlphaMode::Opaque,
            referred: Referred::Scene,
            exposure: None,
            nodata: None,
            gain_map: None,
            depth: None,
            opens_on: None,
            sensor: None,
        }
    }

    /// `image` read as a measurement rather than as light: numbers, on the
    /// linear axis they are measured in.
    fn measured(mut image: DecodedImage) -> DecodedImage {
        image.referred = Referred::Measured;
        image
    }

    /// The point of the separate axis: saturated color runs well past the
    /// luminance range, and binning it there would pile it into the last bin.
    #[test]
    fn color_channels_are_binned_over_their_own_range() {
        let stats = Stats::scan(&measured(rgb_f32(vec![1.0, 0.0, 0.0])));
        assert_eq!(stats.axis, Axis::Curve(Transfer::Linear));
        let plot = &stats.plot;
        let color = plot.color.expect("an rgb image has channel counts");

        assert!((stats.max - 0.2126).abs() < 1e-4, "luminance is unchanged");
        assert!(plot.min.abs() < 1e-6);
        assert!((plot.max - 1.0).abs() < 1e-6);
        // Red at the top of the axis, green and blue at the bottom.
        assert_eq!(color[0][BINS - 1], ONE);
        assert_eq!(color[1][0], ONE);
        assert_eq!(color[2][0], ONE);
        // Luminance sits between them, on the same axis as the rest: spread
        // over a bin's width about where it falls, most of it in its own.
        assert_eq!(plot.luma.iter().sum::<u32>(), ONE);
        let at = (0.2126 * (BINS - 1) as f32).round() as usize;
        assert!(plot.luma[at] > ONE / 2, "{:?}", &plot.luma[at - 1..=at + 1]);
    }

    #[test]
    fn every_plane_counts_every_pixel() {
        let stats = Stats::scan(&rgb_f32(vec![0.25, 0.5, 0.75, 0.0, 0.5, 1.0]));
        let color = stats.plot.color.expect("an rgb image has channel counts");
        // Each share of a pixel spread over two bins is rounded on its own,
        // so the sum can be a share either side of the pixels.
        let near = |sum: u32| sum.abs_diff(2 * ONE) <= 2;
        for plane in &color {
            assert!(near(plane.iter().sum::<u32>()));
        }
        assert!(near(stats.plot.luma.iter().sum::<u32>()));
        assert_eq!(stats.counted, 2);
        assert_eq!(stats.fine.luma.iter().sum::<u32>(), 2, "whole pixels");
    }

    #[test]
    fn alpha_is_left_out_of_the_channel_histogram() {
        let opaque = Stats::scan(&rgb_f32(vec![0.25, 0.5, 0.75]));
        let image = DecodedImage {
            samples: Samples::F32 {
                channels: Channels::Rgba,
                data: vec![0.25, 0.5, 0.75, 1.0],
            },
            width: 1,
            ..rgb_f32(vec![0.0, 0.0, 0.0])
        };
        let with_alpha = Stats::scan(&image).plot;
        let opaque = opaque.plot;

        assert_eq!(with_alpha.max, opaque.max, "alpha would have widened this");
        assert_eq!(with_alpha.color, opaque.color);
    }

    #[test]
    fn gray_images_have_no_channel_histogram() {
        assert!(
            Stats::scan(&linear_gray(vec![0, 1000, 4095]))
                .plot
                .color
                .is_none()
        );
        assert_eq!(
            Stored::scan(&linear_gray(vec![0, 1000, 4095])).planes.len(),
            1
        );
    }

    /// A flat image spans nothing, and binning it would divide by a
    /// zero-width axis. In stops it is given a stop to stand in, since a
    /// spike at its one value is no lie there.
    #[test]
    fn a_flat_image_plots_nothing() {
        let plot = Stats::scan(&measured(rgb_f32(vec![0.5, 0.5, 0.5]))).plot;
        assert_eq!(plot.luma.iter().sum::<u32>(), 0);
        assert!(
            plot.color
                .is_some_and(|planes| planes.iter().all(|plane| plane.iter().sum::<u32>() == 0))
        );

        let stats = Stats::scan(&rgb_f32(vec![0.5, 0.5, 0.5]));
        assert_eq!(stats.axis, Axis::Stops);
        assert_eq!((stats.plot.min, stats.plot.max), (-2.0, -1.0));
        assert_eq!(stats.plot.luma[BINS - 1], ONE);
    }

    /// Light stored linear as floats is plotted in stops, from the darkest
    /// it holds to say anything about up to the first whole stop at or
    /// above its brightest, and a pixel with no light in it is in the first
    /// bin, below every floor.
    #[test]
    fn light_stored_as_floats_is_plotted_in_stops() {
        let stats = Stats::scan(&linear_gray_f32(vec![0.0, 0.25, 1.0, 256.0]));
        assert_eq!(stats.axis, Axis::Stops);
        let plot = &stats.plot;
        assert_eq!((plot.min, plot.max), (-2.0, 8.0));
        assert_eq!(plot.luma[0], 2 * ONE, "none, and a quarter at the floor");
        let unity = plot.bin_at(0.0).expect("1 is on the axis");
        assert_eq!(plot.luma[unity - 1..=unity + 1].iter().sum::<u32>(), ONE);
        assert_eq!(plot.luma[BINS - 1], ONE);
        let sample = linear_gray_f32(vec![0.0, 0.25, 1.0, 256.0]);
        let pixel = sample.sample(2, 0, None).expect("inside");
        assert_eq!(plot.bin_of(stats.axis, &sample, &pixel), Some(unity));
        assert_eq!(stats.axis.place(0.0), f32::NEG_INFINITY);
        assert_eq!(stats.axis.place(-1.0), f32::NEG_INFINITY);
        assert_eq!(stats.axis.value(3.0), 8.0);

        // The file as stored stands on the same axis.
        let stored = Stored::scan(&linear_gray_f32(vec![0.0, 0.25, 1.0, 256.0]));
        assert_eq!(stored.axis, Axis::Stops);
        assert_eq!(stored.shared_range(), Some([-2.0, 8.0]));
        let plotted = stored.planes[0].plot(stored.axis);
        assert_eq!(plotted[0], 2 * ONE);
        assert_eq!(plotted[BINS - 1], ONE);

        // The linear axis stays what it was for a measurement, and a
        // graded linear float is light.
        let numbers = Stats::scan(&measured(linear_gray_f32(vec![0.0, 0.25, 1.0, 256.0])));
        assert_eq!(numbers.axis, Axis::Curve(Transfer::Linear));
        assert_eq!((numbers.plot.min, numbers.plot.max), (0.0, 256.0));
        let mut graded = linear_gray_f32(vec![0.0, 0.25, 1.0]);
        graded.referred = Referred::Display;
        assert_eq!(Stats::scan(&graded).axis, Axis::Stops);
    }

    /// The floor of a stops axis is read off the picture with a sliver
    /// trimmed from the bottom, so that a few stray pixels at next to
    /// nothing cannot put it tens of stops down; a residue that is a real
    /// share of the frame does pull it down, as far as the axis reaches;
    /// and a picture with no light at all has no top for a stops axis and
    /// is plotted on the linear one.
    #[test]
    fn a_stray_pixel_does_not_set_the_floor_of_a_stops_axis() {
        let mut lit = vec![1.0; 2000];
        lit.push(1e-30);
        let stats = Stats::scan(&linear_gray_f32(lit));
        assert_eq!((stats.plot.min, stats.plot.max), (-1.0, 0.0));
        assert_eq!(stats.plot.luma[0], ONE, "the stray is in the first bin");

        let mut residue = vec![1.0; 80];
        residue.extend([1e-9; 20]);
        let stats = Stats::scan(&linear_gray_f32(residue));
        assert_eq!((stats.plot.min, stats.plot.max), (-STOPS_REACH, 0.0));
        assert_eq!(stats.plot.luma[0], 20 * ONE);

        let dark = Stats::scan(&linear_gray_f32(vec![0.0, 0.0, -1.0]));
        assert_eq!(dark.axis, Axis::Curve(Transfer::Linear));
    }

    fn srgb_gray_u8(data: Vec<u8>) -> DecodedImage {
        DecodedImage {
            width: data.len() as u32,
            height: 1,
            samples: Samples::U8 {
                channels: Channels::Gray,
                data,
            },
            color: ColorSpace {
                transfer: Transfer::Srgb,
                ..ColorSpace::LINEAR_BT709
            },
            alpha: AlphaMode::Opaque,
            referred: Referred::Display,
            exposure: None,
            nodata: None,
            gain_map: None,
            depth: None,
            opens_on: None,
            sensor: None,
        }
    }

    /// The reason the plot is binned on the storage curve. Decoding first and
    /// binning the linear values leaves two thirds of the highlight bins
    /// unreachable, which draws as a comb of spikes.
    #[test]
    fn consecutive_eight_bit_codes_land_in_consecutive_bins() {
        let codes: Vec<u8> = (200..=255).collect();
        let expected = codes.len();
        let plot = Stats::scan(&srgb_gray_u8(codes)).plot;

        let occupied = plot.luma.iter().filter(|count| **count > 0).count();
        assert_eq!(occupied, expected, "every code needs a bin of its own");
        // And they are contiguous: no empty bin anywhere between the ends.
        let first = plot.luma.iter().position(|count| *count > 0).unwrap();
        let last = plot.luma.iter().rposition(|count| *count > 0).unwrap();
        assert!(
            plot.luma[first..=last].iter().all(|count| *count > 0),
            "a gap between codes is the comb this binning exists to avoid"
        );
    }

    /// A display-referred file plots against the range it can hold, so that
    /// a low-contrast one reads as low contrast instead of being stretched
    /// across the panel — and so its codes stay one to a bin.
    #[test]
    fn display_referred_images_plot_against_the_nominal_range() {
        let plot = Stats::scan(&srgb_gray_u8(vec![200, 255])).plot;
        assert!(plot.min.abs() < 1e-6);
        assert!((plot.max - 1.0).abs() < 1e-6);
        assert_eq!(plot.luma[200], ONE);
        assert_eq!(plot.luma[BINS - 1], ONE);
    }

    /// Linear samples read as data have no curve to plot against, so the
    /// axis stays in the units the file measured in; the same samples as
    /// light are plotted in stops, as a float's are, however they are
    /// stored.
    #[test]
    fn linear_integers_are_data_on_a_linear_axis_and_light_in_stops() {
        let stats = Stats::scan(&measured(linear_gray(vec![0, 16384, 32768, 65535])));
        assert_eq!(stats.axis, Axis::Curve(Transfer::Linear));
        assert!(stats.plot.min.abs() < 1e-6);
        assert!((stats.plot.max - 1.0).abs() < 1e-6);
        // A quarter of full scale bins a quarter of the way along, which on
        // a curve it would not: sRGB would put it past the half way mark. A
        // quarter of 255 bins is between two of them, and shared by both.
        let quarter = (0.25 * (BINS - 1) as f32).floor() as usize;
        assert_eq!(stats.plot.luma[quarter] + stats.plot.luma[quarter + 1], ONE);

        // As light — a developed raw — the top is white and a quarter of
        // full scale is two stops under it.
        let light = Stats::scan(&linear_gray(vec![0, 16384, 32768, 65535]));
        assert_eq!(light.axis, Axis::Stops);
        assert_eq!(light.plot.max, 0.0, "white at the top");
        assert!(light.plot.min <= -2.0);
        let two_under = bin_in(light.plot.range(), -2.0).expect("on the axis");
        assert!(light.plot.luma[two_under] > 0 || light.plot.luma[two_under + 1] > 0);
    }

    /// A bound sitting on a code clips that code and everything past it,
    /// and nothing else: the share is exact on a quantized file, which is
    /// where a reader is most likely to hold it against the pixels.
    #[test]
    fn the_clipped_share_counts_the_end_bins_exactly() {
        let mut plot = Plot::empty();
        // Ten samples: two at black, one at code 1, six in the middle and
        // one at white. The axis is 0..1, a bin to the code.
        let code = |index: usize| index as f32 / (BINS - 1) as f32;
        plot.luma[0] = 2;
        plot.luma[1] = 1;
        plot.luma[128] = 6;
        plot.luma[BINS - 1] = 1;
        assert_eq!(plot.clipped(0.0, 1.0), [0.2, 0.1]);
        assert_eq!(
            plot.clipped(code(1), code(128)),
            [0.3, 0.7],
            "a bound on a code takes that code with it"
        );
        assert_eq!(
            plot.clipped(-0.5, 1.5),
            [0.0, 0.0],
            "a window wider than the axis clips nothing"
        );
        assert_eq!(plot.clipped(f32::NAN, f32::NAN), [0.0, 0.0]);

        // Color: the plane that loses the most is the one reported, since
        // it is the one that has piled up at the end of the plot.
        let mut planes = [[0u32; BINS]; COLOR];
        planes[0][BINS - 1] = 5;
        planes[0][10] = 5;
        planes[1][10] = 10;
        planes[2][0] = 1;
        planes[2][10] = 9;
        plot.color = Some(planes);
        assert_eq!(plot.clipped(0.0, 1.0), [0.1, 0.5]);

        // No span, no shares: a flat image is not clipped, it is flat.
        plot.max = plot.min;
        assert_eq!(plot.clipped(0.0, 1.0), [0.0, 0.0]);
    }

    /// The fine bins pushed back through a stage that does nothing are the
    /// plot again, exactly: every code of an 8-bit file lands on its own
    /// coarse bin from its fine one, which is what keeps a plot rebinned
    /// from them free of the comb the coarse bins were laid out to avoid.
    #[test]
    fn the_fine_bins_through_nothing_are_the_plot() {
        let codes: Vec<u8> = (0..=255).chain([0, 17, 255, 255]).collect();
        let stats = Stats::scan(&srgb_gray_u8(codes));
        let again = stats
            .fine
            .rebinned([stats.plot.min, stats.plot.max], |value| value);
        assert_eq!(again.luma, stats.plot.luma);

        let rgb = Stats::scan(&rgb_f32(vec![0.25, 0.5, 0.75, 0.0, 0.5, 1.0]));
        let again = rgb
            .fine
            .rebinned([rgb.plot.min, rgb.plot.max], |value| value);
        assert_eq!(again.color, rgb.plot.color);
    }

    /// A stop up doubles every value: a sample at a quarter lands in a
    /// half's bin, and one already past the top of the axis it is binned
    /// over piles into the last bin, which is the clip.
    #[test]
    fn a_stage_moves_the_bins_and_piles_what_it_pushes_off_the_end() {
        let stats = Stats::scan(&measured(linear_gray_f32(vec![0.0, 0.25, 0.75, 1.0])));
        let doubled = stats.fine.rebinned([0.0, 1.0], |value| value * 2.0);
        // Half of 255 bins is between two of them, which share it.
        let half = (0.5 * (BINS - 1) as f32).floor() as usize;
        assert_eq!(
            doubled.luma[half] + doubled.luma[half + 1],
            ONE,
            "a quarter went to a half"
        );
        assert_eq!(
            doubled.luma[BINS - 1],
            2 * ONE,
            "both past white piled at it"
        );
        assert_eq!(doubled.luma[0], ONE);
        assert_eq!(doubled.luma.iter().sum::<u32>(), 4 * ONE);

        let none = stats.fine.rebinned([0.0, 1.0], |_| f32::NAN);
        assert_eq!(none.luma.iter().sum::<u32>(), 0, "no number, no count");
    }

    /// Fine bins counted on `[min, max]` with `counts` at each value given,
    /// for holding [`Fine::rebinned`] to what it spreads.
    fn fine_with(min: f32, max: f32, at: impl Iterator<Item = (f32, u32)>) -> Fine {
        let mut fine = Fine::empty(min, max, false);
        let scale = (FINE_BINS - 1) as f32 / (max - min);
        for (value, count) in at {
            fine.luma[((value - min) * scale).round() as usize] += count;
        }
        fine
    }

    /// The bins between the first and last holding anything: no more apart
    /// than `tolerance` of the fullest of them, neighbor to neighbor.
    fn smooth(plot: &[u32; BINS], tolerance: f32) -> bool {
        let first = plot.iter().position(|&count| count > 0).unwrap();
        let last = plot.iter().rposition(|&count| count > 0).unwrap();
        let inner = &plot[first + 1..last];
        let fullest = *inner.iter().max().unwrap() as f32;
        inner
            .windows(2)
            .all(|pair| pair[0].abs_diff(pair[1]) as f32 <= tolerance * fullest)
    }

    /// A picture spread evenly over its axis comes out even through a
    /// stretch that puts eight and a half fine bins to a coarse one, rather
    /// than eight and nine by turns: the beat between the two grids is the
    /// rebinning's, not the picture's.
    #[test]
    fn an_even_picture_comes_out_even_through_a_stretch() {
        let fine = fine_with(
            0.0,
            1.0,
            (0..FINE_BINS).map(|index| (index as f32 / (FINE_BINS - 1) as f32, 100)),
        );
        let stretched = fine.rebinned([0.0, 1.0], |value| value * 16.0 / 8.5);
        assert!(smooth(&stretched.luma, 0.02), "{:?}", &stretched.luma[..40]);
    }

    /// An 8-bit file's codes on an axis wider than 0..1 — a wide-gamut
    /// file's, carried into the working space — are a bin and a half apart,
    /// and land one and two to a bin by turns where each is dropped in the
    /// bin its middle falls in: a comb on a plot of every code there is.
    #[test]
    fn codes_on_a_wider_axis_do_not_comb() {
        let mut fine = fine_with(-0.4, 1.04, (0..=255).map(|code| (code as f32 / 255.0, 100)));
        fine.code = Some(1.0 / 255.0);
        let plot = fine.rebinned([-0.4, 1.04], |value| value);
        assert!(smooth(&plot.luma, 0.1), "{:?}", &plot.luma[70..110]);
    }

    /// A stage that pulls an 8-bit file's codes further apart than a bin
    /// leaves the bins between them empty: the posterization the plot is
    /// there to show, which the spread stops short of filling in.
    #[test]
    fn codes_pulled_apart_leave_the_bins_between_them_empty() {
        let mut fine = fine_with(0.0, 1.0, (0..=255).map(|code| (code as f32 / 255.0, 100)));
        fine.code = Some(1.0 / 255.0);
        let tripled = fine.rebinned([0.0, 1.0], |value| value * 3.0);
        let empty = tripled.luma[1..BINS - 1]
            .iter()
            .filter(|&&count| count == 0)
            .count();
        assert!(empty > 100, "{empty} empty bins");
    }

    /// The shares a rebinned plot clips at black and white are the shares
    /// the plot itself clips at the window those come from, on a quantized
    /// file, to within the rounding of a share of a pixel in each bin.
    #[test]
    fn a_rebinned_plot_clips_what_the_window_clips() {
        let codes: Vec<u8> = (0..=255).collect();
        let stats = Stats::scan(&srgb_gray_u8(codes));
        let (black, white) = (40.0 / 255.0, 200.0 / 255.0);
        let windowed = stats.fine.rebinned([0.0, 1.0], |value| {
            Transfer::Srgb.to_encoded(
                (Transfer::Srgb.to_linear(value) - Transfer::Srgb.to_linear(black))
                    / (Transfer::Srgb.to_linear(white) - Transfer::Srgb.to_linear(black)),
            )
        });
        let [windowed, plot] = [windowed.clipped(0.0, 1.0), stats.plot.clipped(black, white)];
        for (windowed, plot) in windowed.into_iter().zip(plot) {
            assert!((windowed - plot).abs() < 1e-3, "{windowed} against {plot}");
        }
    }

    #[test]
    fn non_finite_samples_do_not_poison_the_range() {
        let image = DecodedImage {
            width: 4,
            height: 1,
            samples: Samples::F32 {
                channels: Channels::Gray,
                data: vec![0.25, f32::NAN, f32::INFINITY, 0.75],
            },
            color: ColorSpace::LINEAR_BT709,
            alpha: AlphaMode::Opaque,
            referred: Referred::Scene,
            exposure: None,
            nodata: None,
            gain_map: None,
            depth: None,
            opens_on: None,
            sensor: None,
        };
        let stats = Stats::scan(&image);
        assert!((stats.min - 0.25).abs() < 1e-6);
        assert!((stats.max - 0.75).abs() < 1e-6);
        assert_eq!(stats.counted, 2);
    }

    /// The no-data value is no measurement: an integer file's code for it is
    /// taken out of every tally, and a float's value out of every count, so
    /// the extremes, the percentiles and the plots are of the data alone.
    #[test]
    fn the_no_data_value_is_left_out_everywhere() {
        let mut image = linear_gray(vec![0, 1000, 2000, 4095]);
        image.nodata = Some(0.0);
        let (stats, stored) = Stats::scan_both(&image);
        assert!((stats.min - 1000.0 / 65535.0).abs() < 1e-6, "{}", stats.min);
        assert_eq!(stats.counted, 3);
        assert!((stored.planes[0].min - 1000.0 / 65535.0).abs() < 1e-6);

        let mut floats = measured(linear_gray_f32(vec![-9999.0, 1.0, 2.0, 3.0]));
        floats.nodata = Some(-9999.0);
        let (stats, stored) = Stats::scan_both(&floats);
        assert_eq!((stats.min, stats.max), (1.0, 3.0));
        assert_eq!(stats.counted, 3);
        assert_eq!((stored.planes[0].min, stored.planes[0].max), (1.0, 3.0));
        assert_eq!(stored.planes[0].range, [1.0, 3.0]);
    }

    /// A photograph's integer channels share the whole range their samples
    /// can hold; a measurement's each get their own, trimmed of strays
    /// where there are any and the whole of it where there are not; and
    /// light in stops shares one axis read off every channel.
    #[test]
    fn stored_channels_are_plotted_over_the_range_their_reading_asks_for() {
        let raw = DecodedImage {
            samples: Samples::U16 {
                channels: Channels::Rgb,
                data: vec![100, 2000, 300, 4000, 50, 900],
            },
            width: 2,
            height: 1,
            color: ColorSpace {
                transfer: Transfer::Linear,
                primaries: Primaries::Bt2020,
            },
            referred: Referred::Display,
            ..linear_gray(vec![0])
        };
        let stored = Stored::scan(&raw);
        assert!(stored.shared);
        // Linear light, in stops: the top at white, the floor under the
        // darkest channel.
        assert_eq!(stored.axis, Axis::Stops);
        let [floor, top] = stored.shared_range().expect("one axis");
        assert_eq!(top, 0.0);
        assert!(floor <= (50.0f32 / f32::from(u16::MAX)).log2().floor());
        assert_eq!(stored.planes.len(), 3);
        let full = f32::from(u16::MAX);
        assert_eq!(stored.planes[0].min, 100.0 / full);
        assert_eq!(stored.planes[0].max, 4000.0 / full);
        assert_eq!(stored.planes[2].max, 900.0 / full);

        // The same numbers read as data: each channel its own range.
        let data = measured(raw);
        let stored = Stored::scan(&data);
        assert!(!stored.shared);
        assert_eq!(stored.planes[0].range, [100.0 / full, 4000.0 / full]);
        assert_eq!(stored.planes[1].range, [50.0 / full, 2000.0 / full]);
        assert_eq!(stored.planes[2].range, [300.0 / full, 900.0 / full]);
        let plotted = stored.planes[2].plot(stored.axis);
        assert!(plotted[0] > 0 && plotted[BINS - 1] > 0, "{plotted:?}");

        // A measurement with a stray: plotted over the trimmed range, the
        // stray piled into the end bin, the extremes still exact.
        let mut values: Vec<f32> = (0..2000).map(|i| 290.0 + i as f32 / 200.0).collect();
        values.push(1e9);
        let stored = Stored::scan(&measured(linear_gray_f32(values)));
        let plane = &stored.planes[0];
        assert_eq!(plane.max, 1e9);
        assert!(plane.range[1] < 301.0, "{:?}", plane.range);
        assert!(
            plane.range[0] >= 290.0 && plane.range[0] < 291.0,
            "{:?}",
            plane.range
        );
        let plotted = plane.plot(stored.axis);
        assert!(plotted[BINS - 1] >= ONE, "the stray is at the end");
        // Over the range the bins are filled from the pixels rather than
        // from the tally, so the ramp is even rather than combed.
        assert!(smooth(&plotted, 0.5), "{plotted:?}");
        // And one with no strays is plotted whole.
        let whole = Stored::scan(&measured(linear_gray_f32(vec![290.0, 295.0, 300.0])));
        assert_eq!(whole.planes[0].range, [290.0, 300.0]);
    }

    /// A pixel poured whole stays whole: a count spread over a stretch of
    /// bins comes out as exactly that count, however the shares round.
    #[test]
    fn whole_pixels_are_spread_without_loss() {
        for count in [1u32, 2, 3, 7, 100] {
            for (from, to) in [(0.3f32, 2.7f32), (5.0, 5.0), (-3.0, 1.2), (250.4, 260.0)] {
                let mut bins = vec![0u32; BINS];
                spread_whole(&mut bins, from, to, count);
                assert_eq!(bins.iter().sum::<u32>(), count, "{count} over {from}..{to}");
            }
        }
        let mut bins = vec![0u32; 8];
        spread_whole(&mut bins, 1.9, 3.1, 1);
        assert_eq!(bins[2] + bins[3], 1);
        spread_whole(&mut bins, 1.9, 2.4, 1);
        assert_eq!(bins[2], 2, "mostly in bin 2");
    }
}
