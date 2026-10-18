//! The shared icon renderer (§ 4, docs/design/custom-icon.md CI1): Summary is the built-in configuration
//! `docs/design/icon-summary.config.json`; Custom uses the same schema, `validate` and `render`. The compact q stays its
//! own approved asset. Layout mirrors `paint` of docs/design/custom-icon.html in physical pixels.

use serde_json::Value;

/// The built-in Summary configuration (CI1: immutable, versioned).
pub const SUMMARY: &str = include_str!("../docs/design/icon-summary.config.json");

pub fn summary() -> Value {
    serde_json::from_str(SUMMARY).expect("icon-summary.config.json")
}

const VERSION: u64 = 1;
const FONTS: [&str; 2] = ["System", "Mono"];
const BINDS: [&str; 4] = ["work", "ask", "ready", "failed"];
const SERIES: [&str; 4] = ["closed", "failed", "ask", "blocked"];

/// Every schema error of a draft, one per field (CI2 Validate); empty: valid.
pub fn validate(c: &Value) -> Vec<String> {
    let Some(o) = c.as_object() else { return vec!["config: not an object".into()] };
    let mut err = vec![];
    if c["version"].as_u64() != Some(VERSION) {
        err.push(format!("version: {} unsupported, want {VERSION}", c["version"]));
    }
    for k in o.keys().filter(|k| !["version", "font", "gap", "elements"].contains(&k.as_str())) {
        err.push(format!("{k}: unknown field"));
    }
    if !c["font"].as_str().is_some_and(|f| FONTS.contains(&f)) {
        err.push(format!("font: {} unsupported, want one of {FONTS:?}", c["font"]));
    }
    if !c["gap"].as_u64().is_some_and(|g| g <= 2) {
        err.push(format!("gap: {} unsupported, want 0, 1 or 2", c["gap"]));
    }
    let Some(es) = c["elements"].as_array().filter(|es| (1..=8).contains(&es.len())) else {
        err.push("elements: want 1 to 8 elements".into());
        return err;
    };
    let mut cells: Vec<(usize, [f64; 4])> = vec![];
    for (i, e) in es.iter().enumerate() {
        let at = format!("elements[{i}]");
        let Some(eo) = e.as_object() else {
            err.push(format!("{at}: not an object"));
            continue;
        };
        for k in eo.keys().filter(|k| !["id", "type", "bind", "cell", "align", "vertical", "period", "series"].contains(&k.as_str())) {
            err.push(format!("{at}.{k}: unknown field"));
        }
        match e["id"].as_str() {
            None => err.push(format!("{at}.id: want a string")),
            Some(id) if es[..i].iter().any(|o| o["id"] == id) => err.push(format!("{at}.id: duplicate {id}")),
            _ => {}
        }
        let cell: Option<Vec<f64>> = e["cell"].as_array().filter(|a| a.len() == 4).and_then(|a| a.iter().map(Value::as_f64).collect());
        match cell {
            Some(v) if v.iter().all(|x| (0.0..=1.0).contains(x)) && v[2] > 0.0 && v[3] > 0.0 && v[0] + v[2] <= 1.0 && v[1] + v[3] <= 1.0 => {
                let v = [v[0], v[1], v[2], v[3]];
                for (j, o) in &cells {
                    if v[0] < o[0] + o[2] && o[0] < v[0] + v[2] && v[1] < o[1] + o[3] && o[1] < v[1] + v[3] {
                        err.push(format!("{at}.cell: overlaps elements[{j}]"));
                    }
                }
                cells.push((i, v));
            }
            _ => err.push(format!("{at}.cell: want [x, y, w, h] inside the square with w, h > 0")),
        }
        let one = |k: &str, allowed: &[&str], err: &mut Vec<String>| {
            if !e[k].as_str().is_some_and(|v| allowed.contains(&v)) {
                err.push(format!("{at}.{k}: {} unsupported, want one of {allowed:?}", e[k]));
            }
        };
        match e["type"].as_str() {
            Some("counter") => {
                one("bind", &BINDS, &mut err);
                one("align", &["left", "center", "right"], &mut err);
                one("vertical", &["top", "center", "bottom"], &mut err);
            }
            Some("histogram") => {
                one("period", &["24h"], &mut err);
                if e["series"] != serde_json::json!(SERIES) {
                    err.push(format!("{at}.series: want {SERIES:?}"));
                }
            }
            _ => err.push(format!("{at}.type: {} unsupported, want counter or histogram", e["type"])),
        }
    }
    err
}

/// What the icon shows: counts by bind name (None: unknown, drawn as a dash), 12 history bins of the last 24 h
/// (close, failed, ask, blocked; oldest first) and whether every project is paused.
pub struct Data {
    pub counts: [Option<usize>; 4],
    pub bins: Vec<[u32; 4]>,
    pub paused: bool,
}

/// An RGBA image, `s` pixels per point of the 22 pt reference drawing.
pub struct Rgba {
    pub w: u32,
    pub h: u32,
    pub s: f32,
    pub px: Vec<u8>,
}

impl Rgba {
    pub fn new(w: u32, h: u32, s: f32) -> Rgba {
        Rgba { w, h, s, px: vec![0; (w * h * 4) as usize] }
    }

    /// Paint `rgb` over pixel `o` with coverage `cov` (0..=1); alpha 0 clears.
    fn blend(&mut self, x: u32, y: u32, rgb: [u8; 4], cov: f32) {
        let o = ((y * self.w + x) * 4) as usize;
        let da = self.px[o + 3] as f32 / 255.0;
        if rgb[3] == 0 {
            self.px[o + 3] = (da * (1.0 - cov) * 255.0) as u8;
            return;
        }
        let a = cov * rgb[3] as f32 / 255.0;
        let oa = a + da * (1.0 - a);
        for c in 0..3 {
            self.px[o + c] = ((rgb[c] as f32 * a + self.px[o + c] as f32 * da * (1.0 - a)) / oa) as u8;
        }
        self.px[o + 3] = (oa * 255.0) as u8;
    }

    /// Paint `rgb` where `inside(x, y)` holds, in points, 4×4 samples per pixel.
    pub fn fill(&mut self, rgb: [u8; 4], inside: impl Fn(f32, f32) -> bool) {
        for y in 0..self.h {
            for x in 0..self.w {
                let hit = (0..16).filter(|i| inside((x as f32 + (i % 4) as f32 / 4.0 + 0.125) / self.s, (y as f32 + (i / 4) as f32 / 4.0 + 0.125) / self.s)).count();
                if hit > 0 {
                    self.blend(x, y, rgb, hit as f32 / 16.0);
                }
            }
        }
    }

    /// Paint whole pixels `[x0, x1) × [y0, y1)`: chart fills snap to physical pixels once (I6).
    fn pixels(&mut self, rgb: [u8; 4], x0: i32, y0: i32, x1: i32, y1: i32) {
        for y in y0.max(0)..y1.min(self.h as i32) {
            for x in x0.max(0)..x1.min(self.w as i32) {
                self.blend(x as u32, y as u32, rgb, 1.0);
            }
        }
    }

    pub fn png(&self) -> Vec<u8> {
        let mut out = vec![];
        let mut e = ::png::Encoder::new(&mut out, self.w, self.h);
        e.set_color(::png::ColorType::Rgba);
        if let Ok(mut w) = e.write_header() {
            let _ = w.write_image_data(&self.px);
        }
        out
    }
}

pub fn rect(x0: f32, y0: f32, x1: f32, y1: f32) -> impl Fn(f32, f32) -> bool {
    move |x, y| x >= x0 && x < x1 && y >= y0 && y < y1
}

fn rrect(x0: f32, y0: f32, x1: f32, y1: f32, r: f32) -> impl Fn(f32, f32) -> bool {
    move |x, y| {
        let (dx, dy) = ((x0 + r - x).max(x - x1 + r).max(0.0), (y0 + r - y).max(y - y1 + r).max(0.0));
        x >= x0 && x < x1 && y >= y0 && y < y1 && dx * dx + dy * dy <= r * r
    }
}

pub struct Palette {
    pub ink: [u8; 4],
    pub work: [u8; 4],
    pub ask: [u8; 4],
    pub muted: [u8; 4],
    pub failed: [u8; 4],
}

pub fn palette(dark: bool) -> Palette {
    if dark {
        Palette { ink: [236, 239, 242, 255], work: [119, 205, 179, 255], ask: [237, 185, 147, 255], muted: [180, 189, 200, 255], failed: [239, 137, 150, 255] }
    } else {
        Palette { ink: [37, 41, 48, 255], work: [35, 120, 98, 255], ask: [163, 92, 41, 255], muted: [98, 107, 119, 255], failed: [174, 56, 76, 255] }
    }
}

/// The angular q (18·z pt, stroke 1.8·z, tail bottom right) at `(x0, y0)` with ▶ (admits) or Ⅱ inside.
fn draw_q(img: &mut Rgba, x0: f32, y0: f32, z: f32, run: bool, ink: [u8; 4], p: &Palette) {
    let (x1, y1, t) = (x0 + 18.0 * z, y0 + 18.0 * z, 1.8 * z);
    let (outer, inner) = (rrect(x0, y0, x1, y1, t), rrect(x0 + t, y0 + t, x1 - t, y1 - t, 0.4 * z));
    img.fill(ink, |x, y| outer(x, y) && !inner(x, y));
    img.fill(ink, rect(x1 - t, y1 - z, x1, y1 + 3.6 * z));
    let (cx, cy, h) = (x0 + 9.0 * z, y0 + 9.0 * z, 4.5 * z);
    if run {
        let (l, t) = (cx - 3.15 * z, cy - h);
        img.fill(p.work, move |x, y| x >= l && y >= t + (x - l) * 4.5 / 8.1 && y <= t + 2.0 * h - (x - l) * 4.5 / 8.1);
    } else {
        img.fill(p.muted, rect(cx - h, cy - h, cx - 0.4 * h, cy + h));
        img.fill(p.muted, rect(cx + 0.4 * h, cy - h, cx + h, cy + h));
    }
}

/// The Icon view: the compact q in a 22 pt square at `s` pixels per point.
#[tracing::instrument(name="icon.compact",skip_all)]
pub fn compact(run: bool, dark: bool, s: f32) -> Rgba {
    let p = palette(dark);
    let n = (22.0 * s).round() as u32;
    let mut img = Rgba::new(n, n, s);
    draw_q(&mut img, 2.0, 0.2, 1.0, run, p.ink, &p);
    img
}

/// Draw a valid configuration `c` into an `n`×`n` pixel square at `dpr` pixels per point; an invalid one is refused
/// with every error. Paused: everything gray under the centered compact q with Ⅱ (I4).
#[tracing::instrument(name="icon.render",skip_all)]
pub fn render(c: &Value, n: u32, dpr: f32, d: &Data, dark: bool) -> Result<Rgba, Vec<String>> {
    let err = validate(c);
    if !err.is_empty() {
        return Err(err);
    }
    let p = palette(dark);
    let gray = |c: [u8; 4]| if d.paused { p.muted } else { c };
    let mut img = Rgba::new(n, n, n as f32 / 22.0);
    let (nf, pad, g) = (n as f32, (0.5 * dpr).round().max(1.0), c["gap"].as_f64().unwrap_or(1.0) as f32 * dpr);
    let mono = c["font"] == "Mono";
    for e in c["elements"].as_array().into_iter().flatten() {
        let v: Vec<f32> = e["cell"].as_array().into_iter().flatten().filter_map(Value::as_f64).map(|x| x as f32).collect();
        let edge = |f: f32, inner: f32| (pad + f * (nf - 2.0 * pad) + inner).round() as i32;
        let (x, y) = (edge(v[0], if v[0] > 0.0 { g / 2.0 } else { 0.0 }), edge(v[1], if v[1] > 0.0 { g / 2.0 } else { 0.0 }));
        let (right, bottom) = (edge(v[0] + v[2], if v[0] + v[2] < 1.0 { -g / 2.0 } else { 0.0 }), edge(v[1] + v[3], if v[1] + v[3] < 1.0 { -g / 2.0 } else { 0.0 }));
        let (w, h) = (right - x, bottom - y);
        if w < 1 || h < 1 {
            continue;
        }
        if e["type"] == "counter" {
            // I2: the largest size at which every digit, `+` and the dash fit the cell by ink; fixed per cell size
            let mut fs = (w.min(h) as f32 * 1.6) as u32;
            while fs > 1 && !"0123456789+–".chars().all(|ch| glyphs(&ch.to_string(), fs, mono).ink().is_none_or(|i| i.2 - i.0 <= w && i.1 + i.3 <= h)) {
                fs -= 1;
            }
            let digits: Vec<_> = (0..10).filter_map(|k| glyphs(&k.to_string(), fs, mono).ink()).collect();
            let (asc, desc) = (digits.iter().map(|i| i.1).max().unwrap_or(0), digits.iter().map(|i| i.3).max().unwrap_or(0));
            let i = BINDS.iter().position(|b| e["bind"] == *b).unwrap_or(0);
            let text = d.counts[i].map_or("–".to_string(), |v| v.to_string());
            let fits = |t: &str| glyphs(t, fs, mono).ink().is_none_or(|i| i.2 - i.0 <= w);
            // overflow: an explicit threshold, never clipped digits (CI4)
            let text = if fits(&text) { text } else if fits("9+") { "9+".into() } else { "+".into() };
            let m = glyphs(&text, fs, mono);
            let Some((l, _, r, _)) = m.ink() else { continue };
            let ox = match e["align"].as_str() {
                Some("right") => right - r,
                Some("center") => x + (w - (r - l)) / 2 - l,
                _ => x - l,
            };
            let oy = match e["vertical"].as_str() {
                Some("bottom") => bottom - desc,
                Some("center") => y + (h - asc - desc) / 2 + asc,
                _ => y + asc,
            };
            m.blit(&mut img, ox, oy, gray(p.ink));
        } else {
            // R-UI5: B bars b px wide, 1 px gaps, right-aligned; adjacent bins summed (D11), one peak scale
            let b = [12, 6, 4, 3, 2, 1].into_iter().find(|b| b * 2 - 1 <= w).unwrap_or(1);
            let group = 12 / b as usize;
            let rows: Vec<[u32; 4]> = (0..b as usize).map(|i| (0..4).map(|k| d.bins.iter().skip(i * group).take(group).map(|r| r[k]).sum()).collect::<Vec<_>>().try_into().unwrap()).collect();
            let peak = rows.iter().map(|r| r.iter().sum::<u32>()).max().unwrap_or(0).max(1) as f32;
            let bw = (w - b + 1) / b;
            let start = right - (b * bw + b - 1);
            for (i, row) in rows.iter().enumerate() {
                let mut total = 0;
                for (k, cnt) in row.iter().enumerate() {
                    let low = (h as f32 * total as f32 / peak).round() as i32;
                    total += cnt;
                    let high = (h as f32 * total as f32 / peak).round() as i32;
                    let bx = start + i as i32 * (bw + 1);
                    img.pixels(gray([p.work, p.failed, p.ask, p.muted][k]), bx, bottom - high, bx + bw, bottom - low);
                }
            }
        }
    }
    if d.paused {
        // the compact q at 3/4 size on a cleared square, centered at N/2 (I4)
        let z = 0.75;
        let (x0, y0) = (11.0 - 9.0 * z, 11.0 - 9.0 * z - 0.9 * z);
        img.fill([0, 0, 0, 0], rect(x0 - z, y0 - z, x0 + 19.0 * z, y0 + 22.6 * z));
        draw_q(&mut img, x0, y0, z, false, p.muted, &p);
    }
    Ok(img)
}

/// The alpha of `text` drawn with its pen origin at (`ox`, baseline `base`) of a `w`×`h` mask.
struct Mask {
    w: i32,
    h: i32,
    ox: i32,
    base: i32,
    a: Vec<u8>,
}

impl Mask {
    /// Painted ink edges relative to the pen origin: (left, ascent, right, descent); None: no ink.
    fn ink(&self) -> Option<(i32, i32, i32, i32)> {
        let lit: Vec<(i32, i32)> = (0..self.h).flat_map(|y| (0..self.w).map(move |x| (x, y))).filter(|(x, y)| self.a[(y * self.w + x) as usize] > 0).collect();
        let (x0, x1) = (lit.iter().map(|p| p.0).min()?, lit.iter().map(|p| p.0).max()?);
        let (y0, y1) = (lit.iter().map(|p| p.1).min()?, lit.iter().map(|p| p.1).max()?);
        Some((x0 - self.ox, self.base - y0, x1 + 1 - self.ox, y1 + 1 - self.base))
    }

    fn blit(&self, img: &mut Rgba, ox: i32, oy: i32, rgb: [u8; 4]) {
        for y in 0..self.h {
            for x in 0..self.w {
                let (tx, ty, a) = (ox - self.ox + x, oy - self.base + y, self.a[(y * self.w + x) as usize]);
                if a > 0 && tx >= 0 && ty >= 0 && tx < img.w as i32 && ty < img.h as i32 {
                    img.blend(tx as u32, ty as u32, rgb, a as f32 / 255.0);
                }
            }
        }
    }
}

/// `text` in the system font (SF Pro; `mono`: SF Mono), weight 500, `size` px, native antialiasing (I2).
#[cfg(target_os = "macos")]
fn glyphs(text: &str, size: u32, mono: bool) -> Mask {
    use objc2_app_kit::{NSFont, NSFontWeightMedium};
    use objc2_core_foundation::{CGPoint, CGSize};
    use objc2_core_graphics::{CGBitmapContextCreate, CGContext, CGImageAlphaInfo};
    use objc2_core_text::{CTFont, CTFontOrientation};
    use std::ptr::NonNull;
    // SAFETY: a framework constant
    let weight = unsafe { NSFontWeightMedium };
    let font = if mono { NSFont::monospacedSystemFontOfSize_weight(size as f64, weight) } else { NSFont::systemFontOfSize_weight(size as f64, weight) };
    // SAFETY: NSFont is toll-free bridged to CTFont
    let ct = unsafe { &*(&*font as *const NSFont as *const CTFont) };
    let mut chars: Vec<u16> = text.encode_utf16().collect();
    let n = chars.len();
    let (mut g, mut adv) = (vec![0u16; n], vec![CGSize::new(0.0, 0.0); n]);
    // SAFETY: n characters in, n glyphs and advances out
    unsafe {
        ct.glyphs_for_characters(NonNull::new_unchecked(chars.as_mut_ptr()), NonNull::new_unchecked(g.as_mut_ptr()), n as isize);
        ct.advances_for_glyphs(CTFontOrientation::Horizontal, NonNull::new_unchecked(g.as_mut_ptr()), adv.as_mut_ptr(), n as isize);
    }
    let mut pos = vec![];
    let mut x = 0.0;
    for a in &adv {
        pos.push(CGPoint::new(x, 0.0));
        x += a.width;
    }
    let (sz, len) = (size as i32, n as i32);
    let (w, h) = (sz * (len + 2), sz * 3);
    let mut a = vec![0u8; (w * h) as usize];
    // SAFETY: an alpha-only bitmap over `a`, dropped before `a` is read
    unsafe {
        let ctx = CGBitmapContextCreate(a.as_mut_ptr().cast(), w as usize, h as usize, 8, w as usize, None, CGImageAlphaInfo::Only.0).expect("bitmap context");
        CGContext::translate_ctm(Some(&ctx), sz as f64, sz as f64);
        ct.draw_glyphs(NonNull::new_unchecked(g.as_mut_ptr()), NonNull::new_unchecked(pos.as_mut_ptr()), n, &ctx);
    }
    // bitmap rows run top down; the baseline is `sz` above the bottom
    Mask { w, h, ox: sz, base: h - sz, a }
}

// ponytail: seven-segment glyphs until DirectWrite text lands; Windows still shows only the compact q (menu.md M1)
#[cfg(not(target_os = "macos"))]
fn glyphs(text: &str, size: u32, _mono: bool) -> Mask {
    let u = size as f32 / 13.5;
    let (sw, sh, t) = (6.0 * u, 9.0 * u, 1.6 * u);
    let seg = [(0.0, 0.0, sw, t), (sw - t, 0.0, sw, (sh + t) / 2.0), (sw - t, (sh - t) / 2.0, sw, sh), (0.0, sh - t, sw, sh), (0.0, (sh - t) / 2.0, t, sh), (0.0, 0.0, t, (sh + t) / 2.0), (0.0, (sh - t) / 2.0, sw, (sh + t) / 2.0)];
    let plus = [(0.0, (sh - t) / 2.0, sw, (sh + t) / 2.0), ((sw - t) / 2.0, sh / 4.0, (sw + t) / 2.0, sh * 0.75)];
    let (sz, len) = (size as i32, text.chars().count() as i32);
    let mut img = Rgba::new((sz * (len + 2)) as u32, (sz * 3) as u32, 1.0);
    for (i, ch) in text.chars().enumerate() {
        let x0 = size as f32 + i as f32 * 7.0 * u;
        let y0 = 2.0 * size as f32 - sh;
        let rects: Vec<(f32, f32, f32, f32)> = match ch {
            '0'..='9' => ["abcdef", "bc", "abged", "abgcd", "fgbc", "afgcd", "afgedc", "abc", "abcdefg", "abcdfg"][ch as usize - '0' as usize].bytes().map(|c| seg[(c - b'a') as usize]).collect(),
            '+' => plus.to_vec(),
            _ => vec![seg[6]],
        };
        for (a, b, c, d) in rects {
            img.fill([255; 4], rect(x0 + a, y0 + b, x0 + c, y0 + d));
        }
    }
    Mask { w: img.w as i32, h: img.h as i32, ox: sz, base: 2 * sz, a: img.px.chunks(4).map(|p| p[3]).collect() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn data(counts: [usize; 4], paused: bool) -> Data {
        Data { counts: counts.map(Some), bins: vec![[1, 1, 1, 1]; 12], paused }
    }

    fn has(i: &Rgba, c: [u8; 4]) -> bool {
        i.px.chunks(4).any(|p| p[3] == 255 && p[..3] == c[..3])
    }

    // Chain: native tray's first icon -> always-on collector -> committed readback.
    // Risk: per-sample instrumentation stalls the UI and loses observations despite renderer-only tests passing.
    #[test]
    fn first_icon_with_capture_finishes_without_collection_loss() {
        use std::process::{Command, Stdio};
        let root = std::env::temp_dir().join(format!("qi-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        for mode in ["baseline", "capture", "expired"] {
            let log = root.join(format!("{mode}.log"));
            let output = std::fs::File::create(&log).unwrap();
            let mut child = Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "icon::tests::first_icon_capture_child", "--ignored", "--nocapture"])
                .env("TASKQ_ICON_PROBE", &root).env("TASKQ_ICON_PROBE_MODE", mode)
                .stdout(Stdio::from(output.try_clone().unwrap())).stderr(Stdio::from(output)).spawn().unwrap();
            let began = std::time::Instant::now();
            let status = loop {
                if let Some(status) = child.try_wait().unwrap() { break Some(status); }
                if began.elapsed() > std::time::Duration::from_secs(20) {
                    child.kill().unwrap(); child.wait().unwrap(); break None;
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            };
            let detail = std::fs::read_to_string(&log).unwrap();
            eprintln!("{mode}: {detail}");
            assert!(status.is_some_and(|s| s.success()), "{mode}: icon/flush failed or exceeded20s; evidence {}", log.display());
        }
        assert_eq!(std::fs::read(root.join("baseline.png")).unwrap(), std::fs::read(root.join("capture.png")).unwrap());
        assert_eq!(std::fs::read(root.join("baseline.png")).unwrap(), std::fs::read(root.join("expired.png")).unwrap());
        if !cfg!(debug_assertions) {
            let p95 = |mode: &str| {
                let mut values: Vec<u64> = serde_json::from_slice(&std::fs::read(root.join(format!("{mode}.json"))).unwrap()).unwrap();
                assert_eq!(values.len(), 30); values.sort_unstable(); values[28]
            };
            let baseline = p95("baseline");
            for mode in ["capture", "expired"] {
                let captured = p95(mode);
                eprintln!("30 renders p95: baseline={baseline}us {mode}={captured}us");
                assert!(captured <= baseline + 1000.max(baseline / 20), "{mode} capture exceeded the operation budget; evidence {}", root.display());
            }
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[ignore = "subprocess helper exercised by first_icon_with_capture_finishes_without_collection_loss"]
    fn first_icon_capture_child() {
        let root = std::path::PathBuf::from(std::env::var_os("TASKQ_ICON_PROBE").unwrap());
        let mode = std::env::var("TASKQ_ICON_PROBE_MODE").unwrap();
        let db = root.join(format!("{mode}.db"));
        let runtime = (mode != "baseline").then(|| coreq::telemetry::capture::Runtime::install(&db, "taskq", "fixture").unwrap());
        if mode == "expired" {
            coreq::telemetry::capture::diagnose("unused", std::time::Duration::from_nanos(1), 1).unwrap();
            assert_eq!(coreq::telemetry::capture::qualify().unwrap()["diagnostics"]["state"], "expired");
        }
        let mut timings = Vec::new();
        for n in 0..30 {
            let began = std::time::Instant::now();
            let image = compact(false, true, 2.0);
            timings.push(began.elapsed().as_micros() as u64);
            if n == 0 {
                eprintln!("first compact icon: {}us", timings[0]);
                std::fs::write(root.join(format!("{mode}.png")), image.png()).unwrap();
            }
        }
        std::fs::write(root.join(format!("{mode}.json")), serde_json::to_vec(&timings).unwrap()).unwrap();
        if runtime.is_some() {
            eprintln!("capture: {}", coreq::telemetry::capture::qualify().unwrap());
            let reader = coreq::telemetry::Reader::open(&db).unwrap();
            let exported = reader.export(0, 0, 1024).unwrap();
            let rows = exported["events"].as_array().unwrap();
            assert_eq!(rows.iter().filter(|r| r["observation"]["operation"] == "taskq.icon.compact" && r["observation"]["kind"] == "method").count(), 30);
            assert!(!rows.iter().any(|r| r["observation"]["source"] == "diagnostic"), "normal rendering generated internal detail");
        }
    }

    // Chain: § 4 tray icon render. Risk: same config yields different bytes; Custom differs from Summary.
    #[test]
    fn summary_is_valid_and_renders_deterministically() {
        assert_eq!(validate(&summary()), Vec::<String>::new());
        let d = data([3, 2, 9, 1], false);
        let (a, b) = (render(&summary(), 44, 2.0, &d, true).unwrap(), render(&summary(), 44, 2.0, &d, true).unwrap());
        assert_eq!(a.png(), b.png(), "one configuration, identical bytes");
        let hash: String = ring::digest::digest(&ring::digest::SHA256, &a.png()).as_ref().iter().map(|b| format!("{b:02x}")).collect();
        eprintln!("icon summary 44px dpr2 dark 3/2/9: sha256 {hash}");
        // CI-A1: Custom is the same renderer; an identical copy is pixel-identical
        let copy: Value = serde_json::from_str(&summary().to_string()).unwrap();
        assert_eq!(render(&copy, 44, 2.0, &d, true).unwrap().px, a.px);
    }

    // Chain: § 4 icon config validation. Risk: an unsafe or invalid icon config renders.
    #[test]
    fn validate_reports_every_error() {
        let mut c = summary();
        c["version"] = json!(99);
        c["script"] = json!("evil");
        c["font"] = json!("/tmp/x.ttf");
        c["elements"][0]["url"] = json!("https://x");
        c["elements"][1]["cell"] = c["elements"][0]["cell"].clone();
        c["elements"][2]["bind"] = json!("eta");
        c["elements"][3]["series"] = json!(["closed"]);
        let err = validate(&c);
        assert_eq!(err.len(), 7, "{err:#?}");
        for want in ["version", "script", "font", "elements[0].url", "elements[1].cell: overlaps elements[0]", "elements[2].bind", "elements[3].series"] {
            assert!(err.iter().any(|e| e.starts_with(want)), "{want}: {err:#?}");
        }
        assert_eq!(validate(&json!({"version": 1, "font": "System", "gap": 1, "elements": [null]})), ["elements[0]: not an object"]);
        assert!(render(&c, 44, 2.0, &data([0; 4], false), true).is_err());
    }

    // Chain: § 4 tray icon layout. Risk: ink misaligned or series on different scales.
    #[test]
    fn summary_aligns_by_ink_and_stacks_on_one_scale() {
        let p = palette(true);
        for (n, dpr) in [(22, 1.0), (44, 2.0), (66, 2.0)] {
            for counts in [[0, 0, 0, 0], [3, 2, 5, 0], [1, 7, 8, 0], [12, 3, 24, 0], [100, 12, 105, 0]] {
                let img = render(&summary(), n, dpr, &data(counts, false), true).unwrap();
                let lit = |x: u32, y0: u32, y1: u32| (y0..y1).any(|y| img.px[((y * img.w + x) * 4 + 3) as usize] > 0);
                let edge = (0.5 * dpr).round().max(1.0) as u32;
                // work ink starts at the left inset, answers end at the right inset (I3)
                assert!(lit(edge, 0, n / 2) && !lit(edge - 1, 0, n / 2), "{n} {counts:?}: work ink left");
                assert!(lit(n - edge - 1, 0, n / 2) && !lit(n - edge, 0, n / 2), "{n} {counts:?}: answers ink right");
            }
            // equal counts per series: equal pixel rows per color in a bar column
            let img = render(&summary(), n, dpr, &data([0; 4], false), true).unwrap();
            let col = |c: [u8; 4]| (0..img.h).filter(|y| img.px[((y * img.w + img.w - 2) * 4) as usize..][..3] == c[..3]).count();
            assert!(col(p.work) > 0 && col(p.work).abs_diff(col(p.failed)) <= 1 && col(p.failed).abs_diff(col(p.ask)) <= 1, "{n}");
        }
    }

    // Chain: § 4 pause shown in the icon. Risk: a paused service looks running.
    #[test]
    fn paused_is_gray_under_the_q_and_compact_shows_run_or_pause() {
        let p = palette(true);
        let img = render(&summary(), 44, 2.0, &data([3, 2, 9, 1], true), true).unwrap();
        assert!(has(&img, p.muted) && !has(&img, p.work) && !has(&img, p.failed));
        let (run, pause) = (compact(true, true, 2.0), compact(false, true, 2.0));
        assert_eq!((run.w, run.h), (44, 44));
        assert!(has(&run, p.work) && !has(&run, p.muted));
        assert!(has(&pause, p.muted) && !has(&pause, p.work));
        assert!(has(&compact(true, false, 2.0), palette(false).ink));
    }

    // Chain: R12 unknown in the icon. Risk: unknown drawn as zero; large counts widen the icon.
    #[test]
    fn unknown_is_a_dash_and_overflow_a_threshold() {
        let d = Data { counts: [None, Some(2), Some(9), Some(1)], bins: vec![[0; 4]; 12], paused: false };
        let unknown = render(&summary(), 44, 2.0, &d, true).unwrap();
        let zero = render(&summary(), 44, 2.0, &data([0, 2, 9, 1], false), true).unwrap();
        assert_ne!(unknown.px, zero.px, "unknown never draws as zero");
        let big = render(&summary(), 22, 1.0, &data([100_000, 2, 9, 1], false), true).unwrap();
        assert_eq!(big.w, 22, "values never widen the square");
    }
}
