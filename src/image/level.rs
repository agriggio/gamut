//! A camera's electronic level, as the camera wrote it down: how far it was
//! rolled about the lens and how far the lens was pitched, at the moment
//! of the shot.
//!
//! Several makers keep it in their maker note, each in a layout of its own,
//! and a DJI drone keeps its gimbal's in the XMP packet. The layouts are
//! the ones ExifTool decodes — `Canon::LevelInfo`, `Panasonic::Main` tags
//! 0x90 and 0x91, `Pentax::LevelInfo` and `LevelInfoK3III`, and
//! `Olympus::CameraSettings` tags 0x903 and 0x904 — and so are the signs,
//! which [`Level`] holds as ExifTool states them: a roll clockwise as the
//! photographer sees the camera from behind, a pitch above level.
//!
//! Nikon writes the same three angles into its `ShotInfo`, which it
//! encrypts against the camera's serial number and shutter count; that is
//! not read.

use super::tiff::{self, Entry, Order, tag};
use super::xmp::Xmp;

/// What a camera's level says.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Level {
    /// Degrees the camera was turned about its lens, clockwise as seen from
    /// behind it.
    pub roll: f64,
    /// Degrees the lens pointed above level; below it, negative.
    pub pitch: f64,
    /// Degrees east of north the lens pointed, where the source says.
    pub yaw: Option<f64>,
    pub maker: Maker,
}

/// Whose level it is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Maker {
    Canon,
    Panasonic,
    Pentax,
    Olympus,
    /// A DJI drone's gimbal.
    Dji,
}

/// The maker note's tag in the Exif directory.
const MAKER_NOTE: u16 = 0x927c;
/// A DNG's private data, in its first directory: where a Pentax DNG keeps
/// its maker note.
const DNG_PRIVATE_DATA: u16 = 0xc634;
/// The camera's JPEG of the frame, in an RW2's first directory, whose own
/// EXIF is the one with the maker note in it.
const PANASONIC_JPEG: u16 = 0x002e;

/// The level in a TIFF block's maker note — `block` the EXIF as the reader
/// parsed it, little-endian where `little` — of a camera by `make`, named
/// `model`. `None` where the note is not one of the makers here, or holds
/// no level.
pub fn in_block(block: &[u8], little: bool, make: &str, model: &str) -> Option<Level> {
    let order = if little { Order::Little } else { Order::Big };
    let first = tiff::entries(block, order, &[])?;
    if make.starts_with("Panasonic")
        && let Some(jpeg) = find(&first, PANASONIC_JPEG).and_then(|entry| entry.bytes(block))
        && let Ok(inner) = exif::Reader::new().read_from_container(&mut std::io::Cursor::new(jpeg))
        && let Some(level) = in_block(inner.buf(), inner.little_endian(), "", model)
    {
        return Some(level);
    }
    // A Pentax DNG keeps its note, under a header of its own, in the
    // private data a DNG may carry.
    if let Some(private) = find(&first, DNG_PRIVATE_DATA).and_then(|entry| entry.bytes(block))
        && let Some(level) = pentax_note(private, model)
    {
        return Some(level);
    }
    let exif = find(&first, tag::EXIF_IFD)?;
    let at = order.read_u32(exif.bytes(block)?, 0)? as usize;
    let note = tiff::entries_at(block, order, at, &[])?
        .into_iter()
        .find(|entry| entry.tag == MAKER_NOTE)?;
    let start = note.offset;
    let bytes = note.bytes(block)?;
    if bytes.starts_with(b"Panasonic\0") {
        // Offsets count from the block, and the values wanted are in the
        // entries themselves.
        return panasonic(block, order, start + 12);
    }
    if bytes.starts_with(b"AOC\0") {
        // Offsets count from the block, as the rest of the EXIF's do; the
        // note says its own byte order, or two spaces for the block's.
        let order = order_at(bytes, 4).unwrap_or(order);
        return pentax(block, order, start + 6, model);
    }
    if bytes.starts_with(b"PENTAX \0") {
        return pentax_note(bytes, model);
    }
    if bytes.starts_with(b"OLYMPUS\0") {
        return olympus(bytes, order_at(bytes, 8)?, 12);
    }
    if bytes.starts_with(b"OM SYSTEM\0") {
        return olympus(bytes, order_at(bytes, 12)?, 16);
    }
    if make.starts_with("Canon") {
        return canon(block, order, start);
    }
    None
}

/// The level in a CR3's maker note, which the container keeps as a TIFF
/// of its own whose one directory is the note.
pub fn in_canon_tiff(tiff: &[u8]) -> Option<Level> {
    let order = Order::of(tiff)?;
    canon(tiff, order, order.read_u32(tiff, 4)? as usize)
}

/// A DJI drone's gimbal, from the XMP packet.
pub fn dji(xmp: &Xmp) -> Option<Level> {
    const DJI: &str = "http://www.dji.com/drone-dji/1.0/";
    let degrees = |name: &str| -> Option<f64> {
        let value = xmp.property(DJI, name)?.first()?;
        value
            .trim()
            .parse::<f64>()
            .ok()
            .filter(|value| value.is_finite())
    };
    Some(Level {
        roll: degrees("GimbalRollDegree").unwrap_or(0.0),
        pitch: degrees("GimbalPitchDegree")?,
        yaw: degrees("GimbalYawDegree"),
        maker: Maker::Dji,
    })
}

fn find(entries: &[Entry], wanted: u16) -> Option<&Entry> {
    entries.iter().find(|entry| entry.tag == wanted)
}

/// The byte order two bytes at `at` of `bytes` name.
fn order_at(bytes: &[u8], at: usize) -> Option<Order> {
    match bytes.get(at..at + 2)? {
        b"II" => Some(Order::Little),
        b"MM" => Some(Order::Big),
        _ => None,
    }
}

/// The signed 16- or 32-bit number at `at` of `bytes`.
fn i16_at(bytes: &[u8], order: Order, at: usize) -> Option<f64> {
    Some(f64::from(order.read_u16(bytes, at)? as i16))
}

fn i32_at(bytes: &[u8], order: Order, at: usize) -> Option<f64> {
    Some(f64::from(order.read_u32(bytes, at)? as i32))
}

/// Canon's `LevelInfo`, tag 0x4059 of the note whose directory is at `at`:
/// 32-bit numbers, the roll fifth and the pitch sixth, in tenths of a
/// degree taken round 3600 — the roll counted counterclockwise.
fn canon(block: &[u8], order: Order, at: usize) -> Option<Level> {
    let entries = tiff::entries_at(block, order, at, &[])?;
    let info = find(&entries, 0x4059)?.bytes(block)?;
    let angle = |at: usize| {
        let tenths = i32_at(info, order, at)?;
        Some(
            if tenths > 1800.0 {
                tenths - 3600.0
            } else {
                tenths
            } / 10.0,
        )
    };
    Some(Level {
        roll: -angle(16)?,
        pitch: angle(20)?,
        yaw: None,
        maker: Maker::Canon,
    })
}

/// Panasonic's tags 0x90 and 0x91, in the note's directory at `at`, each a
/// signed count of tenths of a degree — the pitch counted downward.
fn panasonic(block: &[u8], order: Order, at: usize) -> Option<Level> {
    let entries = tiff::entries_at(block, order, at, &[])?;
    let tenths = |wanted: u16| i16_at(find(&entries, wanted)?.bytes(block)?, order, 0);
    Some(Level {
        roll: tenths(0x90)? / 10.0,
        pitch: -tenths(0x91)? / 10.0,
        yaw: None,
        maker: Maker::Panasonic,
    })
}

/// A Pentax note under its `PENTAX \0` header, as a DNG's private data and
/// the newer cameras' JPEGs keep it: the byte order after the header, then
/// the directory, its offsets counting from the header's first byte. A
/// DNG's private data opens with the program that wrote it, which is
/// passed over to the header.
fn pentax_note(bytes: &[u8], model: &str) -> Option<Level> {
    let start = bytes.windows(8).position(|window| window == b"PENTAX \0")?;
    let note = &bytes[start..];
    pentax(note, order_at(note, 8)?, 10, model)
}

/// Pentax's `LevelInfo`, tag 0x022b of the directory at `at`: signed bytes,
/// the roll and the pitch each in half degrees counted the other way — or,
/// for the K-3 Mark III, 16-bit numbers at the fourth and sixth bytes. A
/// camera with no level says so in the first byte's low four bits.
fn pentax(block: &[u8], order: Order, at: usize, model: &str) -> Option<Level> {
    let entries = tiff::entries_at(block, order, at, &[])?;
    let info = find(&entries, 0x022b)?.bytes(block)?;
    let (roll, pitch) = if model.contains("K-3 Mark III") {
        (i16_at(info, order, 3)?, i16_at(info, order, 5)?)
    } else {
        if info.first()? & 0x0f == 0 {
            return None;
        }
        (
            f64::from(*info.get(1)? as i8),
            f64::from(*info.get(2)? as i8),
        )
    };
    Some(Level {
        roll: -roll / 2.0,
        pitch: -pitch / 2.0,
        yaw: None,
        maker: Maker::Pentax,
    })
}

/// Olympus's tags 0x903 and 0x904, in the camera settings directory that
/// tag 0x2020 of the note points at: each two signed 16-bit numbers, the
/// angle in tenths of a degree — the roll counted counterclockwise — and
/// whether the level was on. Every offset counts from the note's first
/// byte, and its directory starts `at` bytes in.
fn olympus(note: &[u8], order: Order, at: usize) -> Option<Level> {
    let entries = tiff::entries_at(note, order, at, &[])?;
    let settings = find(&entries, 0x2020)?;
    let settings = order.read_u32(settings.bytes(note)?, 0)? as usize;
    let entries = tiff::entries_at(note, order, settings, &[])?;
    let tenths = |wanted: u16| {
        let bytes = find(&entries, wanted)?.bytes(note)?;
        (i16_at(bytes, order, 2)? == 1.0).then_some(i16_at(bytes, order, 0)?)
    };
    Some(Level {
        roll: -tenths(0x903)? / 10.0,
        pitch: tenths(0x904)? / 10.0,
        yaw: None,
        maker: Maker::Olympus,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One directory entry, little-endian: its tag, its type, its count and
    /// its four bytes of value or offset.
    fn entry(tag: u16, kind: u16, count: u32, value: [u8; 4]) -> Vec<u8> {
        [
            &tag.to_le_bytes()[..],
            &kind.to_le_bytes(),
            &count.to_le_bytes(),
            &value,
        ]
        .concat()
    }

    /// A directory of `entries`, with no next one.
    fn directory(entries: &[Vec<u8>]) -> Vec<u8> {
        let mut bytes = (entries.len() as u16).to_le_bytes().to_vec();
        for entry in entries {
            bytes.extend_from_slice(entry);
        }
        bytes.extend_from_slice(&[0; 4]);
        bytes
    }

    /// Where [`block`] puts the maker note.
    const NOTE: u32 = 44;

    /// A little-endian EXIF block whose Exif directory holds `note` as its
    /// maker note, at [`NOTE`].
    fn block(note: &[u8]) -> Vec<u8> {
        let mut bytes = b"II\x2a\0\x08\0\0\0".to_vec();
        bytes.extend(directory(&[entry(
            tag::EXIF_IFD,
            4,
            1,
            26u32.to_le_bytes(),
        )]));
        bytes.extend(directory(&[entry(
            MAKER_NOTE,
            7,
            note.len() as u32,
            NOTE.to_le_bytes(),
        )]));
        assert_eq!(bytes.len(), NOTE as usize);
        bytes.extend_from_slice(note);
        bytes
    }

    fn read(note: &[u8], make: &str, model: &str) -> Option<(f64, f64)> {
        let level = in_block(&block(note), true, make, model)?;
        Some((level.roll, level.pitch))
    }

    fn i16s(values: &[i16]) -> Vec<u8> {
        values
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect()
    }

    /// The R6 Mark III's: a roll of 3565 tenths, which is 3.5 degrees
    /// counterclockwise of the full turn, and so clockwise, and a pitch of
    /// two tenths. Its values are after the directory, addressed from the
    /// block.
    #[test]
    fn canons_level_info() {
        let values: Vec<u8> = [28i32, 0, 0, 0, 3565, 2, 0]
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect();
        let mut note = directory(&[entry(0x4059, 9, 7, (NOTE + 18).to_le_bytes())]);
        note.extend(values);
        let (roll, pitch) = read(&note, "Canon", "Canon EOS R6 Mark III").unwrap();
        assert!((roll - 3.5).abs() < 1e-9 && (pitch - 0.2).abs() < 1e-9);
        assert_eq!(read(&note, "NIKON CORPORATION", ""), None);
    }

    /// The TZ95's: the roll as it is, the pitch the other way.
    #[test]
    fn panasonics_two_tags() {
        let mut note = b"Panasonic\0\0\0".to_vec();
        note.extend(directory(&[
            entry(0x90, 3, 1, [38, 0, 0, 0]),
            entry(0x91, 3, 1, [32, 0, 0, 0]),
        ]));
        assert_eq!(read(&note, "Panasonic", "DC-TZ95"), Some((3.8, -3.2)));
    }

    /// The KF's, in a PEF's note: half degrees, each the other way; and a
    /// camera that says it has no level has none.
    #[test]
    fn pentaxs_level_info() {
        let note = |first: u8| {
            let mut note = b"AOC\0II".to_vec();
            note.extend(directory(&[entry(0x022b, 7, 4, [first, -9i8 as u8, 4, 0])]));
            note
        };
        assert_eq!(read(&note(1), "PENTAX", "PENTAX KF"), Some((4.5, -2.0)));
        assert_eq!(
            read(&note(0), "RICOH IMAGING COMPANY, LTD.", "GR III"),
            None
        );
    }

    /// The K-3 Mark III's own layout, sixteen bits each, after the
    /// directory.
    #[test]
    fn the_k3_mark_iiis_level_info() {
        let mut note = b"AOC\0II".to_vec();
        note.extend(directory(&[entry(0x022b, 7, 8, (NOTE + 24).to_le_bytes())]));
        note.extend([0, 0, 0]);
        note.extend(i16s(&[1, -8]));
        note.push(0);
        let model = "PENTAX K-3 Mark III";
        assert_eq!(
            read(&note, "RICOH IMAGING COMPANY, LTD.", model),
            Some((-0.5, 4.0))
        );
    }

    /// A Pentax DNG keeps the note in its private data, behind the name of
    /// what wrote it and under a header that says its byte order, its
    /// offsets counting from the header.
    #[test]
    fn a_pentax_dngs_note_is_in_its_private_data() {
        let mut private = b"Adobe\0MakN\0\0\0\0MM".to_vec();
        private.extend(b"PENTAX \0MM");
        private.extend(1u16.to_be_bytes());
        private.extend(0x022bu16.to_be_bytes());
        private.extend(7u16.to_be_bytes());
        private.extend(4u32.to_be_bytes());
        private.extend([1, 2, 6, 0]);
        private.extend([0; 4]);
        let mut block = b"II\x2a\0\x08\0\0\0".to_vec();
        block.extend(directory(&[entry(
            DNG_PRIVATE_DATA,
            1,
            private.len() as u32,
            26u32.to_le_bytes(),
        )]));
        block.extend(&private);
        let level = in_block(&block, true, "PENTAX", "PENTAX K-3 II").unwrap();
        assert_eq!((level.roll, level.pitch), (-1.0, -3.0));
    }

    /// An Olympus note addresses everything from its own first byte: the
    /// camera settings directory, and in it the two angles, tenths of a
    /// degree, each beside whether the level was on.
    #[test]
    fn olympuss_camera_settings() {
        let note = |on: i16| {
            let mut note = b"OLYMPUS\0II\x03\0".to_vec();
            note.extend(directory(&[entry(0x2020, 13, 1, 30u32.to_le_bytes())]));
            assert_eq!(note.len(), 30);
            let roll: [u8; 4] = i16s(&[-21, on]).try_into().unwrap();
            let pitch: [u8; 4] = i16s(&[-94, on]).try_into().unwrap();
            note.extend(directory(&[
                entry(0x903, 8, 2, roll),
                entry(0x904, 8, 2, pitch),
            ]));
            note
        };
        assert_eq!(
            read(&note(1), "OLYMPUS CORPORATION", "E-M10 Mark III"),
            Some((2.1, -9.4))
        );
        assert_eq!(
            read(&note(0), "OLYMPUS CORPORATION", "E-M10 Mark III"),
            None
        );
    }

    /// A CR3's note is a TIFF of its own, its directory the note.
    #[test]
    fn a_cr3s_note_is_its_own_tiff() {
        let mut tiff = b"II\x2a\0\x08\0\0\0".to_vec();
        tiff.extend(directory(&[entry(0x4059, 9, 7, 26u32.to_le_bytes())]));
        for value in [28i32, 0, 0, 0, 31, 3367, 0] {
            tiff.extend(value.to_le_bytes());
        }
        let level = in_canon_tiff(&tiff).unwrap();
        assert!((level.roll + 3.1).abs() < 1e-9 && (level.pitch + 23.3).abs() < 1e-9);
    }

    /// The gimbal's three angles, as a DJI drone writes them, signs and all.
    #[test]
    fn a_djis_gimbal() {
        let packet = br#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF
            xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description
            xmlns:drone-dji="http://www.dji.com/drone-dji/1.0/"
            drone-dji:GimbalRollDegree="+0.00" drone-dji:GimbalYawDegree="+135.90"
            drone-dji:GimbalPitchDegree="-89.90"/></rdf:RDF></x:xmpmeta>"#;
        let level = dji(&Xmp::parse(packet).unwrap()).unwrap();
        assert_eq!(
            (level.roll, level.pitch, level.yaw, level.maker),
            (0.0, -89.9, Some(135.9), Maker::Dji)
        );
    }
}
