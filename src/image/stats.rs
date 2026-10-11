//! Value statistics, used to choose a sensible window for images whose
//! numbers do not conveniently fill 0..1 — 12-bit sensor data stored in
//! 16-bit containers, or HDR frames with a few very bright highlights.

use std::sync::Arc;

use super::gain_map::Table;
use super::{Channels, DecodedImage, Reader, Referred, Sample, Samples, Transfer};

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

/// At most this many pixels are examined; larger images are sampled on a
/// stride. Enough for stable percentiles, fast enough to run on every load.
const MAX_SAMPLED_PIXELS: usize = 1 << 21;

/// Number of color components a pixel carries, alpha aside.
pub const COLOR: usize = 3;

/// The share of the lit pixels left out at each end when the key of a scene
/// is read: the darkest, so that a floor of near-nothing — a render's
/// residue where a bounce all but died, a merge's darkest exposure divided
/// out — does not pull the meter down by however much of the frame it
/// covers, and the brightest, which are the light sources the curve is
/// for. A meter reads the scene between them.
const KEY_TRIM: f64 = 0.05;

/// The bins the lit pixels' luminance is counted in for the key, in stops:
/// every finite positive `f32` has a logarithm in `LOG_FLOOR..LOG_CEIL`,
/// and [`LOG_BINS_PER_STOP`] of them to the stop is finer than any meter
/// reads. Fixed rather than fitted to the picture so that the range pass
/// can bin as it goes.
const LOG_FLOOR: f32 = -150.0;
const LOG_CEIL: f32 = 128.0;
const LOG_BINS_PER_STOP: usize = 8;
const LOG_BINS: usize = (LOG_CEIL - LOG_FLOOR) as usize * LOG_BINS_PER_STOP;

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
    /// light in it — zero, or less — below every floor. For light stored
    /// linear as floats, which has no curve of its own and no top: a
    /// Radiance picture runs from nothing to a few hundred, and almost all
    /// of it is in the bottom hundredth, where a linear axis would have it
    /// in the first bin. Spaced in stops, the shadows get the room the eye
    /// gives them, a stop of exposure moves every value the same distance,
    /// and a window is a stretch of the axis rather than a sliver of it.
    /// Not for a measurement, whose values are quantities rather than
    /// light and may be negative, and whose stops no one counts.
    Stops,
}

impl Axis {
    /// The axis `image` is plotted on. The scan settles on a linear axis
    /// instead where [`Axis::Stops`] has nothing to stand on — a picture
    /// with no light in it — so what is read from is [`Stats::axis`].
    pub fn of(image: &DecodedImage) -> Self {
        let transfer = image.color.transfer;
        let floats = matches!(image.samples, Samples::F32 { .. });
        if floats && transfer.is_linear() && image.referred != Referred::Measured {
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
/// luminance is binned by its logarithm in `log_counts`, as the range pass
/// bins it: the top is the first whole stop at or above the brightest
/// channel, and the floor is the darkest lit stop after the bottom
/// [`FLOOR_TRIM`] is left off, no more than [`STOPS_REACH`] under the top
/// and at least a stop under it, so that the axis always has a span.
/// `None` where nothing reaches above zero, which has no top.
fn stops_axis(highest: f32, log_counts: &[u32]) -> Option<(f32, f32)> {
    // Written so that a `highest` that is not a number has no top either.
    let top = highest.log2().ceil();
    if !top.is_finite() {
        return None;
    }
    let total: u64 = log_counts.iter().map(|&count| u64::from(count)).sum();
    let trim = (total as f64 * FLOOR_TRIM) as u64;
    let mut seen = 0u64;
    let darkest = log_counts
        .iter()
        .position(|&count| {
            seen += u64::from(count);
            count > 0 && seen > trim
        })
        .map(|bin| LOG_FLOOR + bin as f32 / LOG_BINS_PER_STOP as f32)
        .unwrap_or(f32::NEG_INFINITY);
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
    /// Counts over `min..max` for the luminance of each sampled pixel.
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
    /// range it measured — which a strided scan can miss, and which the plot
    /// therefore does not cover.
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
        // Written so that a non-finite axis fails the test as well: every
        // comparison against a NaN is false, and the positive form would let
        // one through to divide by it.
        let span = self.max - self.min;
        if !span.is_finite() || span <= f32::MIN_POSITIVE {
            return None;
        }
        let bin = (stored - self.min) * ((BINS - 1) as f32 / span) + 0.5;
        (0.0..BINS as f32).contains(&bin).then_some(bin as usize)
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

    fn merge(mut self, other: &Self) -> Self {
        fn add(into: &mut [u32], from: &[u32]) {
            for (sum, count) in into.iter_mut().zip(from) {
                *sum += count;
            }
        }
        add(&mut self.luma, &other.luma);
        if let (Some(planes), Some(others)) = (self.color.as_mut(), &other.color) {
            for (plane, other) in planes.iter_mut().zip(others) {
                add(plane, other);
            }
        }
        self
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
                let width = to - from;
                // A stretch the stage flattens to a point — the run past a
                // window's end — is the one bin it lands in.
                if width <= 0.0 {
                    spread[(from.round().clamp(0.0, last)) as usize] += f64::from(count);
                    continue;
                }
                let first = (from + 0.5).floor().clamp(0.0, last) as usize;
                let final_bin = (to + 0.5).floor().clamp(0.0, last) as usize;
                for (bin, share) in spread
                    .iter_mut()
                    .enumerate()
                    .take(final_bin + 1)
                    .skip(first)
                {
                    // The end bins reach out past the axis, so that what
                    // lands off it piles there.
                    let left = if bin == 0 { from } else { bin as f32 - 0.5 };
                    let right = if bin == BINS - 1 {
                        to
                    } else {
                        bin as f32 + 0.5
                    };
                    let overlap = right.min(to) - left.max(from);
                    if overlap > 0.0 {
                        *share += f64::from(count) * f64::from(overlap / width);
                    }
                }
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

/// What [`Fine::rebinned`] counts a pixel as: sixteen shares, so that a
/// pixel spread over two or three bins keeps a share in each. A plot is read
/// by its bins against one another and against their sum, which the unit
/// leaves alone.
pub const SHARES: f64 = 16.0;

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
    /// Counts over `min..max`, in linear units, for percentiles.
    pub histogram: [u32; BINS],
    pub counted: u32,
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
    /// The same pixels binned for drawing. See [`Plot`] for why it is a
    /// second scan rather than the same one.
    pub plot: Plot,
    /// The same pixels on the same axis in finer bins, for what a stage
    /// after the file makes of them. Shared, since a clone of the stats is
    /// made at every arrival and is otherwise a copy of 64 KiB; what is
    /// rebinned from it is cached by the address of this.
    pub fine: Arc<Fine>,
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
    ///
    /// Two passes over the pixels, each divided by rows between rayon's
    /// threads: the range first, then the counts, which cannot be binned
    /// until the range is known. Every pixel is measured on its own, so a
    /// band's numbers depend on nothing outside it, and the bands fold
    /// together into exactly what one thread would have counted.
    pub fn scan(image: &DecodedImage) -> Self {
        Self::scan_with(image, None)
    }

    /// [`Stats::scan`], with the picture lifted through `lift` where it has
    /// a gain map: what the screen shows on a surface with room above
    /// white, which is what the histogram is to describe. With no table,
    /// or no map, the base is what is measured.
    pub fn scan_with(image: &DecodedImage, lift: Option<&Table>) -> Self {
        let stride = Self::stride(image);
        let reader = Reader::tabulated(image, lift);
        Self::scan_in(image, &reader, stride, Values::bands_for(image, stride))
    }

    /// The picture as the file stores it, rather than as the display starts
    /// from it: no lift, and a wide-gamut file's color in its own primaries
    /// rather than carried into the working space's. What the histogram's
    /// plot of the file is, so that a P3 red is at the top of the file's
    /// range there and past it only further up the panel, where the working
    /// space has taken it.
    ///
    /// `None` where the reader would move no color — a gray file, or a
    /// BT.709 one with no gain map — since the picture as stored is then
    /// what [`Stats::scan`] measured, and is not worth a second walk.
    ///
    /// A second walk over the picture for every other file, and for every
    /// frame of a wide-gamut animation as it is decoded; on a thread of its
    /// own either way, beside the scan it doubles.
    pub fn scan_as_stored(image: &DecodedImage) -> Option<Self> {
        if !image.color_moves() {
            return None;
        }
        let stride = Self::stride(image);
        let reader = Reader::as_stored(image);
        Some(Self::scan_in(
            image,
            &reader,
            stride,
            Values::bands_for(image, stride),
        ))
    }

    /// The picture as the file stores it, beside `shown`, the scan of it as
    /// it is shown: [`Stats::scan_as_stored`] where that is anything else,
    /// and `shown` itself where it is not.
    pub fn stored_beside(image: &DecodedImage, shown: &Stats) -> Self {
        Self::scan_as_stored(image).unwrap_or_else(|| shown.clone())
    }

    /// [`Stats::scan`] through `reader` over a given number of bands, so
    /// that a test can hold a divided scan against the same one undivided.
    fn scan_in(image: &DecodedImage, reader: &Reader, stride: usize, bands: usize) -> Self {
        let channels = image.samples.channels();
        let values = Values::new(image, reader, stride);
        let transfer = image.color.transfer;
        let nodata = image.nodata;
        let is_data =
            move |value: f32| value.is_finite() && nodata.is_none_or(|sentinel| value != sentinel);

        let color = !channels.is_gray();
        let plotted = if color { COLOR } else { 1 };
        let alpha = channels.alpha_index();
        let mut axis = Axis::of(image);
        let Range {
            min,
            max,
            peak,
            axis_min,
            axis_max,
            log_counts,
            log_sums,
        } = values
            .bands(bands, |band| {
                let mut range = Range::new(transfer);
                band.for_each(|encoded, linear| {
                    let value = luminance(linear, channels);
                    if is_data(value) {
                        range.min = range.min.min(value);
                        range.max = range.max.max(value);
                        let lit = value > 0.0 && alpha.is_none_or(|index| encoded[index] > 0.0);
                        if lit {
                            let log = value.log2();
                            let bin = ((log - LOG_FLOOR) * LOG_BINS_PER_STOP as f32) as usize;
                            let bin = bin.min(LOG_BINS - 1);
                            range.log_counts[bin] += 1;
                            range.log_sums[bin] += f64::from(log);
                        }
                    }
                    // The axis spans the channels that get plotted, in the
                    // units they are plotted in; the peak is the highest of
                    // them in linear ones.
                    for (stored, decoded) in encoded[..plotted].iter().zip(linear) {
                        if is_data(*decoded) {
                            range.axis_min = range.axis_min.min(*stored);
                            range.axis_max = range.axis_max.max(*stored);
                            range.peak = range.peak.max(*decoded);
                        }
                    }
                });
                range
            })
            .into_iter()
            .fold(Range::new(transfer), Range::merge);

        if !min.is_finite() || !max.is_finite() {
            // Nothing measurable at all; fall back to unit range so
            // the rest of the pipeline still has something usable.
            return Self {
                min: 0.0,
                max: 1.0,
                peak: 1.0,
                histogram: [0; BINS],
                counted: 0,
                key: None,
                axis: Axis::Curve(transfer),
                plot: Plot::empty(),
                fine: Arc::new(Fine::empty(0.0, 1.0, color)),
            };
        }

        // A stops axis is read off what was measured, from the darkest
        // the picture holds to say anything about up to its brightest; a
        // picture with nothing above zero has no top for one, and is
        // plotted on the linear axis its samples are in.
        let (axis_min, axis_max) = match axis {
            Axis::Stops => match stops_axis(axis_max, &log_counts) {
                Some(range) => range,
                None => {
                    axis = Axis::Curve(transfer);
                    (axis_min, axis_max)
                }
            },
            Axis::Curve(_) => (axis_min, axis_max),
        };

        let span = max - min;
        // A span at or below the smallest normal float is treated as flat:
        // dividing by a subnormal gives an infinite scale, which then bins
        // every sample into the two ends. The clamps downstream keep it from
        // going out of range, but the plot it draws is a lie.
        let scale = (span > f32::MIN_POSITIVE).then(|| (BINS - 1) as f32 / span);

        let axis_span = axis_max - axis_min;
        // A flat image spans nothing to plot against; leaving the counts at
        // zero draws an empty panel rather than one misleading spike.
        let fine_scale =
            (axis_span > f32::MIN_POSITIVE).then(|| (FINE_BINS - 1) as f32 / axis_span);
        let mut counts = Counts::new(color, axis_min, axis_max);
        if scale.is_some() || fine_scale.is_some() {
            counts = values
                .bands(bands, |band| {
                    let mut counts = Counts::new(color, axis_min, axis_max);
                    band.for_each(|encoded, linear| {
                        let value = luminance(linear, channels);
                        let is_data_value = is_data(value);
                        if let Some(scale) = scale
                            && is_data_value
                        {
                            // Rounded, not truncated: bins are sample points
                            // spread from `min` to `max`, which is how
                            // `percentile` reads them back, and what lands a
                            // code on its own bin without a rounding error
                            // stealing the boundary.
                            let bin = ((value - min) * scale + 0.5) as usize;
                            counts.histogram[bin.min(BINS - 1)] += 1;
                            counts.counted += 1;
                        }
                        if let Some(fine_scale) = fine_scale {
                            // Rounded, as the luminance is: a fine bin is
                            // the sample point at its middle. A value under
                            // a stops axis's floor — minus infinity, for no
                            // light at all — is the first bin, which is
                            // where the cast saturates to.
                            let bin = |fine: &mut [u32], stored: f32| {
                                let index = ((stored - axis_min) * fine_scale + 0.5) as usize;
                                fine[index.min(FINE_BINS - 1)] += 1;
                            };
                            if is_data_value {
                                // Luminance is a linear quantity; it goes
                                // onto the axis the same way the samples
                                // themselves did.
                                bin(&mut counts.fine.luma, axis.place(value));
                            }
                            if let Some(fine) = counts.fine.color.as_mut() {
                                for (plane, (stored, decoded)) in
                                    encoded[..COLOR].iter().zip(linear).enumerate()
                                {
                                    if is_data(*decoded) {
                                        bin(&mut fine[plane], axis.place_stored(*stored));
                                    }
                                }
                            }
                        }
                    });
                    counts
                })
                .into_iter()
                .fold(counts, Counts::merge);
        }

        counts.fine.code = match &image.samples {
            Samples::U8 { .. } => Some(1.0 / f32::from(u8::MAX)),
            Samples::U16 { .. } => Some(1.0 / f32::from(u16::MAX)),
            Samples::F32 { .. } => None,
        };
        let key = trimmed_log_mean(&log_counts, &log_sums).map(|log| log.exp2() as f32);

        Self {
            min,
            max,
            peak: if peak.is_finite() { peak } else { max },
            histogram: counts.histogram,
            counted: counts.counted,
            key,
            axis,
            // The plot is the fine bins on the same axis, spread as any
            // other stage's plot of them is, so that a quantized file's codes
            // do not comb against bins of another width.
            plot: counts.fine.rebinned([axis_min, axis_max], |value| value),
            fine: Arc::new(counts.fine),
        }
    }

    fn stride(image: &DecodedImage) -> usize {
        let pixels = image.width as usize * image.height as usize;
        pixels.div_ceil(MAX_SAMPLED_PIXELS).max(1)
    }

    /// The value below which `fraction` of the samples fall, `fraction` in
    /// 0..=1. Returns `min`/`max` at the extremes.
    pub fn percentile(&self, fraction: f32) -> f32 {
        if self.counted == 0 || self.max <= self.min {
            return if fraction <= 0.0 { self.min } else { self.max };
        }
        let target = (fraction.clamp(0.0, 1.0) * self.counted as f32) as u32;
        let mut running = 0u32;
        for (index, count) in self.histogram.iter().enumerate() {
            running += count;
            if running >= target {
                let position = index as f32 / (BINS - 1) as f32;
                return self.min + position * (self.max - self.min);
            }
        }
        self.max
    }
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

/// What the first pass measures: the extremes of the luminance and the
/// highest channel, in linear units, and the plotted channels' extremes on
/// the file's own curve.
#[derive(Clone)]
struct Range {
    min: f32,
    max: f32,
    peak: f32,
    axis_min: f32,
    axis_max: f32,
    /// The lit pixels' luminance binned by its logarithm, [`LOG_BINS`]
    /// wide, and the logarithms themselves summed bin by bin — wide, since
    /// two million of them are added up — so that the key can be read
    /// between the two trims without a second walk. See [`Stats::key`].
    log_counts: Vec<u32>,
    log_sums: Vec<f64>,
}

impl Range {
    /// Nothing seen yet. A curved file's axis starts at the range such a
    /// file nominally holds, and only widens; a linear one's is whatever is
    /// measured. See [`Plot`].
    fn new(transfer: Transfer) -> Self {
        let (axis_min, axis_max) = if transfer.is_linear() {
            (f32::INFINITY, f32::NEG_INFINITY)
        } else {
            (0.0, 1.0)
        };
        Self {
            min: f32::INFINITY,
            max: f32::NEG_INFINITY,
            peak: f32::NEG_INFINITY,
            axis_min,
            axis_max,
            log_counts: vec![0; LOG_BINS],
            log_sums: vec![0.0; LOG_BINS],
        }
    }

    fn merge(self, other: Self) -> Self {
        Self {
            min: self.min.min(other.min),
            max: self.max.max(other.max),
            peak: self.peak.max(other.peak),
            axis_min: self.axis_min.min(other.axis_min),
            axis_max: self.axis_max.max(other.axis_max),
            log_counts: self
                .log_counts
                .iter()
                .zip(&other.log_counts)
                .map(|(mine, theirs)| mine + theirs)
                .collect(),
            log_sums: self
                .log_sums
                .iter()
                .zip(&other.log_sums)
                .map(|(mine, theirs)| mine + theirs)
                .collect(),
        }
    }
}

/// The mean of the binned logarithms between the two trims: the darkest
/// [`KEY_TRIM`] of the pixels and the brightest are left out, by count, and
/// a bin the trim falls inside contributes the share of itself that is
/// kept, taken at the bin's own mean. `None` where nothing was lit.
fn trimmed_log_mean(counts: &[u32], sums: &[f64]) -> Option<f64> {
    let total: u64 = counts.iter().map(|&count| u64::from(count)).sum();
    if total == 0 {
        return None;
    }
    let total = total as f64;
    let (keep_from, keep_to) = (total * KEY_TRIM, total * (1.0 - KEY_TRIM));
    let mut below = 0.0;
    let mut kept_sum = 0.0;
    let mut kept_count = 0.0;
    for (&count, &sum) in counts.iter().zip(sums) {
        if count == 0 {
            continue;
        }
        let count = f64::from(count);
        let above = below + count;
        let kept = (above.min(keep_to) - below.max(keep_from)).max(0.0);
        if kept > 0.0 {
            kept_sum += sum * (kept / count);
            kept_count += kept;
        }
        below = above;
    }
    // The trims leave nine tenths of the pixels between them whatever the
    // count, so there is always something kept.
    Some(kept_sum / kept_count)
}

/// What the second pass counts: the bins of [`Stats`], of its [`Plot`] and
/// of its [`Fine`].
struct Counts {
    histogram: [u32; BINS],
    counted: u32,
    fine: Fine,
}

impl Counts {
    fn new(color: bool, axis_min: f32, axis_max: f32) -> Self {
        Self {
            histogram: [0; BINS],
            counted: 0,
            fine: Fine::empty(axis_min, axis_max, color),
        }
    }

    fn merge(mut self, other: Self) -> Self {
        fn add(into: &mut [u32; BINS], from: &[u32; BINS]) {
            for (sum, count) in into.iter_mut().zip(from) {
                *sum += count;
            }
        }
        add(&mut self.histogram, &other.histogram);
        self.counted += other.counted;
        self.fine = self.fine.merge(&other.fine);
        self
    }
}

/// Sampled pixels below which a pass is not worth dividing: handing bands to
/// the pool costs more than a picture this small takes to walk.
const PARALLEL_FROM: usize = 1 << 16;

/// The fewest sampled pixels a band is given, so that a picture just over
/// [`PARALLEL_FROM`] goes to two or three threads rather than thirty-two.
const BAND_PIXELS: usize = 1 << 15;

/// Iterator over each sampled pixel, as stored and as decoded.
#[derive(Clone)]
struct Values<'a> {
    /// The pipeline every pixel is read through, set up once for the scan.
    reader: &'a Reader<'a>,
    transfer: Transfer,
    stride: usize,
    /// The rows this walk covers, as a range of pixel indices: the whole
    /// image, or one band of it.
    pixels: std::ops::Range<usize>,
    width: usize,
}

impl<'a> Values<'a> {
    fn new(image: &'a DecodedImage, reader: &'a Reader<'a>, stride: usize) -> Self {
        let width = image.width as usize;
        Self {
            reader,
            transfer: image.color.transfer,
            stride,
            pixels: 0..width * image.height as usize,
            width,
        }
    }

    /// How many ways to cut the walk: one for a picture too small to be
    /// worth the threads, otherwise one per thread the pool will run at
    /// once, and never more than there are rows to give them.
    fn bands_for(image: &DecodedImage, stride: usize) -> usize {
        let height = image.height as usize;
        let sampled = image.width as usize * height / stride.max(1);
        if image.width == 0 || height == 0 || sampled < PARALLEL_FROM {
            return 1;
        }
        (sampled / BAND_PIXELS)
            .clamp(1, rayon_core::current_num_threads().max(1))
            .min(height)
    }

    /// The walk cut into `bands` bands of whole rows, and `pass` run over
    /// each on rayon's pool. The results come back in row order, but nothing
    /// a band measures depends on its neighbors, so the order does not
    /// matter.
    ///
    /// A band walks its own rows on the whole image's stride, in the whole
    /// image's phase, so that the bands together land on exactly the pixels
    /// one walk would have. What the scan measures then does not depend on
    /// how many threads the machine has.
    fn bands<T: Send>(&self, bands: usize, pass: impl Fn(Values<'a>) -> T + Sync) -> Vec<T> {
        let height = self.pixels.len() / self.width.max(1);
        let bands = bands.clamp(1, height.max(1));
        if bands == 1 {
            return vec![pass(self.clone())];
        }

        // Whole rows each, so rounding up the rows can leave fewer bands
        // than asked for; the count follows the rows, not the other way.
        let rows = height.div_ceil(bands);
        let bands = height.div_ceil(rows);
        let mut results: Vec<Option<T>> = (0..bands).map(|_| None).collect();
        rayon_core::scope(|scope| {
            for (index, slot) in results.iter_mut().enumerate() {
                let first = self.pixels.start + index * rows * self.width;
                let last = (first + rows * self.width).min(self.pixels.end);
                let band = Values {
                    pixels: first..last,
                    ..self.clone()
                };
                let pass = &pass;
                scope.spawn(move |_| *slot = Some(pass(band)));
            }
        });
        results
            .into_iter()
            .map(|result| result.expect("every band was walked"))
            .collect()
    }

    /// Calls `visit` with one pixel's components twice over: first on the
    /// file's own curve, normalized to 0..1 for integer samples, then
    /// decoded to the linear working space — lifted and carried into its
    /// primaries by the reader, with the stored side put back on the curve
    /// to match where either moved it, so that the color channels are
    /// plotted where the file would have stored what the screen shows:
    /// past the top of the file's own range where the lift or the matrix
    /// takes them, and below the bottom where the matrix does. Alpha is
    /// included where the image has one, since callers slice down to what
    /// they want.
    fn for_each(self, mut visit: impl FnMut(&[f32], &[f32])) {
        let reader = self.reader;
        let channels = reader.channels();
        let count = channels.count();
        let colors = channels.color_count();
        let scale = reader.scale();
        let moved = reader.moves_color();
        let mut stored = [0.0f32; 4];
        // From the first pixel of the band the whole image's stride would
        // land on, not from the band's own first pixel.
        let first = self
            .pixels
            .start
            .next_multiple_of(self.stride.max(1))
            .min(self.pixels.end);
        for pixel in (first..self.pixels.end).step_by(self.stride) {
            let (x, y) = ((pixel % self.width) as u32, (pixel / self.width) as u32);
            let (raw, linear) = reader.decode(x, y);
            for (slot, value) in stored.iter_mut().zip(&raw[..count]) {
                *slot = *value * scale;
            }
            if moved {
                for (stored, linear) in stored.iter_mut().zip(&linear).take(colors) {
                    *stored = self.transfer.to_encoded(*linear);
                }
            }
            visit(&stored[..count], &linear[..count]);
        }
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
        };
        let key = Stats::scan(&element).key.unwrap();
        assert!((key - 0.5).abs() < 1e-6, "{key}");

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

    /// Dividing the scan between threads must not change a count. Held
    /// against the same scan on one band, over an image with a stride of
    /// one so that every pixel is looked at, and again on a stride so that
    /// the bands are shown to keep its phase; both fold to exactly what a
    /// plain walk would have — bins, extremes and the pixel count.
    #[test]
    fn a_divided_scan_agrees_with_a_plain_one() {
        let (width, height) = (97u32, 61u32);
        // Something with structure in every band: a gradient across, a ramp
        // down, and a stripe of nodata-free NaNs the scan has to skip.
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
        let image = DecodedImage {
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
        };

        for stride in [1, 5] {
            let reader = Reader::tabulated(&image, None);
            let plain = Stats::scan_in(&image, &reader, stride, 1);
            assert!(plain.counted > 0);
            for bands in [2, 7, 60, 200] {
                let divided = Stats::scan_in(&image, &reader, stride, bands);
                assert_eq!(divided.min, plain.min, "stride {stride}, {bands} bands");
                assert_eq!(divided.max, plain.max, "stride {stride}, {bands} bands");
                assert_eq!(divided.plot.min, plain.plot.min);
                assert_eq!(divided.plot.max, plain.plot.max);
                assert_eq!(divided.counted, plain.counted, "{bands} bands");
                assert_eq!(divided.histogram, plain.histogram, "{bands} bands");
                assert_eq!(divided.plot.luma, plain.plot.luma, "{bands} bands");
                assert_eq!(divided.plot.color, plain.plot.color, "{bands} bands");
                assert_eq!(divided.fine.luma, plain.fine.luma, "{bands} bands");
                assert_eq!(divided.fine.color, plain.fine.color, "{bands} bands");
            }
        }
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
        let stored = Stats::scan_as_stored(&red(Primaries::DisplayP3)).expect("P3 moves color");
        assert_eq!((stored.plot.min, stored.plot.max), (0.0, 1.0));
        let planes = stored.plot.color.expect("three planes");
        assert_eq!(planes[0][BINS - 1], ONE, "red at the top");
        assert_eq!(planes[1][0], ONE, "green at the bottom");
        // And a BT.709 file is stored as it is shown, so is not scanned
        // again.
        assert!(Stats::scan_as_stored(&red(Primaries::Bt709)).is_none());
        assert!(Stats::scan_as_stored(&srgb_gray_u8(vec![0, 255])).is_none());
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

    /// Linear samples have no curve to plot against, so the axis stays in
    /// the units the file measured in.
    #[test]
    fn scene_linear_images_keep_a_linear_axis() {
        let stats = Stats::scan(&linear_gray(vec![0, 16384, 32768, 65535]));
        assert!(stats.plot.min.abs() < 1e-6);
        assert!((stats.plot.max - 1.0).abs() < 1e-6);
        // A quarter of full scale bins a quarter of the way along, which on
        // a curve it would not: sRGB would put it past the half way mark. A
        // quarter of 255 bins is between two of them, and shared by both.
        let quarter = (0.25 * (BINS - 1) as f32).floor() as usize;
        assert_eq!(stats.plot.luma[quarter] + stats.plot.luma[quarter + 1], ONE);
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
        };
        let stats = Stats::scan(&image);
        assert!((stats.min - 0.25).abs() < 1e-6);
        assert!((stats.max - 0.75).abs() < 1e-6);
    }
}
