//! A lossless JPEG — ITU T.81's process 14, a frame that starts with SOF3 —
//! decoded whole, as a tile of a DNG is. The format is the one the DNG
//! specification names for a compression of 7: each sample predicted from
//! the ones to its left and above by one of seven predictors, and the
//! difference Huffman-coded as a category and that many bits. A tile is
//! small enough to decode on one thread, and the tiles are decoded in
//! parallel by the caller, which is why LibRaw does not do this: its
//! decoder walks the tiles one after another on one thread, and a
//! 48-megapixel picture takes it a second and a half.
//!
//! Only what a DNG can hold is read: one scan of up to four components at
//! equal sampling, two to sixteen bits deep, with restart intervals of whole
//! rows. The frame's samples come back in the order the scan holds them —
//! row by row, a pixel's components together — which is how a DNG lays its
//! tile out whatever shape the frame says it is.

use anyhow::{Result, anyhow, bail};

/// The markers read, by T.81's numbers.
mod marker {
    pub const SOF3: u8 = 0xC3;
    pub const DHT: u8 = 0xC4;
    pub const RST0: u8 = 0xD0;
    pub const RST7: u8 = 0xD7;
    pub const SOI: u8 = 0xD8;
    pub const EOI: u8 = 0xD9;
    pub const SOS: u8 = 0xDA;
    pub const DRI: u8 = 0xDD;
}

/// The most components a scan may hold, and so the most Huffman tables.
const COMPONENTS: usize = 4;

/// How many bits of the stream a table lookup takes at once: a code this
/// long or shorter is found in one read, and a longer one bit by bit. A
/// ten-bit picture's categories run to eleven, so its codes are rarely
/// longer than this.
const LOOKUP: u32 = 9;

/// The one frame in `codestream`, which must hold exactly `samples`
/// samples — the caller knows what the tile is, and the check comes before
/// anything is allocated for it — as the scan holds them: row by row, each
/// pixel's components together, every sample below `1 << precision`.
pub fn decode(codestream: &[u8], samples: usize) -> Result<Vec<u16>> {
    let mut reader = Markers {
        bytes: codestream,
        at: 0,
    };
    if reader.next()? != marker::SOI {
        bail!("not a JPEG: no SOI marker");
    }
    let mut frame: Option<Header> = None;
    let mut tables: [Option<Table>; COMPONENTS] = [None, None, None, None];
    let mut restart: usize = 0;
    loop {
        let kind = reader.next()?;
        let body = match kind {
            marker::EOI => bail!("the JPEG ends before its scan"),
            marker::RST0..=marker::RST7 => bail!("a restart marker before the scan"),
            _ => reader.segment()?,
        };
        match kind {
            marker::SOF3 => {
                if frame.is_some() {
                    bail!("two frames");
                }
                frame = Some(Header::parse(body, samples)?);
            }
            0xC0..=0xCF if kind != marker::DHT && kind != 0xC8 && kind != 0xCC => {
                bail!("a JPEG of process {:#x}, not lossless", kind);
            }
            marker::DHT => {
                let mut rest = body;
                while !rest.is_empty() {
                    let (id, table, after) = Table::parse(rest)?;
                    tables[id] = Some(table);
                    rest = after;
                }
            }
            marker::DRI => {
                let [high, low] = body
                    .first_chunk::<2>()
                    .ok_or_else(|| anyhow!("a short restart interval"))?;
                restart = usize::from(u16::from_be_bytes([*high, *low]));
            }
            marker::SOS => {
                let header = frame.ok_or_else(|| anyhow!("a scan before the frame"))?;
                let scan = Scan::parse(body, &header, &tables)?;
                return scan.decode(&header, &tables, &reader.bytes[reader.at..], restart);
            }
            // APPn, COM, DQT and whatever else a writer puts first.
            _ => {}
        }
    }
}

/// Walks the segments ahead of the scan.
struct Markers<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Markers<'_> {
    /// The next marker's code, past any fill bytes.
    fn next(&mut self) -> Result<u8> {
        if self.bytes.get(self.at) != Some(&0xFF) {
            bail!("a marker was expected at byte {}", self.at);
        }
        while self.bytes.get(self.at) == Some(&0xFF) {
            self.at += 1;
        }
        let code = *self
            .bytes
            .get(self.at)
            .ok_or_else(|| anyhow!("the JPEG ends inside a marker"))?;
        self.at += 1;
        Ok(code)
    }

    /// The body of the segment the last marker opened, past its length.
    fn segment(&mut self) -> Result<&[u8]> {
        let [high, low] = self.bytes[self.at..]
            .first_chunk::<2>()
            .ok_or_else(|| anyhow!("the JPEG ends inside a segment's length"))?;
        let length = usize::from(u16::from_be_bytes([*high, *low]));
        if length < 2 {
            bail!("a segment shorter than its own length");
        }
        let body = self
            .bytes
            .get(self.at + 2..self.at + length)
            .ok_or_else(|| anyhow!("the JPEG ends inside a segment"))?;
        self.at += length;
        Ok(body)
    }
}

/// What SOF3 says of the frame.
#[derive(Clone, Copy)]
struct Header {
    precision: u8,
    height: u16,
    width: u16,
    components: u8,
    /// Each component's identifier, in the frame's order.
    ids: [u8; COMPONENTS],
}

impl Header {
    fn parse(body: &[u8], samples: usize) -> Result<Self> {
        let [precision, h1, h0, w1, w0, components] = *body
            .first_chunk::<6>()
            .ok_or_else(|| anyhow!("a short frame header"))?;
        let (height, width) = (u16::from_be_bytes([h1, h0]), u16::from_be_bytes([w1, w0]));
        if !(2..=16).contains(&precision) {
            bail!("a precision of {precision} bits");
        }
        if width == 0 || height == 0 {
            bail!("a frame of {width}x{height}");
        }
        if components == 0 || usize::from(components) > COMPONENTS {
            bail!("a frame of {components} components");
        }
        let mut ids = [0; COMPONENTS];
        for (c, id) in ids.iter_mut().take(usize::from(components)).enumerate() {
            let [found, sampling, _quantization] = *body
                .get(6 + c * 3..9 + c * 3)
                .and_then(|spec| spec.first_chunk::<3>())
                .ok_or_else(|| anyhow!("a frame header short of its components"))?;
            if sampling != 0x11 {
                bail!("a component sampled at other than 1x1, which a lossless frame has none of");
            }
            *id = found;
        }
        let held = usize::from(width) * usize::from(height) * usize::from(components);
        if held != samples {
            bail!(
                "a frame of {width}x{height} by {components}, {held} samples where {samples} were \
                 expected"
            );
        }
        Ok(Self {
            precision,
            height,
            width,
            components,
            ids,
        })
    }
}

/// A Huffman table, as T.81's Annex C builds it from the counts of codes
/// of each length and the symbols in code order, with a lookup over the
/// first [`LOOKUP`] bits ahead of the bit-by-bit search.
struct Table {
    /// For each `LOOKUP`-bit prefix, the code's length and its symbol, or a
    /// length of zero where the code is longer than the prefix.
    lookup: Vec<(u8, u8)>,
    /// The largest code of each length, or -1 where there are none, and the
    /// place in `symbols` of the smallest code of that length less its
    /// value.
    maxcode: [i32; 17],
    valptr: [i32; 17],
    symbols: Vec<u8>,
}

impl Table {
    /// One table out of a DHT segment, and what follows it in the segment.
    fn parse(body: &[u8]) -> Result<(usize, Self, &[u8])> {
        let (&class_id, rest) = body
            .split_first()
            .ok_or_else(|| anyhow!("a short Huffman table"))?;
        let (class, id) = (class_id >> 4, usize::from(class_id & 0xF));
        if class != 0 {
            bail!("an AC Huffman table, which a lossless frame has no use for");
        }
        if id >= COMPONENTS {
            bail!("Huffman table {id}");
        }
        let (counts, rest) = rest
            .split_first_chunk::<16>()
            .ok_or_else(|| anyhow!("a Huffman table short of its counts"))?;
        let total: usize = counts.iter().map(|&count| usize::from(count)).sum();
        let (symbols, rest) = rest
            .split_at_checked(total)
            .ok_or_else(|| anyhow!("a Huffman table short of its symbols"))?;
        if let Some(symbol) = symbols.iter().find(|&&symbol| symbol > 16) {
            bail!("a difference category of {symbol}, past sixteen bits");
        }
        let mut lookup = vec![(0u8, 0u8); 1 << LOOKUP];
        let mut maxcode = [-1i32; 17];
        let mut valptr = [0i32; 17];
        let mut code: i32 = 0;
        let mut at = 0usize;
        for length in 1..=16u32 {
            let count = usize::from(counts[length as usize - 1]);
            if count > 0 {
                valptr[length as usize] = at as i32 - code;
                for symbol in &symbols[at..at + count] {
                    if code >= 1 << length {
                        bail!("a Huffman table whose codes run past their length");
                    }
                    if length <= LOOKUP {
                        let first = (code as usize) << (LOOKUP - length);
                        let spread = 1 << (LOOKUP - length);
                        for entry in &mut lookup[first..first + spread] {
                            *entry = (length as u8, *symbol);
                        }
                    }
                    code += 1;
                }
                at += count;
                maxcode[length as usize] = code - 1;
            }
            code <<= 1;
        }
        Ok((
            id,
            Self {
                lookup,
                maxcode,
                valptr,
                symbols: symbols.to_vec(),
            },
            rest,
        ))
    }

    /// The next symbol — a difference's category — off the stream, which
    /// holds at least sixteen bits.
    #[inline]
    fn symbol(&self, bits: &mut Bits) -> Result<u32> {
        let (length, symbol) = self.lookup[bits.peek(LOOKUP) as usize];
        if length != 0 {
            bits.consume(u32::from(length));
            return Ok(u32::from(symbol));
        }
        let ahead = bits.peek(16) as i32;
        for length in LOOKUP + 1..=16 {
            let code = ahead >> (16 - length);
            if code <= self.maxcode[length as usize] {
                bits.consume(length);
                let at = (self.valptr[length as usize] + code) as usize;
                return Ok(u32::from(self.symbols[at]));
            }
        }
        Err(anyhow!("a code the Huffman table does not hold"))
    }
}

/// What SOS says of the scan.
struct Scan {
    /// The Huffman table of each component, in the frame's order.
    tables: [usize; COMPONENTS],
    predictor: u8,
    /// The point transform: how many low bits each sample was shifted
    /// down by before coding.
    shift: u8,
}

impl Scan {
    fn parse(body: &[u8], frame: &Header, tables: &[Option<Table>]) -> Result<Self> {
        let (&count, rest) = body
            .split_first()
            .ok_or_else(|| anyhow!("a short scan header"))?;
        if count != frame.components {
            bail!(
                "a scan of {count} of the frame's {} components",
                frame.components
            );
        }
        let (specs, rest) = rest
            .split_at_checked(usize::from(count) * 2)
            .ok_or_else(|| anyhow!("a scan header short of its components"))?;
        let mut chosen = [0; COMPONENTS];
        for (c, spec) in specs.as_chunks::<2>().0.iter().enumerate() {
            let position = frame.ids[..usize::from(frame.components)]
                .iter()
                .position(|&id| id == spec[0])
                .ok_or_else(|| anyhow!("a scan of component {}, which the frame lacks", spec[0]))?;
            if position != c {
                bail!("a scan whose components are not in the frame's order");
            }
            let table = usize::from(spec[1] >> 4);
            if table >= COMPONENTS || tables[table].is_none() {
                bail!("a scan through Huffman table {table}, which was not given");
            }
            chosen[c] = table;
        }
        let [predictor, _end, transform] = *rest
            .first_chunk::<3>()
            .ok_or_else(|| anyhow!("a scan header short of its predictor"))?;
        if !(1..=7).contains(&predictor) {
            bail!("predictor {predictor}");
        }
        let shift = transform & 0xF;
        if shift >= frame.precision {
            bail!(
                "a point transform of {shift} bits on a precision of {}",
                frame.precision
            );
        }
        Ok(Self {
            tables: chosen,
            predictor,
            shift,
        })
    }

    /// The scan's samples, `data` being the entropy-coded segment and
    /// whatever follows it.
    fn decode(
        &self,
        frame: &Header,
        tables: &[Option<Table>; COMPONENTS],
        data: &[u8],
        restart: usize,
    ) -> Result<Vec<u16>> {
        let (width, height) = (usize::from(frame.width), usize::from(frame.height));
        let components = usize::from(frame.components);
        let tables: Vec<&Table> = self.tables[..components]
            .iter()
            .map(|&id| {
                tables[id].as_ref().ok_or_else(|| {
                    anyhow!("a scan through Huffman table {id}, which was not given")
                })
            })
            .collect::<Result<_>>()?;
        if restart != 0 && !restart.is_multiple_of(width) {
            bail!("a restart interval of {restart} pixels, which is not whole rows of {width}");
        }
        let restart_rows = if restart == 0 { 0 } else { restart / width };
        let depth = u32::from(frame.precision - self.shift);
        let mut samples = vec![0u16; width * height * components];
        let mut bits = Bits::new(data);
        let stride = width * components;
        for row in 0..height {
            let (before, from) = samples.split_at_mut(row * stride);
            let current = &mut from[..stride];
            let first = row == 0 || (restart_rows != 0 && row % restart_rows == 0);
            if first && row != 0 {
                bits.restart()?;
            }
            let above = (!first).then(|| &before[(row - 1) * stride..]);
            self.row(current, above, &tables, depth, &mut bits)?;
        }
        bits.finish()?;
        if self.shift != 0 {
            for sample in &mut samples {
                *sample <<= self.shift;
            }
        }
        Ok(samples)
    }

    /// One row of samples into `current`, from `above` where the row is
    /// not the first of the scan or of a restart interval.
    #[inline]
    fn row(
        &self,
        current: &mut [u16],
        above: Option<&[u16]>,
        tables: &[&Table],
        depth: u32,
        bits: &mut Bits,
    ) -> Result<()> {
        let components = tables.len();
        let full = 1u32 << depth;
        let next = |bits: &mut Bits, c: usize, prediction: i32| -> Result<u16> {
            bits.refill();
            let difference = tables[c].difference(bits)?;
            let sample = (prediction + difference) & 0xFFFF;
            if sample as u32 >= full {
                bail!("a sample of {sample} past the precision");
            }
            Ok(sample as u16)
        };
        match above {
            // The first row of a scan, or of a restart interval: half scale,
            // then each sample from the one to its left.
            None => {
                for (c, sample) in current[..components].iter_mut().enumerate() {
                    *sample = next(bits, c, 1 << (depth - 1))?;
                }
                // Which component each sample is, counted rather than
                // divided for, at every sample.
                let mut c = 0;
                for x in components..current.len() {
                    current[x] = next(bits, c, i32::from(current[x - components]))?;
                    c = if c + 1 == components { 0 } else { c + 1 };
                }
            }
            // Any other: the first pixel from the one above, and the rest
            // as the predictor says.
            Some(above) => {
                for (c, (sample, &up)) in current[..components].iter_mut().zip(above).enumerate() {
                    *sample = next(bits, c, i32::from(up))?;
                }
                let predict = PREDICTORS[usize::from(self.predictor)];
                let mut c = 0;
                for x in components..current.len() {
                    let (a, b, cc) = (
                        i32::from(current[x - components]),
                        i32::from(above[x]),
                        i32::from(above[x - components]),
                    );
                    current[x] = next(bits, c, predict(a, b, cc))?;
                    c = if c + 1 == components { 0 } else { c + 1 };
                }
            }
        }
        Ok(())
    }
}

/// T.81's seven predictors, by number, of the sample to the left, the one
/// above and the one above that; the zeroth is no prediction, which a
/// scan of a lossless frame may not choose.
type Predictor = fn(i32, i32, i32) -> i32;
const PREDICTORS: [Predictor; 8] = [
    |_, _, _| 0,
    |a, _, _| a,
    |_, b, _| b,
    |_, _, c| c,
    |a, b, c| a + b - c,
    |a, b, c| a + ((b - c) >> 1),
    |a, b, c| b + ((a - c) >> 1),
    |a, b, _| (a + b) >> 1,
];

impl Table {
    /// The next difference off the stream, which holds at least thirty-two
    /// bits: its category, then that many bits, the low half of each
    /// category standing for the negative differences.
    #[inline]
    fn difference(&self, bits: &mut Bits) -> Result<i32> {
        let category = self.symbol(bits)?;
        Ok(match category {
            0 => 0,
            16 => 32768,
            _ => {
                let value = bits.take(category) as i32;
                if value < 1 << (category - 1) {
                    value - (1 << category) + 1
                } else {
                    value
                }
            }
        })
    }
}

/// The entropy-coded segment read a bit at a time, from the top of a
/// sixty-four-bit window: a stuffed zero after `0xFF` is dropped, and a
/// marker stops the reading, zeros standing in for whatever is asked for
/// past it so that the one check at a restart or the end catches an
/// overrun.
struct Bits<'a> {
    data: &'a [u8],
    at: usize,
    window: u64,
    /// How many of the window's bits are real.
    held: u32,
    /// Whether a marker or the end of the data has been reached, and how
    /// many bits of zeros were fed past it.
    stopped: bool,
    fed: u32,
}

impl<'a> Bits<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            at: 0,
            window: 0,
            held: 0,
            stopped: false,
            fed: 0,
        }
    }

    /// Fills the window to at least fifty-six bits.
    #[inline]
    fn refill(&mut self) {
        while self.held <= 56 {
            let byte = if self.stopped {
                self.fed += 8;
                0
            } else {
                match self.data.get(self.at) {
                    Some(&0xFF) => {
                        if self.data.get(self.at + 1) == Some(&0) {
                            self.at += 2;
                            0xFF
                        } else {
                            self.stopped = true;
                            continue;
                        }
                    }
                    Some(&byte) => {
                        self.at += 1;
                        byte
                    }
                    None => {
                        self.stopped = true;
                        continue;
                    }
                }
            };
            self.window |= u64::from(byte) << (56 - self.held);
            self.held += 8;
        }
    }

    #[inline]
    fn peek(&self, count: u32) -> u32 {
        (self.window >> (64 - count)) as u32
    }

    #[inline]
    fn consume(&mut self, count: u32) {
        self.window <<= count;
        self.held -= count;
    }

    #[inline]
    fn take(&mut self, count: u32) -> u32 {
        let value = self.peek(count);
        self.consume(count);
        value
    }

    /// Whether any of the zeros fed past a marker or the end were taken as
    /// data.
    fn overran(&self) -> bool {
        self.fed > self.held
    }

    /// At a restart interval's end: the rest of the byte is dropped, the
    /// restart marker is passed, and the reading goes on after it.
    fn restart(&mut self) -> Result<()> {
        if self.overran() {
            bail!("the scan runs out before its restart interval does");
        }
        // The reading stops short of a marker, so what the window holds is
        // the rest of the interval's last byte, which is fill, and the
        // zeros fed past the marker. A whole byte of anything else is data
        // the interval had no room for.
        if !self.stopped && self.held >= 8 {
            bail!("data left over where a restart marker was expected");
        }
        let [0xFF, code] = *self.data[self.at..]
            .first_chunk::<2>()
            .ok_or_else(|| anyhow!("the scan ends where a restart marker was expected"))?
        else {
            bail!("no restart marker where the interval ends");
        };
        if !(marker::RST0..=marker::RST7).contains(&code) {
            bail!("marker {code:#x} where a restart marker was expected");
        }
        self.at += 2;
        self.window = 0;
        self.held = 0;
        self.stopped = false;
        self.fed = 0;
        Ok(())
    }

    /// At the scan's end.
    fn finish(&self) -> Result<()> {
        if self.overran() {
            bail!("the scan ends before the frame does");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A lossless JPEG of `samples`, `width` by `height` by `components`,
    /// written as the fixtures are, under one Huffman table that gives each
    /// category a code one bit longer than the last; with `predictor`,
    /// `shift` low bits dropped, and a restart every `restart` rows.
    #[allow(clippy::too_many_arguments)]
    fn encode(
        samples: &[u16],
        width: u16,
        height: u16,
        components: u8,
        precision: u8,
        predictor: u8,
        shift: u8,
        restart: u16,
    ) -> Vec<u8> {
        let (w, n) = (usize::from(width), usize::from(components));
        let mut bits: Vec<bool> = Vec::new();
        let put = |bits: &mut Vec<bool>, value: u32, count: u32| {
            for s in (0..count).rev() {
                bits.push((value >> s) & 1 == 1);
            }
        };
        let mut scan = Vec::new();
        let flush = |bits: &mut Vec<bool>, scan: &mut Vec<u8>| {
            while !bits.len().is_multiple_of(8) {
                bits.push(true);
            }
            for byte in bits.chunks(8) {
                let byte = byte
                    .iter()
                    .fold(0u8, |acc, &bit| (acc << 1) | u8::from(bit));
                scan.push(byte);
                if byte == 0xFF {
                    scan.push(0);
                }
            }
            bits.clear();
        };
        let shifted: Vec<i32> = samples.iter().map(|&s| i32::from(s >> shift)).collect();
        // The codes of the seventeen categories — a predictor of the
        // fourth through sixth kinds can put a prediction past the scale,
        // and the difference is modulo sixteen bits whatever the precision
        // — four of three bits and then one a bit longer than the last,
        // built as T.81's Annex C does.
        let categories = 17;
        let counts: [u8; 16] = [0, 0, 4, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1];
        let mut codes = vec![(0u32, 0u32); categories];
        let (mut code, mut symbol) = (0u32, 0usize);
        for (i, &n) in counts.iter().enumerate() {
            for _ in 0..n {
                codes[symbol] = (code, i as u32 + 1);
                code += 1;
                symbol += 1;
            }
            code <<= 1;
        }
        let mut restarts = 0u8;
        for y in 0..usize::from(height) {
            let first = y == 0 || (restart != 0 && y % usize::from(restart) == 0);
            if first && y != 0 {
                flush(&mut bits, &mut scan);
                scan.extend([0xFF, marker::RST0 + restarts % 8]);
                restarts += 1;
            }
            for x in 0..w {
                for c in 0..n {
                    let at = (y * w + x) * n + c;
                    let left = (x > 0).then(|| shifted[at - n]);
                    let above = (!first).then(|| shifted[at - w * n]);
                    let corner = (!first && x > 0).then(|| shifted[at - w * n - n]);
                    let prediction = match (left, above, corner) {
                        (None, None, _) => 1 << (precision - shift - 1),
                        (Some(a), None, _) => a,
                        (None, Some(b), _) => b,
                        (Some(a), Some(b), Some(c)) => PREDICTORS[usize::from(predictor)](a, b, c),
                        _ => unreachable!(),
                    };
                    // Modulo sixteen bits, into the signed half: -32768 is
                    // the one difference of category 16, which has no bits.
                    let difference = ((shifted[at] - prediction) & 0xFFFF) as i32;
                    let difference = if difference >= 32768 {
                        difference - 65536
                    } else {
                        difference
                    };
                    let category = 32 - difference.unsigned_abs().leading_zeros();
                    put(
                        &mut bits,
                        codes[category as usize].0,
                        codes[category as usize].1,
                    );
                    if category < 16 {
                        let coded = if difference < 0 {
                            difference + (1 << category) - 1
                        } else {
                            difference
                        };
                        put(&mut bits, coded as u32, category);
                    }
                }
            }
        }
        flush(&mut bits, &mut scan);
        let mut out = vec![0xFF, marker::SOI];
        let segment = |out: &mut Vec<u8>, code: u8, body: &[u8]| {
            out.extend([0xFF, code]);
            out.extend((body.len() as u16 + 2).to_be_bytes());
            out.extend_from_slice(body);
        };
        let mut frame = vec![precision];
        frame.extend(height.to_be_bytes());
        frame.extend(width.to_be_bytes());
        frame.push(components);
        for c in 0..components {
            frame.extend([c, 0x11, 0]);
        }
        segment(&mut out, marker::SOF3, &frame);
        let mut table = vec![0u8];
        table.extend(counts);
        table.extend(0..categories as u8);
        segment(&mut out, marker::DHT, &table);
        if restart != 0 {
            segment(&mut out, marker::DRI, &(restart * width).to_be_bytes());
        }
        let mut sos = vec![components];
        for c in 0..components {
            sos.extend([c, 0]);
        }
        sos.extend([predictor, 0, shift]);
        segment(&mut out, marker::SOS, &sos);
        out.extend(scan);
        out.extend([0xFF, marker::EOI]);
        out
    }

    /// Values that exercise every category, with runs of the largest
    /// differences and of none.
    fn picture(count: usize, precision: u8) -> Vec<u16> {
        let full = (1u32 << precision) - 1;
        let mut state = 0x2545_F491u32;
        (0..count)
            .map(|i| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                match i % 7 {
                    0 => 0,
                    1 => full as u16,
                    _ => (state % (full + 1)) as u16,
                }
            })
            .collect()
    }

    #[test]
    fn every_predictor_round_trips() {
        let (width, height, components) = (13u16, 7u16, 3u8);
        let samples = picture(13 * 7 * 3, 10);
        for predictor in 1..=7 {
            let coded = encode(&samples, width, height, components, 10, predictor, 0, 0);
            assert_eq!(
                decode(&coded, samples.len()).unwrap(),
                samples,
                "predictor {predictor}"
            );
        }
    }

    /// Sixteen bits, where the differences reach the category that has no
    /// bits of its own, and one component wide where there is no left
    /// neighbor but the previous sample.
    #[test]
    fn sixteen_bits_one_component_round_trips() {
        let samples = picture(40 * 9, 16);
        let coded = encode(&samples, 40, 9, 1, 16, 4, 0, 0);
        assert_eq!(decode(&coded, samples.len()).unwrap(), samples);
    }

    #[test]
    fn a_point_transform_drops_the_low_bits() {
        let samples = picture(8 * 8 * 2, 12);
        let coded = encode(&samples, 8, 8, 2, 12, 6, 2, 0);
        let expected: Vec<u16> = samples.iter().map(|&s| (s >> 2) << 2).collect();
        assert_eq!(decode(&coded, samples.len()).unwrap(), expected);
    }

    #[test]
    fn restart_intervals_of_whole_rows_are_followed() {
        let samples = picture(17 * 10 * 3, 10);
        let coded = encode(&samples, 17, 10, 3, 10, 7, 0, 3);
        assert_eq!(decode(&coded, samples.len()).unwrap(), samples);
    }

    /// A tile cut short is an error rather than a picture of what the
    /// zeros past the end decode as.
    #[test]
    fn a_cut_scan_is_refused() {
        let samples = picture(32 * 32 * 3, 10);
        let coded = encode(&samples, 32, 32, 3, 10, 7, 0, 0);
        let error = decode(&coded[..coded.len() / 2], samples.len()).unwrap_err();
        assert!(format!("{error}").contains("ends before"), "{error}");
        assert!(decode(&coded[..40], samples.len()).is_err());
    }

    #[test]
    fn what_is_not_lossless_is_refused_by_name() {
        let samples = picture(4 * 4 * 3, 8);
        let mut coded = encode(&samples, 4, 4, 3, 8, 1, 0, 0);
        // The frame's marker made a baseline one.
        coded[3] = 0xC0;
        let error = decode(&coded, samples.len()).unwrap_err();
        assert!(format!("{error}").contains("not lossless"), "{error}");
        // The wrong number of samples asked for.
        let coded = encode(&samples, 4, 4, 3, 8, 1, 0, 0);
        assert!(decode(&coded, samples.len() + 1).is_err());
        assert!(decode(b"", 1).is_err());
        assert!(decode(&[0xFF, 0xD8, 0xFF, 0xD9], 1).is_err());
    }
}
