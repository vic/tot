//! Kitty graphics protocol encoder (client side). Spec: sw.kovidgoyal.net/kitty/graphics-protocol/
use crate::grid::dec;
use crate::{Error, Result};

/// z-index of a layer drawn under text but above non-default cell backgrounds.
/// Spec: "Negative z-index values mean that the images will be drawn under the text";
/// "below INT32_MIN/2 (-1,073,741,824) will be drawn under cells with non-default background colors".
/// z=0 draws over text, so 0 is not in range.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ZIndex(i32);

impl ZIndex {
    pub const MIN: i32 = i32::MIN / 2;
    pub const MAX: i32 = -1;
    pub fn new(z: i32) -> Result<Self> {
        if (Self::MIN..=Self::MAX).contains(&z) { Ok(ZIndex(z)) } else { Err(Error::ZRange(z)) }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Medium {
    Shm,
    File,
    Direct,
}

impl Medium {
    pub const ALL: [Medium; 3] = [Medium::Shm, Medium::File, Medium::Direct];
    fn key(self) -> u8 {
        match self {
            Medium::Shm => b's',
            Medium::File => b't',
            Medium::Direct => b'd',
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Medium::Shm => "shm",
            Medium::File => "file",
            Medium::Direct => "direct",
        }
    }
}

/// Transmit+display (`a=T`) or transmit only (`a=t`, N3: never placed).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    TransmitPlace,
    TransmitOnly,
    Query,
}

pub struct Image<'a> {
    pub id: u32,
    pub w: u32,
    pub h: u32,
    pub rgba: &'a [u8],
}

pub struct Placement {
    pub pid: u32,
    pub cols: u16,
    pub rows: u16,
    pub z: Option<ZIndex>,
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn base64(out: &mut Vec<u8>, data: &[u8]) {
    for c in data.chunks(3) {
        let n = (u32::from(c[0]) << 16)
            | (u32::from(c.get(1).copied().unwrap_or(0)) << 8)
            | u32::from(c.get(2).copied().unwrap_or(0));
        let idx = |s: u32| B64[((n >> s) & 63) as usize];
        out.extend_from_slice(&[idx(18), idx(12)]);
        out.push(if c.len() > 1 { idx(6) } else { b'=' });
        out.push(if c.len() > 2 { idx(0) } else { b'=' });
    }
}

fn kv(out: &mut Vec<u8>, k: u8, v: u64) {
    out.extend_from_slice(&[k, b'=']);
    dec(out, v);
    out.push(b',');
}

/// Unique-per-frame shm / temp-file name: the terminal unlinks it after reading.
pub fn medium_path(m: Medium, tag: u64) -> Option<String> {
    let pid = std::process::id();
    match m {
        Medium::Shm => Some(format!("/dev/shm/tot-proto1-{pid}-{tag}")),
        // spec: temp file path must contain "tty-graphics-protocol"
        Medium::File => Some(format!("/tmp/tty-graphics-protocol-tot-proto1-{pid}-{tag}")),
        Medium::Direct => None,
    }
}

/// Append one image command. Shm/File write the pixels to disk first (not part of the pty write).
pub fn image(out: &mut Vec<u8>, img: &Image, m: Medium, act: Action, place: Option<&Placement>, tag: u64) -> Result<()> {
    let head = |out: &mut Vec<u8>| {
        out.extend_from_slice(b"\x1b_G");
        out.extend_from_slice(match act {
            Action::TransmitPlace => b"a=T,",
            Action::TransmitOnly => b"a=t,",
            Action::Query => b"a=q,",
        });
        kv(out, b'i', img.id.into());
        kv(out, b's', img.w.into());
        kv(out, b'v', img.h.into());
        out.extend_from_slice(b"f=32,t=");
        out.push(m.key());
        out.push(b',');
        if let Some(p) = place {
            kv(out, b'p', p.pid.into());
            kv(out, b'c', p.cols.into());
            kv(out, b'r', p.rows.into());
            out.extend_from_slice(b"C=1,");
            if let Some(ZIndex(z)) = p.z {
                out.extend_from_slice(b"z=-");
                dec(out, u64::from(z.unsigned_abs()));
                out.push(b',');
            }
        }
    };
    match medium_path(m, tag) {
        Some(path) => {
            std::fs::write(&path, img.rgba)?;
            head(out);
            // Ghostty 1.3.1 file read fails (EINVAL) when S= equals the file size
            // (graphics_image.zig readFile appendRemaining .limited(size)); omit S for t=t.
            if m == Medium::Shm {
                kv(out, b'S', img.rgba.len() as u64);
            }
            out.pop();
            out.push(b';');
            let name = path.strip_prefix("/dev/shm").unwrap_or(&path);
            base64(out, name.as_bytes());
            out.extend_from_slice(b"\x1b\\");
        }
        None => {
            // direct: base64 in 4096-byte chunks, m=1 on all but the last (spec)
            let mut enc = Vec::with_capacity(img.rgba.len() * 4 / 3 + 4);
            base64(&mut enc, img.rgba);
            let n = enc.chunks(4096).len();
            for (k, chunk) in enc.chunks(4096).enumerate() {
                if k == 0 {
                    head(out);
                } else {
                    out.extend_from_slice(b"\x1b_G");
                }
                out.extend_from_slice(if k + 1 < n { b"m=1;" } else { b"m=0;" });
                out.extend_from_slice(chunk);
                out.extend_from_slice(b"\x1b\\");
            }
        }
    }
    Ok(())
}

/// Place an already uploaded image at the cursor.
pub fn place(out: &mut Vec<u8>, id: u32, p: &Placement) {
    out.extend_from_slice(b"\x1b_Ga=p,");
    kv(out, b'i', id.into());
    kv(out, b'p', p.pid.into());
    kv(out, b'c', p.cols.into());
    kv(out, b'r', p.rows.into());
    out.extend_from_slice(b"C=1\x1b\\");
}

/// Delete one placement, keep image data (`d=i` lower case).
pub fn unplace(out: &mut Vec<u8>, id: u32, pid: u32) {
    out.extend_from_slice(b"\x1b_Ga=d,d=i,");
    kv(out, b'i', id.into());
    kv(out, b'p', pid.into());
    out.pop();
    out.extend_from_slice(b"\x1b\\");
}

/// Delete all images and their data.
pub const DELETE_ALL: &[u8] = b"\x1b_Ga=d,d=A\x1b\\";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zindex_range_is_typed() {
        assert!(ZIndex::new(-1).is_ok());
        assert!(ZIndex::new(-1_073_741_824).is_ok());
        assert!(ZIndex::new(0).is_err(), "0 draws over text");
        assert!(ZIndex::new(5).is_err());
        assert!(ZIndex::new(-1_073_741_825).is_err(), "under non-default bg");
        assert!(ZIndex::new(i32::MIN).is_err());
        assert_eq!(ZIndex::MIN, -1_073_741_824);
    }

    #[test]
    fn base64_vectors() {
        for (i, o) in [("", ""), ("f", "Zg=="), ("fo", "Zm8="), ("foo", "Zm9v"), ("foobar", "Zm9vYmFy")] {
            let mut v = Vec::new();
            base64(&mut v, i.as_bytes());
            assert_eq!(v, o.as_bytes());
        }
    }

    #[test]
    fn direct_chunks_and_keys() {
        let px = vec![7u8; 4 * 2000];
        let img = Image { id: 9, w: 40, h: 50, rgba: &px };
        let p = Placement { pid: 1, cols: 20, rows: 6, z: ZIndex::new(-1).ok() };
        let mut out = Vec::new();
        image(&mut out, &img, Medium::Direct, Action::TransmitPlace, Some(&p), 0).unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(s.starts_with("\x1b_Ga=T,i=9,s=40,v=50,f=32,t=d,p=1,c=20,r=6,C=1,z=-1,m=1;"));
        assert_eq!(s.matches("\x1b\\").count(), 3); // 10668 b64 bytes = 3 chunks
        assert!(s.contains("\x1b_Gm=0;"));
        assert!(!s.contains("m=1;") || s.find("m=0;") > s.find("m=1;"));
    }

    #[test]
    fn transmit_only_has_no_placement() {
        let px = [0u8; 4];
        let img = Image { id: 3, w: 1, h: 1, rgba: &px };
        let mut out = Vec::new();
        image(&mut out, &img, Medium::Direct, Action::TransmitOnly, None, 0).unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(s.starts_with("\x1b_Ga=t,i=3,") && !s.contains("p="));
    }
}
