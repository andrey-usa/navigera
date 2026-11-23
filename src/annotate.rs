//! `screenshot --annotate`: each ref of the page outlined and labelled
//! with its number in the image, so a model that reads screenshots can
//! say `click 12` about what it sees. The marks live in a closed shadow
//! root (the page's styles can't reach them), are `aria-hidden` with no
//! pointer events, and are removed right after the capture.

use serde_json::json;

/// The refs an `ax` text tree shows (`[ref=12]`), in order, each once.
pub fn refs_in(tree: &str) -> Vec<u64> {
    let mut out = Vec::new();
    for part in tree.split("[ref=").skip(1) {
        let digits: String = part.chars().take_while(char::is_ascii_digit).collect();
        if let Ok(n) = digits.parse::<u64>() {
            if part[digits.len()..].starts_with(']') && !out.contains(&n) {
                out.push(n);
            }
        }
    }
    out
}

/// The script that draws the marks: `(ref, x, y, width, height)` in
/// viewport CSS pixels. It answers once a frame with the marks was painted,
/// as `[painted, ms, dpr, scrollX, scrollY]`: `painted` is false when no
/// frame came in time.
pub fn draw(boxes: &[(u64, f64, f64, f64, f64)]) -> String {
    let marks = json!(boxes.iter().map(|&(id, x, y, w, h)| json!([id, x, y, w, h])).collect::<Vec<_>>());
    format!(
        r#"(() => {{
  document.getElementById('__nv_marks')?.remove();
  const host = document.createElement('div');
  host.id = '__nv_marks';
  host.setAttribute('aria-hidden', 'true');
  host.style.cssText = 'all:initial;position:absolute;left:0;top:0;width:0;height:0;z-index:2147483647;pointer-events:none';
  const root = host.attachShadow({{ mode: 'closed' }});
  const sx = scrollX, sy = scrollY;
  for (const [id, x, y, w, h] of {marks}) {{
    const b = document.createElement('div');
    b.style.cssText = `position:absolute;left:${{x + sx}}px;top:${{y + sy}}px;width:${{w}}px;height:${{h}}px;box-sizing:border-box;border:2px solid #e6007e`;
    const l = document.createElement('div');
    l.textContent = id;
    l.style.cssText = `position:absolute;left:${{x + sx}}px;top:${{(y >= 14 ? y - 14 : y) + sy}}px;font:bold 11px/13px monospace;color:#fff;background:#e6007e;padding:0 3px;border-radius:2px;white-space:nowrap`;
    root.append(b, l);
  }}
  document.documentElement.append(host);
  // The capture is of what was painted: wait for a frame that has the marks
  // (two animation frames). A hidden tab paints nothing and gets 250 ms; a
  // visible one whose frames are late gets 2 s, since an image without its
  // labels is the wrong answer. Answers [the frames came, ms waited, the
  // device pixels per CSS pixel, scrollX, scrollY].
  const t0 = performance.now(), cap = document.visibilityState === 'visible' ? 2000 : 250;
  return new Promise((done) => {{
    setTimeout(() => done([false, Math.round(performance.now() - t0), devicePixelRatio, sx, sy]), cap);
    requestAnimationFrame(() => requestAnimationFrame(() => done([true, Math.round(performance.now() - t0), devicePixelRatio, sx, sy])));
  }});
}})()"#
    )
}

/// The outline's colour (`#e6007e`), as a capture shows it: near enough
/// for a display's colour profile, far from any text or background a
/// page is likely to have at that one pixel.
fn is_mark(rgb: [u8; 3]) -> bool {
    rgb[0] >= 190 && rgb[1] <= 70 && (70..=190).contains(&rgb[2])
}

/// A point that must show the outline's colour in an image that has the
/// marks: on the bottom edge of a box, where no label lies over it (a
/// box's own label sits at its top; another's could lie anywhere), in CSS
/// pixels of the viewport. The topmost such point, since the image is
/// read only down to it. None when no box has one inside `width` x
/// `height`.
pub fn proof_point(boxes: &[(u64, f64, f64, f64, f64)], width: f64, height: f64) -> Option<(f64, f64)> {
    // A label: 13 px high, at most a few digits wide, above its box or,
    // at the top of the page, inside it.
    let label = |&(_, x, y, _, _): &(u64, f64, f64, f64, f64)| (x, if y >= 14.0 { y - 14.0 } else { y });
    let mut best: Option<(f64, f64)> = None;
    for (i, &(_, x, y, w, h)) in boxes.iter().enumerate() {
        let (px, py) = (x + 1.0, y + h - 1.0);
        let inside = w >= 4.0 && h >= 16.0 && px >= 0.0 && py >= 0.0 && px < width - 1.0 && py < height - 1.0;
        let covered = || {
            boxes.iter().enumerate().any(|(j, other)| {
                let (lx, ly) = label(other);
                j != i && px >= lx - 1.0 && px <= lx + 80.0 && py >= ly - 1.0 && py <= ly + 14.0
            })
        };
        if inside && best.is_none_or(|(_, by)| py < by) && !covered() {
            best = Some((px, py));
        }
    }
    best
}

/// Whether the PNG `image` shows the outline's colour at device pixel
/// (`x`, `y`). None when that cannot be read (not an 8-bit RGB or RGBA
/// PNG, interlaced, or the point outside it): the caller then has no
/// proof either way. Only the rows down to `y` are inflated.
pub fn shows_mark(image: &[u8], x: u32, y: u32) -> Option<bool> {
    pixel(image, x, y).map(is_mark)
}

/// The RGB at device pixel (`x`, `y`) of the PNG `image`, as
/// [`shows_mark`] reads it (for a failure's report).
pub fn pixel_at(image: &[u8], x: u32, y: u32) -> Option<[u8; 3]> {
    pixel(image, x, y)
}

fn pixel(png: &[u8], x: u32, y: u32) -> Option<[u8; 3]> {
    let mut found = None;
    let (width, _) = rows(png, Some(y), |r, row, bpp| {
        if r == y {
            let at = x as usize * bpp;
            found = row.get(at..at + 3).map(|p| [p[0], p[1], p[2]]);
        }
    })?;
    if x >= width {
        return None;
    }
    found
}

/// For a capture that failed its proof: the image's size, how many of its
/// pixels have the outline's colour, and the first of them (row by row).
/// Marks drawn but not at the proof point show a count with a place;
/// marks missing show none. Reads the whole image: for the failure's
/// report only, which this is the text of.
pub fn mark_census(png: &[u8]) -> String {
    let (mut count, mut first) = (0u64, None);
    let dims = rows(png, None, |r, row, bpp| {
        for (i, p) in row.chunks_exact(bpp).enumerate() {
            if is_mark([p[0], p[1], p[2]]) {
                count += 1;
                first.get_or_insert((i as u32, r));
            }
        }
    });
    match (dims, first) {
        (None, _) => "unreadable".into(),
        (Some((w, h)), None) => format!("{w}x{h}, no marked pixel"),
        (Some((w, h)), Some((x, y))) => format!("{w}x{h}, {count} marked pixels, the first at ({x}, {y})"),
    }
}

/// For a capture without its marks: how many pixels are not white and
/// the box they fill (None: a blank image, the page's content missing
/// too), and a hash of the bytes, to tell one capture from the next.
pub fn content_census(png: &[u8]) -> String {
    let (mut count, mut bbox) = (0u64, None::<(u32, u32, u32, u32)>);
    rows(png, None, |r, row, bpp| {
        for (i, p) in row.chunks_exact(bpp).enumerate() {
            if p[..3] != [255, 255, 255] {
                count += 1;
                let i = i as u32;
                bbox = Some(bbox.map_or((i, r, i, r), |(a, b, c, d)| (a.min(i), b.min(r), c.max(i), d.max(r))));
            }
        }
    });
    let hash = png.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| (h ^ b as u64).wrapping_mul(0x100_0000_01b3));
    format!("{} bytes, hash {hash:016x}, {count} pixels not white in {bbox:?}", png.len())
}

/// Decode an 8-bit RGB or RGBA, non-interlaced PNG's rows down to `last`
/// (all of them when None), handing each to `visit` with its index and
/// bytes per pixel. Returns (width, height); None when the image cannot
/// be read so far. Only the rows asked for are inflated.
fn rows(png: &[u8], last: Option<u32>, mut visit: impl FnMut(u32, &[u8], usize)) -> Option<(u32, u32)> {
    let body = png.strip_prefix(b"\x89PNG\r\n\x1a\n")?;
    let (mut at, mut idat, mut head) = (0usize, Vec::new(), None);
    while let Some(len) = body.get(at..at + 4).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize) {
        let kind = body.get(at + 4..at + 8)?;
        let data = body.get(at + 8..(at + 8).checked_add(len)?)?;
        match kind {
            b"IHDR" => head = Some(data),
            b"IDAT" => idat.extend_from_slice(data),
            b"IEND" => break,
            _ => {}
        }
        at += len.checked_add(12)?;
    }
    let head = head.filter(|h| h.len() >= 13)?;
    let width = u32::from_be_bytes([head[0], head[1], head[2], head[3]]);
    let height = u32::from_be_bytes([head[4], head[5], head[6], head[7]]);
    let bpp = match (head[8], head[9], head[12]) {
        (8, 2, 0) => 3usize,
        (8, 6, 0) => 4,
        _ => return None,
    };
    let y = last.unwrap_or(height.checked_sub(1)?);
    if y >= height || width > 1 << 16 {
        return None;
    }
    let line = 1 + width as usize * bpp;
    let want = line.checked_mul(y as usize + 1)?;
    if want > 512 << 20 {
        return None;
    }
    // A limit short of the whole image ends the inflate early, with what
    // it has so far: that is all this reads.
    let raw = match miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(&idat, want) {
        Ok(raw) => raw,
        Err(e) => e.output,
    };
    if raw.len() < want {
        return None;
    }
    // Each row is filtered against the one above: undo them in order.
    let mut above = vec![0u8; line - 1];
    let mut row = vec![0u8; line - 1];
    for r in 0..=y as usize {
        let (filter, data) = raw[r * line..(r + 1) * line].split_first()?;
        for i in 0..data.len() {
            let left = if i >= bpp { row[i - bpp] } else { 0 };
            let up = above[i];
            let diag = if i >= bpp { above[i - bpp] } else { 0 };
            let add = match filter {
                0 => 0,
                1 => left,
                2 => up,
                3 => ((left as u16 + up as u16) / 2) as u8,
                4 => {
                    let p = left as i16 + up as i16 - diag as i16;
                    let (pa, pb, pc) = ((p - left as i16).abs(), (p - up as i16).abs(), (p - diag as i16).abs());
                    if pa <= pb && pa <= pc {
                        left
                    } else if pb <= pc {
                        up
                    } else {
                        diag
                    }
                }
                _ => return None,
            };
            row[i] = data[i].wrapping_add(add);
        }
        visit(r as u32, &row, bpp);
        std::mem::swap(&mut above, &mut row);
    }
    Some((width, height))
}

/// Answers after the page's next frame (100 ms at most).
pub const NEXT_FRAME: &str =
    "new Promise((done) => { setTimeout(() => done(true), 100); requestAnimationFrame(() => done(true)); })";

/// Takes the marks away.
pub const REMOVE: &str = "(() => { document.getElementById('__nv_marks')?.remove(); return true; })()";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refs_come_from_the_tree_in_order_once() {
        let tree = "page \"Shop\"\n- button \"Buy\" [ref=12]\n- link \"Cart\" [ref=7]\n- text: [ref=x] not one\n- button \"Buy\" [ref=12]";
        assert_eq!(refs_in(tree), vec![12, 7]);
        assert!(refs_in("no refs here").is_empty());
    }

    /// A PNG as a browser writes one: every filter type, RGB and RGBA,
    /// the pixel read back from the rows above it only.
    #[test]
    fn one_pixel_is_read_from_a_png_without_the_rest() {
        fn chunk(kind: &[u8], data: &[u8], out: &mut Vec<u8>) {
            out.extend_from_slice(&(data.len() as u32).to_be_bytes());
            out.extend_from_slice(kind);
            out.extend_from_slice(data);
            out.extend_from_slice(&[0; 4]);
        }
        for bpp in [3usize, 4] {
            let (w, h) = (7usize, 6usize);
            // Two pixels in the outline's colour, for the census.
            let colour = |x: usize, y: usize| match (x, y) {
                (4, 3) | (6, 5) => [0xe6, 0x00, 0x7e, 255],
                _ => [(x * 40 + y) as u8, (y * 37 + 3) as u8, (x * y * 9) as u8, 255],
            };
            let mut raw = Vec::new();
            let mut above = vec![0u8; w * bpp];
            for y in 0..h {
                let plain: Vec<u8> = (0..w).flat_map(|x| colour(x, y)[..bpp].to_vec()).collect();
                let filter = (y % 5) as u8;
                raw.push(filter);
                for i in 0..plain.len() {
                    let left = if i >= bpp { plain[i - bpp] } else { 0 };
                    let diag = if i >= bpp { above[i - bpp] } else { 0 };
                    let up = above[i];
                    let sub = match filter {
                        0 => 0,
                        1 => left,
                        2 => up,
                        3 => ((left as u16 + up as u16) / 2) as u8,
                        _ => {
                            let p = left as i16 + up as i16 - diag as i16;
                            let (pa, pb, pc) =
                                ((p - left as i16).abs(), (p - up as i16).abs(), (p - diag as i16).abs());
                            if pa <= pb && pa <= pc {
                                left
                            } else if pb <= pc {
                                up
                            } else {
                                diag
                            }
                        }
                    };
                    raw.push(plain[i].wrapping_sub(sub));
                }
                above = plain;
            }
            let packed = miniz_oxide::deflate::compress_to_vec_zlib(&raw, 6);
            let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
            let mut head = Vec::new();
            head.extend_from_slice(&(w as u32).to_be_bytes());
            head.extend_from_slice(&(h as u32).to_be_bytes());
            head.extend_from_slice(&[8, if bpp == 3 { 2 } else { 6 }, 0, 0, 0]);
            chunk(b"IHDR", &head, &mut png);
            // In two pieces, as a large image comes.
            chunk(b"IDAT", &packed[..packed.len() / 2], &mut png);
            chunk(b"IDAT", &packed[packed.len() / 2..], &mut png);
            chunk(b"IEND", &[], &mut png);
            for (x, y) in [(0, 0), (6, 0), (3, 2), (0, 5), (6, 5), (4, 4)] {
                assert_eq!(
                    pixel(&png, x as u32, y as u32),
                    Some(colour(x, y)[..3].try_into().unwrap()),
                    "{bpp} {x},{y}"
                );
            }
            assert_eq!(pixel(&png, 7, 0), None);
            assert_eq!(pixel(&png, 0, 6), None);
            assert_eq!(pixel(&png[..png.len() / 2], 6, 5), None, "a cut image proves nothing");
            // The census against a plain count of the same pixels.
            let colour_rgb = |x: usize, y: usize| -> [u8; 3] { colour(x, y)[..3].try_into().unwrap() };
            let marked: Vec<_> =
                (0..h).flat_map(|y| (0..w).map(move |x| (x, y))).filter(|&(x, y)| is_mark(colour_rgb(x, y))).collect();
            assert_eq!(marked, [(4, 3), (6, 5)]);
            assert_eq!(mark_census(&png), "7x6, 2 marked pixels, the first at (4, 3)", "{bpp}");
            assert_eq!(mark_census(&png[..png.len() / 2]), "unreadable");
            // Not white: against a plain count and box of the same pixels.
            let lit: Vec<_> = (0..h)
                .flat_map(|y| (0..w).map(move |x| (x, y)))
                .filter(|&(x, y)| colour_rgb(x, y) != [255; 3])
                .collect();
            let (x0, y0) = (lit.iter().map(|p| p.0).min().unwrap(), lit.iter().map(|p| p.1).min().unwrap());
            let (x1, y1) = (lit.iter().map(|p| p.0).max().unwrap(), lit.iter().map(|p| p.1).max().unwrap());
            let census = content_census(&png);
            let want = format!("{} pixels not white in Some(({x0}, {y0}, {x1}, {y1}))", lit.len());
            assert!(census.starts_with(&format!("{} bytes, hash ", png.len())) && census.ends_with(&want), "{census}");
        }
        assert_eq!(pixel(b"\xff\xd8 a jpeg", 0, 0), None);
        assert!(is_mark([0xe6, 0x00, 0x7e]) && is_mark([0xea, 0x33, 0x80]));
        assert!(!is_mark([255, 255, 255]) && !is_mark([0, 0, 0]) && !is_mark([255, 0, 0]));
    }

    #[test]
    fn the_proof_point_is_the_topmost_bottom_edge_no_label_covers() {
        let boxes = [(1, 2000.0, 10.0, 40.0, 40.0), (2, 10.0, 300.0, 50.0, 30.0), (3, 8.5, 20.0, 100.0, 30.0)];
        // The first is off the viewport; of the others, the higher one.
        assert_eq!(proof_point(&boxes, 1280.0, 720.0), Some((9.5, 49.0)));
        // A label of the next box over that edge: the next box is used.
        let stacked = [(1, 10.0, 20.0, 100.0, 30.0), (2, 10.0, 60.0, 100.0, 30.0)];
        assert_eq!(proof_point(&stacked, 1280.0, 720.0), Some((11.0, 89.0)));
        // Too small to have an edge clear of its own label, or nothing.
        assert_eq!(proof_point(&[(1, 10.0, 60.0, 50.0, 8.0)], 1280.0, 720.0), None);
        assert_eq!(proof_point(&[], 1280.0, 720.0), None);
    }

    #[test]
    fn the_script_carries_every_box() {
        let js = draw(&[(12, 10.0, 20.0, 30.0, 40.0)]);
        assert!(js.contains("[[12,10.0,20.0,30.0,40.0]]") && js.contains("attachShadow"), "{js}");
    }
}
