//! Apple's maker note: the directory an iPhone or an iPad keeps inside its
//! EXIF, under the `MakerNote` tag. `Apple iOS\0`, two bytes of version, the
//! byte order, then a TIFF directory whose offsets count from the note's
//! first byte. What is read out of it here is its signed rationals by tag:
//! the two numbers a gain map's headroom is worked out from (see
//! `decode/heif/apple.rs`), and the accelerometer's reading (see
//! [`super::attitude`]).

use super::tiff::{self, Order};

/// The note's signed rationals under `wanted`, as many as the entry holds.
/// `None` where `note` is not Apple's, the tag is not in it, or it is not
/// signed rationals.
pub fn rationals(note: &[u8], wanted: u16) -> Option<Vec<f64>> {
    if !note.starts_with(b"Apple iOS\0") {
        return None;
    }
    let order = match note.get(12..14)? {
        b"MM" => Order::Big,
        b"II" => Order::Little,
        _ => return None,
    };
    const DIRECTORY: usize = 14;
    const SRATIONAL: u16 = 10;
    let entry = tiff::entries_at(note, order, DIRECTORY, &[])?
        .into_iter()
        .find(|entry| entry.tag == wanted)?;
    if entry.kind != SRATIONAL || entry.count == 0 {
        return None;
    }
    // Eight bytes each, so never inline, and never more than the note holds:
    // the count is checked against the note before anything is allocated.
    let count = entry.count as usize;
    note.get(entry.offset..entry.offset.checked_add(count.checked_mul(8)?)?)?;
    (0..count)
        .map(|index| {
            let at = entry.offset + 8 * index;
            let numerator = order.read_u32(note, at)? as i32;
            let denominator = order.read_u32(note, at + 4)? as i32;
            (denominator != 0).then(|| f64::from(numerator) / f64::from(denominator))
        })
        .collect()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// One entry of a note written by hand: its tag, its type, and the
    /// rationals under it, each a numerator and a denominator.
    pub(crate) type Written<'a> = (u16, u16, &'a [(i32, i32)]);

    /// A note written by hand in Apple's layout, big-endian, holding
    /// `entries`, with their values after the directory.
    pub(crate) fn note(entries: &[Written]) -> Vec<u8> {
        let mut note = b"Apple iOS\0\0\x01MM".to_vec();
        note.extend_from_slice(&(entries.len() as u16).to_be_bytes());
        let mut values = Vec::new();
        let start = 16 + 12 * entries.len();
        for (tag, kind, numbers) in entries {
            note.extend_from_slice(&tag.to_be_bytes());
            note.extend_from_slice(&kind.to_be_bytes());
            note.extend_from_slice(&(numbers.len() as u32).to_be_bytes());
            note.extend_from_slice(&((start + values.len()) as u32).to_be_bytes());
            for (numerator, denominator) in *numbers {
                values.extend_from_slice(&numerator.to_be_bytes());
                values.extend_from_slice(&denominator.to_be_bytes());
            }
        }
        note.extend_from_slice(&values);
        note
    }

    #[test]
    fn every_rational_of_an_entry_is_read() {
        let note = note(&[
            (8, 10, &[(-1, 2), (3, 4), (5, -8)]),
            (33, 10, &[(101, 100)]),
        ]);
        assert_eq!(rationals(&note, 8), Some(vec![-0.5, 0.75, -0.625]));
        assert_eq!(rationals(&note, 33), Some(vec![1.01]));
        assert_eq!(rationals(&note, 9), None);
        assert_eq!(rationals(b"Nikon\0", 8), None);
    }

    /// A count running past the end of the note gives nothing rather than
    /// asking for room for it.
    #[test]
    fn a_count_past_the_end_is_refused() {
        let mut note = note(&[(8, 10, &[(1, 1)])]);
        note[20..24].copy_from_slice(&u32::MAX.to_be_bytes());
        assert_eq!(rationals(&note, 8), None);
    }

    #[test]
    fn another_type_or_a_zero_denominator_is_refused() {
        assert_eq!(rationals(&note(&[(8, 5, &[(1, 2)])]), 8), None);
        assert_eq!(rationals(&note(&[(8, 10, &[(1, 0)])]), 8), None);
    }
}
