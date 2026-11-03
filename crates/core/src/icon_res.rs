//! Turns an `.ico` file into a compiled Windows resource (`.res`) that holds it as the
//! program's icon, so the build can hand it straight to the linker without a resource compiler.
//! The app crate's build script uses this; it is plain byte layout, with no Windows calls.

use thiserror::Error;

/// One image of an `.ico` file, as its directory describes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IcoImage {
    /// Width in pixels; 0 means 256.
    pub width: u8,
    /// Height in pixels; 0 means 256.
    pub height: u8,
    pub colors: u8,
    pub planes: u16,
    pub bit_count: u16,
    /// The image itself: a DIB without its file header, or a PNG.
    pub data: Vec<u8>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum IcoError {
    #[error("the icon file is cut short")]
    TooShort,
    #[error("not an icon file")]
    NotIcon,
    #[error("the icon file holds no images")]
    NoImages,
    #[error("image {0} lies outside the icon file")]
    ImageOutOfBounds(usize),
}

/// Resource type ids (`RT_ICON`, `RT_GROUP_ICON`).
const RT_ICON: u16 = 3;
const RT_GROUP_ICON: u16 = 14;
/// Memory flags `rc.exe` gives icons (moveable, discardable) and groups (also pure).
const ICON_FLAGS: u16 = 0x1010;
const GROUP_FLAGS: u16 = 0x1030;
/// English (United States), what `rc.exe` uses by default.
const LANG_EN_US: u16 = 0x0409;

fn u16_le(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn u32_le(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

/// The images of the `.ico` file `bytes`, in file order.
pub fn parse_ico(bytes: &[u8]) -> Result<Vec<IcoImage>, IcoError> {
    if bytes.len() < 6 {
        return Err(IcoError::TooShort);
    }
    if u16_le(bytes, 0) != 0 || u16_le(bytes, 2) != 1 {
        return Err(IcoError::NotIcon);
    }
    let count = u16_le(bytes, 4) as usize;
    if count == 0 {
        return Err(IcoError::NoImages);
    }
    if bytes.len() < 6 + 16 * count {
        return Err(IcoError::TooShort);
    }
    (0..count)
        .map(|i| {
            let e = &bytes[6 + 16 * i..6 + 16 * (i + 1)];
            let (size, offset) = (u32_le(e, 8) as usize, u32_le(e, 12) as usize);
            let data = offset
                .checked_add(size)
                .and_then(|end| bytes.get(offset..end))
                .ok_or(IcoError::ImageOutOfBounds(i))?;
            Ok(IcoImage {
                width: e[0],
                height: e[1],
                colors: e[2],
                planes: u16_le(e, 4),
                bit_count: u16_le(e, 6),
                data: data.to_vec(),
            })
        })
        .collect()
}

/// Appends one `.res` entry with a numeric type and name, padded to a 4-byte boundary.
fn push_entry(out: &mut Vec<u8>, kind: u16, name: u16, flags: u16, lang: u16, data: &[u8]) {
    out.extend((data.len() as u32).to_le_bytes());
    out.extend(32u32.to_le_bytes()); // header size
    out.extend(
        [0xFFFF, kind, 0xFFFF, name]
            .iter()
            .flat_map(|w| w.to_le_bytes()),
    );
    out.extend(0u32.to_le_bytes()); // data version
    out.extend(flags.to_le_bytes());
    out.extend(lang.to_le_bytes());
    out.extend(0u32.to_le_bytes()); // version
    out.extend(0u32.to_le_bytes()); // characteristics
    out.extend(data);
    out.resize(out.len().next_multiple_of(4), 0);
}

/// A `.res` file holding the images of the `.ico` file `ico` as icons 1..=n and an icon group
/// named `group_id` that lists them. The first icon group in an exe is its icon.
pub fn icon_res(ico: &[u8], group_id: u16) -> Result<Vec<u8>, IcoError> {
    let images = parse_ico(ico)?;
    let mut out = Vec::new();
    // Every .res file starts with an empty entry that marks it as 32-bit.
    push_entry(&mut out, 0, 0, 0, 0, &[]);
    let mut group = Vec::with_capacity(6 + 14 * images.len());
    group.extend(
        [0u16, 1, images.len() as u16]
            .iter()
            .flat_map(|w| w.to_le_bytes()),
    );
    for (i, image) in images.iter().enumerate() {
        let id = i as u16 + 1;
        push_entry(&mut out, RT_ICON, id, ICON_FLAGS, LANG_EN_US, &image.data);
        group.extend([image.width, image.height, image.colors, 0]);
        group.extend(image.planes.to_le_bytes());
        group.extend(image.bit_count.to_le_bytes());
        group.extend((image.data.len() as u32).to_le_bytes());
        group.extend(id.to_le_bytes());
    }
    push_entry(
        &mut out,
        RT_GROUP_ICON,
        group_id,
        GROUP_FLAGS,
        LANG_EN_US,
        &group,
    );
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An `.ico` file with one image per `(width, height, bit_count, data)`.
    fn ico(images: &[(u8, u8, u16, &[u8])]) -> Vec<u8> {
        let mut out = vec![0, 0, 1, 0];
        out.extend((images.len() as u16).to_le_bytes());
        let mut offset = 6 + 16 * images.len() as u32;
        for (w, h, bits, data) in images {
            out.extend([*w, *h, 0, 0]);
            out.extend(1u16.to_le_bytes());
            out.extend(bits.to_le_bytes());
            out.extend((data.len() as u32).to_le_bytes());
            out.extend(offset.to_le_bytes());
            offset += data.len() as u32;
        }
        for (_, _, _, data) in images {
            out.extend(*data);
        }
        out
    }

    /// One entry of a `.res` file: type id, name id, memory flags, language, data.
    #[derive(Debug, PartialEq, Eq)]
    struct ResEntry {
        kind: u16,
        name: u16,
        flags: u16,
        lang: u16,
        data: Vec<u8>,
    }

    fn u16_at(b: &[u8], at: usize) -> u16 {
        u16::from_le_bytes([b[at], b[at + 1]])
    }

    fn u32_at(b: &[u8], at: usize) -> u32 {
        u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
    }

    /// Reads back a `.res` file whose types and names are all numeric, checking each entry
    /// starts on a 4-byte boundary.
    fn read_res(b: &[u8]) -> Vec<ResEntry> {
        let mut entries = Vec::new();
        let mut at = 0;
        while at < b.len() {
            assert_eq!(at % 4, 0, "entry at {at} is not DWORD-aligned");
            let data_size = u32_at(b, at) as usize;
            let header_size = u32_at(b, at + 4) as usize;
            assert_eq!(header_size, 32);
            assert_eq!(u16_at(b, at + 8), 0xFFFF, "numeric type");
            assert_eq!(u16_at(b, at + 12), 0xFFFF, "numeric name");
            entries.push(ResEntry {
                kind: u16_at(b, at + 10),
                name: u16_at(b, at + 14),
                flags: u16_at(b, at + 20),
                lang: u16_at(b, at + 22),
                data: b[at + header_size..at + header_size + data_size].to_vec(),
            });
            at += (header_size + data_size).next_multiple_of(4);
        }
        assert_eq!(at, b.len(), "trailing bytes");
        entries
    }

    #[test]
    fn parse_reads_each_image() {
        let file = ico(&[(16, 16, 32, b"sixteen"), (0, 0, 32, b"\x89PNG big")]);
        let images = parse_ico(&file).unwrap();
        assert_eq!(
            images,
            vec![
                IcoImage {
                    width: 16,
                    height: 16,
                    colors: 0,
                    planes: 1,
                    bit_count: 32,
                    data: b"sixteen".to_vec()
                },
                IcoImage {
                    width: 0,
                    height: 0,
                    colors: 0,
                    planes: 1,
                    bit_count: 32,
                    data: b"\x89PNG big".to_vec()
                },
            ]
        );
    }

    #[test]
    fn parse_rejects_a_cursor_and_a_bad_header() {
        let mut cursor = ico(&[(16, 16, 32, b"x")]);
        cursor[2] = 2;
        assert_eq!(parse_ico(&cursor), Err(IcoError::NotIcon));
        let mut reserved = ico(&[(16, 16, 32, b"x")]);
        reserved[0] = 1;
        assert_eq!(parse_ico(&reserved), Err(IcoError::NotIcon));
    }

    #[test]
    fn parse_rejects_short_files() {
        let file = ico(&[(16, 16, 32, b"x")]);
        assert_eq!(parse_ico(&file[..5]), Err(IcoError::TooShort));
        assert_eq!(parse_ico(&file[..20]), Err(IcoError::TooShort));
        assert_eq!(
            parse_ico(&file[..file.len() - 1]),
            Err(IcoError::ImageOutOfBounds(0))
        );
    }

    #[test]
    fn parse_rejects_an_image_pointing_outside_the_file() {
        let mut file = ico(&[(16, 16, 32, b"abcd")]);
        file[18..22].copy_from_slice(&u32::MAX.to_le_bytes()); // offset
        assert_eq!(parse_ico(&file), Err(IcoError::ImageOutOfBounds(0)));
    }

    #[test]
    fn parse_rejects_a_file_without_images() {
        assert_eq!(parse_ico(&ico(&[])), Err(IcoError::NoImages));
    }

    #[test]
    fn res_holds_an_empty_entry_then_each_icon_then_the_group() {
        let file = ico(&[(16, 16, 32, b"abcde"), (32, 32, 32, b"0123456789")]);
        let entries = read_res(&icon_res(&file, 7).unwrap());
        let kinds: Vec<_> = entries.iter().map(|e| (e.kind, e.name)).collect();
        assert_eq!(kinds, vec![(0, 0), (3, 1), (3, 2), (14, 7)]);
        assert!(entries[0].data.is_empty());
        assert_eq!(entries[1].data, b"abcde");
        assert_eq!(entries[2].data, b"0123456789");
        for e in &entries[1..] {
            assert_eq!(e.lang, 0x0409);
        }
        assert_eq!(entries[1].flags, 0x1010);
        assert_eq!(entries[3].flags, 0x1030);
    }

    #[test]
    fn group_lists_each_image_with_its_icon_id() {
        let file = ico(&[(16, 16, 32, b"abcde"), (0, 0, 32, b"\x89PNG!")]);
        let entries = read_res(&icon_res(&file, 1).unwrap());
        let g = &entries[3].data;
        assert_eq!(g.len(), 6 + 14 * 2);
        assert_eq!((u16_at(g, 0), u16_at(g, 2), u16_at(g, 4)), (0, 1, 2));
        let entry = |i: usize| {
            let e = &g[6 + 14 * i..6 + 14 * (i + 1)];
            (
                e[0],
                e[1],
                e[2],
                u16_at(e, 4),
                u16_at(e, 6),
                u32_at(e, 8),
                u16_at(e, 12),
            )
        };
        assert_eq!(entry(0), (16, 16, 0, 1, 32, 5, 1));
        assert_eq!(entry(1), (0, 0, 0, 1, 32, 5, 2));
    }

    #[test]
    fn res_of_a_bad_icon_file_is_an_error() {
        assert_eq!(icon_res(b"nope", 1), Err(IcoError::TooShort));
    }
}
