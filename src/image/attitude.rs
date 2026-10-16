//! Which way the camera was facing when it took the picture: how far its
//! lens was pointed above or below level, and how far its frame was turned
//! from level about the lens — read from the way gravity pulled on it.
//!
//! Three kinds of source say so. An iPhone or an iPad writes its
//! accelerometer's reading into Apple's maker note, tag 8, in g; a GoPro
//! writes the direction of gravity into the GPMF block in a JPEG's `APP6`
//! segment, under the key `GRAV`; and a camera with an electronic level, or
//! a drone's gimbal, writes the angles it measured — see [`super::level`].
//! The first two are said in their own axes:
//!
//! - Apple's are the device's — x toward its right edge, y toward its top
//!   and z out of its screen, seen from the front of the device held upright
//!   — and the sensor is the device's landscape, so a picture held level and
//!   upright reads as gravity along -x. The rear camera looks along -z and
//!   the front one along +z, which turns its picture's axes round too.
//! - GoPro's are the picture's own: x to its right, y down it and z out of
//!   the lens into the scene.
//!
//! [`Attitude`] holds gravity in that last form, the picture as stored — x
//! across it, y down it, z into the scene — which a level's angles are
//! turned into, and [`Attitude::shown`] carries
//! it through the orientation tag and the turn in force, as a region is
//! carried, to the picture as shown, where the angles are worked out.

use std::io::SeekFrom;

use ::image::metadata::Orientation;

use super::apple_note;
use super::decode::ReadSeek;
use super::level::Level;
use super::orient::{self, Turn};

/// Apple's tag for the accelerometer's reading, three signed rationals.
const ACCELERATION: u16 = 8;

/// How far from straight up or down the lens may point and the picture
/// still have a horizon worth drawing. Past it, a turn about the lens is a
/// turn about the vertical, which gravity cannot see.
pub const STEEPEST: f64 = 80.0;

/// The diagonal of a 35 mm frame, 36 by 24 mm, which a focal length said
/// as its 35 mm equivalent is measured against.
const FULL_FRAME_DIAGONAL: f64 = 43.266_615_305_567_87;

/// What a file says about which way its camera faced.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Attitude {
    /// The way gravity pulled, in the picture as stored: x across it to the
    /// right, y down it, z out of the lens into the scene. As long as the
    /// sensor read it.
    pub gravity: [f64; 3],
    /// What read it, and what the file holds as it wrote it.
    pub source: Source,
}

/// What read gravity, with the numbers the file holds in the source's own
/// terms.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Source {
    /// An iPhone's or an iPad's accelerometer, from the maker note, in g,
    /// in the device's axes.
    Apple([f64; 3]),
    /// A GoPro's gravity vector, from its GPMF block.
    GoPro([f64; 3]),
    /// A camera's electronic level, or a drone's gimbal.
    Level(Level),
}

/// The attitude in the picture as shown: what the panel says and the level
/// line is drawn from.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Shown {
    /// Degrees the lens pointed above level; below it, negative.
    pub pitch: f64,
    /// Degrees the level line runs clockwise of the picture's own across,
    /// as shown — a horizon tilted down to the right is a positive one.
    /// `None` where the lens pointed within `90 - STEEPEST` degrees of
    /// straight up or down.
    pub horizon: Option<f64>,
    /// Gravity as a unit vector in the picture as shown: x across it, y
    /// down it, z into the scene.
    pub gravity: [f64; 3],
}

impl Attitude {
    /// From the accelerometer's reading in Apple's maker note — the note as
    /// the EXIF holds it — for the rear camera, or the front one where
    /// `front`. `None` where the note is not Apple's or holds no reading.
    pub fn apple(note: &[u8], front: bool) -> Option<Self> {
        let reading: [f64; 3] = apple_note::rationals(note, ACCELERATION)?.try_into().ok()?;
        let [x, y, z] = reading;
        // The rear camera's picture runs down the device's -x and across
        // its -y, and it looks out of the back, -z. The front camera's
        // picture is not mirrored, so its across is the rear's, but it is
        // the other way up, looking out of the screen.
        let gravity = if front { [-y, x, z] } else { [-y, -x, -z] };
        Self::new(gravity, Source::Apple(reading))
    }

    /// From a GoPro's `GRAV`, which is already in the picture's axes.
    pub fn gopro(reading: [f64; 3]) -> Option<Self> {
        Self::new(reading, Source::GoPro(reading))
    }

    /// From a level's angles, for a picture the orientation tag stands up
    /// by `orientation`.
    ///
    /// Which way round the camera was held is the tag's to say as well as
    /// the level's, and the two must agree. A level that measures its roll
    /// all the way round reads near a quarter turn for a picture taken
    /// held on its side, and one that measures it from the way the camera
    /// was held reads near none: whichever of the roll and the roll after
    /// the tag's quarter turns is the nearer to that quarter turn is the
    /// one taken.
    pub fn level(level: Level, orientation: Orientation) -> Option<Self> {
        let held = match orientation {
            Orientation::Rotate90 => 90.0,
            Orientation::Rotate180 => 180.0,
            Orientation::Rotate270 => -90.0,
            _ => 0.0,
        };
        let apart = |roll: f64| (roll - held + 180.0).rem_euclid(360.0) - 180.0;
        let roll = if apart(level.roll).abs() <= apart(level.roll + held).abs() {
            level.roll
        } else {
            level.roll + held
        };
        // Rolled clockwise as seen from behind, the camera's right side goes
        // down, and gravity pulls toward the picture's right; pitched up,
        // gravity pulls back out of the lens.
        let (roll, pitch) = (roll.to_radians(), level.pitch.to_radians());
        let gravity = [
            roll.sin() * pitch.cos(),
            roll.cos() * pitch.cos(),
            -pitch.sin(),
        ];
        Self::new(gravity, Source::Level(level))
    }

    /// `None` where the reading has no direction to it.
    fn new(gravity: [f64; 3], source: Source) -> Option<Self> {
        let length = gravity
            .iter()
            .map(|value| value * value)
            .sum::<f64>()
            .sqrt();
        (length.is_finite() && length > 1e-6).then_some(Self { gravity, source })
    }

    /// The attitude in the picture as shown, `shown` pixels across and down,
    /// turned by `turn` from the upright picture, which `orientation` stood
    /// up from the stored one.
    ///
    /// An Apple reading is in the sensor's axes, and the sensor is wider
    /// than it is tall: a stored picture that is taller than it is wide was
    /// stood up a quarter turn after it was taken — saved again by an editor
    /// that turned the pixels and dropped the tag — and is read as having
    /// been turned whichever way leaves gravity pulling down it rather than
    /// up it, that being what the editor was doing.
    pub fn shown(&self, orientation: Orientation, turn: Turn, shown: [u32; 2]) -> Shown {
        let upright = turn.size(shown);
        let stored = if orient::quarter_turn(orientation) {
            [upright[1], upright[0]]
        } else {
            upright
        };
        let [x, y, z] = self.gravity;
        let mut across = [x, y];
        if matches!(self.source, Source::Apple(_)) && stored[1] > stored[0] {
            let clockwise = turned(Orientation::Rotate90, across);
            let counter = turned(Orientation::Rotate270, across);
            across = if clockwise[1] >= counter[1] {
                clockwise
            } else {
                counter
            };
        }
        let [x, y] = turned(turn.orientation(), turned(orientation, across));
        let length = (x * x + y * y + z * z).sqrt();
        let gravity = [x / length, y / length, z / length];
        let pitch = -gravity[2].clamp(-1.0, 1.0).asin().to_degrees();
        let horizon =
            (pitch.abs() <= STEEPEST).then(|| (-gravity[0]).atan2(gravity[1]).to_degrees());
        Shown {
            pitch,
            horizon,
            gravity,
        }
    }
}

impl Shown {
    /// Where the true horizon runs in a picture `size` pixels across and
    /// down, taken with a lens of `equivalent` mm on a 35 mm frame: the
    /// point of it nearest the picture's middle, as an offset from the
    /// middle in pixels. `None` where there is no horizon to draw, or no
    /// focal length to place it by.
    ///
    /// The horizon is where everything level and far away is seen: the
    /// directions square to gravity. Through a lens that keeps straight
    /// lines straight, with the middle of the picture on its axis, a pixel
    /// `(x, y)` from the middle looks along `(x, y, f)`, `f` the focal
    /// length in pixels, so the horizon is the line on which
    /// `gx·x + gy·y + gz·f` is nothing. It runs at [`Shown::horizon`]'s
    /// angle, as far from the middle as `f·gz` is against gravity's
    /// share across the picture: above the middle with the lens pointed
    /// down, below it pointed up.
    pub fn horizon_through(&self, size: [f64; 2], equivalent: Option<f64>) -> Option<[f64; 2]> {
        self.horizon?;
        let equivalent = equivalent.filter(|mm| mm.is_finite() && *mm > 0.0)?;
        let focal = equivalent / FULL_FRAME_DIAGONAL * size[0].hypot(size[1]);
        let [x, y, z] = self.gravity;
        let across = x * x + y * y;
        let along = -z * focal / across;
        Some([x * along, y * along])
    }
}

/// A direction across the picture carried through `orientation`: the part
/// of [`orient::upright`] that does not move the origin.
fn turned(orientation: Orientation, [x, y]: [f64; 2]) -> [f64; 2] {
    let [origin_x, origin_y] = orient::upright(orientation, [0.0, 0.0]);
    let [to_x, to_y] = orient::upright(orientation, [x, y]);
    [to_x - origin_x, to_y - origin_y]
}

/// The header a GoPro's `APP6` segment starts with, before its GPMF.
const GOPRO_HEADER: &[u8] = b"GoPro\0";

/// How deep GPMF's containers are followed: a GoPro nests them two deep.
const MAX_NESTING: usize = 8;

/// The gravity vector in the GPMF block of a GoPro's JPEG, if `source` is a
/// JPEG with one. The segments come before the scan, so the walk stops at
/// the scan's marker.
pub fn in_jpeg(source: &mut dyn ReadSeek) -> Option<Attitude> {
    source.seek(SeekFrom::Start(0)).ok()?;
    let mut start = [0u8; 2];
    source.read_exact(&mut start).ok()?;
    if start != [0xff, 0xd8] {
        return None;
    }
    loop {
        let mut marker = [0u8; 2];
        source.read_exact(&mut marker).ok()?;
        if marker[0] != 0xff {
            return None;
        }
        match marker[1] {
            0xff => {
                source.seek(SeekFrom::Current(-1)).ok()?;
                continue;
            }
            0xda | 0xd9 => return None,
            0x01 | 0xd0..=0xd7 => continue,
            _ => {}
        }
        let mut length = [0u8; 2];
        source.read_exact(&mut length).ok()?;
        // A length of two bytes, so at most 64 KiB is ever held.
        let length = usize::from(u16::from_be_bytes(length)).checked_sub(2)?;
        if marker[1] == 0xe6 {
            let mut payload = vec![0; length];
            source.read_exact(&mut payload).ok()?;
            if let Some(gpmf) = payload.strip_prefix(GOPRO_HEADER) {
                return gravity(gpmf, 0).and_then(Attitude::gopro);
            }
        } else {
            source.seek(SeekFrom::Current(length as i64)).ok()?;
        }
    }
}

/// The first `GRAV` in a run of GPMF's key-length-values: each a four-byte
/// key, a type, the size of one sample, a count of samples, and the samples
/// padded to four bytes; a type of zero is a run of them nested inside.
fn gravity(gpmf: &[u8], depth: usize) -> Option<[f64; 3]> {
    let mut at = 0;
    while let Some(header) = gpmf.get(at..at + 8) {
        let kind = header[4];
        let length =
            usize::from(header[5]) * usize::from(u16::from_be_bytes([header[6], header[7]]));
        let body = gpmf.get(at + 8..at + 8 + length)?;
        if kind == 0 && depth < MAX_NESTING {
            if let Some(found) = gravity(body, depth + 1) {
                return Some(found);
            }
        } else if &header[..4] == b"GRAV" && kind == b'f' && body.len() >= 12 {
            let value = |index: usize| {
                let bytes = body[4 * index..4 * index + 4].try_into().ok()?;
                Some(f64::from(f32::from_be_bytes(bytes)))
            };
            return Some([value(0)?, value(1)?, value(2)?]);
        }
        at += 8 + length.next_multiple_of(4);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const LEVEL: f64 = 1e-9;

    fn shown(attitude: &Attitude, orientation: Orientation, turn: Turn, size: [u32; 2]) -> Shown {
        attitude.shown(orientation, turn, size)
    }

    fn apple(reading: [f64; 3], front: bool) -> Attitude {
        let numbers: Vec<(i32, i32)> = reading
            .iter()
            .map(|value| ((value * 1e6).round() as i32, 1_000_000))
            .collect();
        let note = apple_note::tests::note(&[(ACCELERATION, 10, &numbers)]);
        Attitude::apple(&note, front).unwrap()
    }

    /// A phone held level in landscape, its top to the left, takes a picture
    /// that needs no turning, and gravity pulls straight down it; held
    /// upright, the picture is stood up by the tag and gravity still does.
    #[test]
    fn a_level_phone_has_a_level_horizon_however_it_was_held() {
        let landscape = apple([-1.0, 0.0, 0.0], false);
        let level = shown(
            &landscape,
            Orientation::NoTransforms,
            Turn::NONE,
            [400, 300],
        );
        assert!(level.pitch.abs() < LEVEL && level.horizon.unwrap().abs() < LEVEL);
        let portrait = apple([0.0, -1.0, 0.0], false);
        let level = shown(&portrait, Orientation::Rotate90, Turn::NONE, [300, 400]);
        assert!(level.horizon.unwrap().abs() < LEVEL, "{level:?}");
        // Upside down, and stood the right way up by the tag.
        let upside_down = apple([1.0, 0.0, 0.0], false);
        let level = shown(&upside_down, Orientation::Rotate180, Turn::NONE, [400, 300]);
        assert!(level.horizon.unwrap().abs() < LEVEL, "{level:?}");
    }

    /// The front camera's picture of the phone held in landscape the other
    /// way round, which its tag leaves alone, is level too: the front
    /// camera's axes are not the rear one's.
    #[test]
    fn the_front_camera_reads_the_other_way_up() {
        let level = shown(
            &apple([1.0, 0.0, 0.0], true),
            Orientation::NoTransforms,
            Turn::NONE,
            [400, 300],
        );
        assert!(level.horizon.unwrap().abs() < LEVEL, "{level:?}");
        // Lying on its back, the front camera looks up.
        let up = shown(
            &apple([0.0, 0.0, -1.0], true),
            Orientation::NoTransforms,
            Turn::NONE,
            [400, 300],
        );
        assert!(
            (up.pitch - 90.0).abs() < LEVEL && up.horizon.is_none(),
            "{up:?}"
        );
    }

    /// Pointed down, the rear camera's gravity comes out of the screen —
    /// toward the scene — and the pitch is negative.
    #[test]
    fn pointing_down_is_a_negative_pitch() {
        let half = std::f64::consts::FRAC_1_SQRT_2;
        let down = shown(
            &apple([-half, 0.0, -half], false),
            Orientation::NoTransforms,
            Turn::NONE,
            [400, 300],
        );
        assert!((down.pitch + 45.0).abs() < 1e-3, "{down:?}");
    }

    /// A picture an editor stood up and saved without its tag still reads
    /// as level.
    #[test]
    fn a_picture_stood_up_by_an_editor_is_turned_back() {
        let portrait = apple([0.0, -1.0, 0.0], false);
        let level = shown(&portrait, Orientation::NoTransforms, Turn::NONE, [300, 400]);
        assert!(level.horizon.unwrap().abs() < LEVEL, "{level:?}");
    }

    /// The GoPro picture whose horizon runs down to the right by about two
    /// degrees, as its `GRAV` says; and the same turned a quarter on screen.
    #[test]
    fn a_horizon_down_to_the_right_is_clockwise() {
        let gopro = Attitude::gopro([-0.0377, 0.9992, -0.0092]).unwrap();
        let tilted = shown(&gopro, Orientation::NoTransforms, Turn::NONE, [4000, 3000]);
        assert!((tilted.horizon.unwrap() - 2.16).abs() < 0.01, "{tilted:?}");
        let turned = shown(
            &gopro,
            Orientation::NoTransforms,
            Turn::NONE.clockwise(),
            [3000, 4000],
        );
        assert!((turned.horizon.unwrap() - 92.16).abs() < 0.01, "{turned:?}");
    }

    /// A level measured all the way round and one measured from the way
    /// the camera was held agree, for a picture taken held on its side:
    /// the S1R II's portrait, -89.7 degrees, and the same 0.3 degrees off
    /// the quarter turn.
    #[test]
    fn a_levels_roll_agrees_with_the_tag_either_way() {
        use super::super::level::Maker;
        let level = |roll: f64| Level {
            roll,
            pitch: 20.7,
            yaw: None,
            maker: Maker::Panasonic,
        };
        for roll in [-89.7, 0.3] {
            let attitude = Attitude::level(level(roll), Orientation::Rotate270).unwrap();
            let shown = attitude.shown(Orientation::Rotate270, Turn::NONE, [3000, 4000]);
            assert!(
                (shown.horizon.unwrap() + 0.3).abs() < 1e-9,
                "{roll}: {shown:?}"
            );
            assert!((shown.pitch - 20.7).abs() < 1e-9);
        }
        // Rolled clockwise as seen from behind, the horizon runs
        // counterclockwise in the picture.
        let shown = Attitude::level(level(3.8), Orientation::NoTransforms)
            .unwrap()
            .shown(Orientation::NoTransforms, Turn::NONE, [4000, 3000]);
        assert!((shown.horizon.unwrap() + 3.8).abs() < 1e-9);
    }

    /// Pointed down, the horizon is above the middle, as far as the focal
    /// length makes the pitch; pointed up, below it; level, through it.
    /// Without a focal length it has no place.
    #[test]
    fn the_horizon_is_placed_by_the_pitch_and_the_focal_length() {
        use super::super::level::Maker;
        let at = |pitch: f64, roll: f64| Level {
            roll,
            pitch,
            yaw: None,
            maker: Maker::Panasonic,
        };
        let size = [4000.0, 3000.0];
        // 43.27 mm on a 35 mm frame is a focal length as long as the
        // frame's diagonal, 5000 pixels here.
        let normal = Some(FULL_FRAME_DIAGONAL);
        for (pitch, sign) in [(-20.0, -1.0), (15.0, 1.0)] {
            let shown = Attitude::level(at(pitch, 0.0), Orientation::NoTransforms)
                .unwrap()
                .shown(Orientation::NoTransforms, Turn::NONE, [4000, 3000]);
            let [x, y] = shown.horizon_through(size, normal).unwrap();
            let expected = 5000.0 * f64::tan(pitch.abs().to_radians());
            assert!(x.abs() < 1e-6, "{pitch}: {x}");
            assert!((y - sign * expected).abs() < 1e-6, "{pitch}: {y}");
            assert_eq!(shown.horizon_through(size, None), None);
        }
        let level = Attitude::level(at(0.0, 10.0), Orientation::NoTransforms)
            .unwrap()
            .shown(Orientation::NoTransforms, Turn::NONE, [4000, 3000]);
        let [x, y] = level.horizon_through(size, normal).unwrap();
        assert!(x.hypot(y) < 1e-6);
        // Rolled, the offset is square to the line, still on the up side.
        let rolled = Attitude::level(at(-20.0, 10.0), Orientation::NoTransforms)
            .unwrap()
            .shown(Orientation::NoTransforms, Turn::NONE, [4000, 3000]);
        let [x, y] = rolled.horizon_through(size, normal).unwrap();
        let (sin, cos) = rolled.horizon.unwrap().to_radians().sin_cos();
        assert!((x * cos + y * sin).abs() < 1e-6);
        assert!(y < 0.0);
        // Too steep for a horizon, nothing is placed.
        let steep = Attitude::level(at(-85.0, 0.0), Orientation::NoTransforms)
            .unwrap()
            .shown(Orientation::NoTransforms, Turn::NONE, [4000, 3000]);
        assert_eq!(steep.horizon_through(size, normal), None);
    }

    #[test]
    fn no_direction_is_no_attitude() {
        assert_eq!(Attitude::gopro([0.0; 3]), None);
        assert_eq!(Attitude::gopro([f64::NAN, 0.0, 1.0]), None);
    }

    /// A key-length-value of floats.
    fn floats(key: &[u8; 4], values: &[f32]) -> Vec<u8> {
        let mut klv = key.to_vec();
        klv.extend_from_slice(&[b'f', 4 * values.len() as u8, 0, 1]);
        for value in values {
            klv.extend_from_slice(&value.to_be_bytes());
        }
        klv
    }

    /// A container of `inside`.
    fn nest(key: &[u8; 4], inside: &[u8]) -> Vec<u8> {
        let mut klv = key.to_vec();
        klv.extend_from_slice(&[0, 4]);
        klv.extend_from_slice(&((inside.len() / 4) as u16).to_be_bytes());
        klv.extend_from_slice(inside);
        klv
    }

    /// GRAV found two containers deep, past a value padded to four bytes,
    /// in a JPEG's APP6 after an APP1.
    #[test]
    fn grav_is_found_in_the_app6_segment() {
        let mut odd = b"MINF".to_vec();
        odd.extend_from_slice(&[b'c', 1, 0, 5, b'H', b'E', b'R', b'O', b'1', 0, 0, 0]);
        let inside = [odd, floats(b"GRAV", &[0.0, 1.0, 0.5])].concat();
        let gpmf = nest(b"DEVC", &nest(b"STRM", &inside));
        let segment = |marker: u8, payload: &[u8]| {
            let mut segment = vec![0xff, marker];
            segment.extend_from_slice(&((payload.len() + 2) as u16).to_be_bytes());
            segment.extend_from_slice(payload);
            segment
        };
        let jpeg = [
            &[0xff, 0xd8][..],
            &segment(0xe1, b"Exif\0\0"),
            &segment(0xe6, &[GOPRO_HEADER, &gpmf].concat()),
            &[0xff, 0xda],
        ]
        .concat();
        let attitude = in_jpeg(&mut std::io::Cursor::new(jpeg)).unwrap();
        assert_eq!(attitude.gravity, [0.0, 1.0, 0.5]);
        assert_eq!(attitude.source, Source::GoPro([0.0, 1.0, 0.5]));
        // No GoPro segment, and not a JPEG at all.
        assert_eq!(
            in_jpeg(&mut std::io::Cursor::new(vec![0xff, 0xd8, 0xff, 0xda])),
            None
        );
        assert_eq!(
            in_jpeg(&mut std::io::Cursor::new(b"\x89PNG".to_vec())),
            None
        );
    }

    /// A length running past the block ends the walk rather than reading
    /// past it.
    #[test]
    fn a_truncated_block_gives_nothing() {
        let mut gpmf = floats(b"GRAV", &[0.0, 1.0, 0.0]);
        gpmf.truncate(14);
        assert_eq!(gravity(&gpmf, 0), None);
    }
}
