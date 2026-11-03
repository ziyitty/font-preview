//! GDI 把文字画进 DIB，再转成 egui 纹理。
//! 用系统字体直接渲染，不把字体文件读进内存。

use std::collections::HashMap;
use std::ffi::c_void;
use std::ptr::null_mut;

use eframe::egui::{Color32, ColorImage};
use windows::core::HSTRING;
use windows::Win32::Foundation::{COLORREF, RECT};
use windows::Win32::Graphics::Gdi::*;

pub struct TextRaster {
    dc: HDC,
    fonts: HashMap<(String, i32), HFONT>,
    faces: HashMap<String, String>,
}

impl TextRaster {
    pub fn new() -> Self {
        unsafe {
            let screen = GetDC(None);
            let dc = CreateCompatibleDC(Some(screen));
            ReleaseDC(None, screen);
            Self {
                dc,
                fonts: HashMap::new(),
                faces: HashMap::new(),
            }
        }
    }

    /// GDI 用的族名可能和 DirectWrite 不同，先探测一次真实匹配到的字体名并缓存。
    pub fn face_for(&mut self, en: &str, zh: &str) -> String {
        if let Some(v) = self.faces.get(en) {
            return v.clone();
        }
        let mut resolved = String::from(en);
        if let Some(actual) = self.probe(en) {
            if same_face(&actual, en) {
                resolved = en.to_string();
            } else if !zh.is_empty() {
                if let Some(actual_zh) = self.probe(zh) {
                    if same_face(&actual_zh, zh) {
                        resolved = zh.to_string();
                    }
                }
            }
        }
        self.faces.insert(en.to_string(), resolved.clone());
        resolved
    }

    fn probe(&mut self, face: &str) -> Option<String> {
        let hf = self.hfont(face, 20);
        unsafe {
            let old = SelectObject(self.dc, HGDIOBJ(hf.0));
            let mut buf = [0u16; 256];
            let n = GetTextFaceW(self.dc, Some(&mut buf));
            SelectObject(self.dc, old);
            if n > 1 {
                Some(String::from_utf16_lossy(&buf[..n as usize - 1]))
            } else {
                None
            }
        }
    }

    fn hfont(&mut self, face: &str, px: i32) -> HFONT {
        let key = (face.to_string(), px);
        if let Some(h) = self.fonts.get(&key) {
            return *h;
        }
        let hf = unsafe {
            CreateFontW(
                -px,
                0,
                0,
                0,
                400,
                0,
                0,
                0,
                DEFAULT_CHARSET,
                OUT_TT_PRECIS,
                CLIP_DEFAULT_PRECIS,
                ANTIALIASED_QUALITY,
                DEFAULT_PITCH.0 as u32 | FF_DONTCARE.0 as u32,
                &HSTRING::from(face),
            )
        };
        self.fonts.insert(key, hf);
        hf
    }

    /// 把 text 以 face 渲染成 width×height 的位图（白字透明底），并让文字在框内垂直居中。
    /// 有些字体（如 Cambria Math）行高远大于字号，所以先量高度再落位。
    pub fn render(&mut self, face: &str, text: &str, width: u32, height: u32, px: i32) -> ColorImage {
        let w = width.clamp(1, 4096);
        let h = height.clamp(1, 2048);
        let hf = self.hfont(face, px.max(6));
        let mut buf: Vec<u16> = text.encode_utf16().collect();
        if buf.is_empty() {
            return ColorImage::new([w as usize, h as usize], vec![Color32::TRANSPARENT; (w * h) as usize]);
        }

        unsafe {
            let old_font = SelectObject(self.dc, HGDIOBJ(hf.0));
            SetBkMode(self.dc, TRANSPARENT);
            SetTextColor(self.dc, COLORREF(0x00FF_FFFF));
            SetTextAlign(self.dc, TA_TOP);

            // 先量出整段文字排完需要多高
            let mut measure = RECT {
                left: 0,
                top: 0,
                right: w as i32,
                bottom: 4096,
            };
            let mut probe = buf.clone();
            DrawTextW(
                self.dc,
                &mut probe,
                &mut measure,
                DRAW_TEXT_FORMAT(DT_CALCRECT.0 | DT_WORDBREAK.0 | DT_NOPREFIX.0),
            );
            let measured = (measure.bottom - measure.top).max(1) as u32;
            // 行高异常的字体给足空间，长文本则限制在 3 倍高度内截断
            let draw_h = measured.min(h.saturating_mul(3)).max(h);

            let bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w as i32,
                    biHeight: -(draw_h as i32), // 负数 = 自上而下
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                bmiColors: [RGBQUAD::default()],
            };

            let mut bits: *mut c_void = null_mut();
            let hbmp =
                CreateDIBSection(Some(self.dc), &bmi, DIB_RGB_COLORS, &mut bits, None, 0);
            let Ok(hbmp) = hbmp else {
                SelectObject(self.dc, old_font);
                return ColorImage::new(
                    [w as usize, h as usize],
                    vec![Color32::TRANSPARENT; (w * h) as usize],
                );
            };

            let old_bmp = SelectObject(self.dc, HGDIOBJ(hbmp.0));
            std::ptr::write_bytes(bits, 0, (w * draw_h * 4) as usize);

            let mut rect = RECT {
                left: 0,
                top: 0,
                right: w as i32,
                bottom: draw_h as i32,
            };
            DrawTextW(
                self.dc,
                &mut buf,
                &mut rect,
                DRAW_TEXT_FORMAT(DT_WORDBREAK.0 | DT_NOPREFIX.0 | DT_END_ELLIPSIS.0),
            );

            SelectObject(self.dc, old_bmp);
            SelectObject(self.dc, old_font);

            let raw = std::slice::from_raw_parts(bits as *const u8, (w * draw_h * 4) as usize);
            let ink_at = |x: u32, y: u32| -> u8 {
                let i = ((y * w + x) * 4) as usize;
                raw[i].max(raw[i + 1]).max(raw[i + 2])
            };

            // 找出实际墨迹范围，用于垂直居中
            let (mut top, mut bottom) = (u32::MAX, 0u32);
            for y in 0..draw_h {
                let mut any = false;
                for x in 0..w {
                    if ink_at(x, y) > 8 {
                        any = true;
                        break;
                    }
                }
                if any {
                    if y < top {
                        top = y;
                    }
                    bottom = y;
                }
            }

            let mut pixels = vec![Color32::TRANSPARENT; (w * h) as usize];
            if top != u32::MAX {
                let ink_h = bottom - top + 1;
                let dy = if ink_h >= h { 0 } else { (h - ink_h) / 2 };
                for y in 0..ink_h {
                    let sy = top + y;
                    if sy >= draw_h {
                        break;
                    }
                    let ty = dy + y;
                    if ty >= h {
                        break;
                    }
                    for x in 0..w {
                        let a = ink_at(x, sy);
                        if a > 0 {
                            pixels[(ty * w + x) as usize] = Color32::from_white_alpha(a);
                        }
                    }
                }
            }

            let _ = DeleteObject(HGDIOBJ(hbmp.0));
            ColorImage::new([w as usize, h as usize], pixels)
        }
    }
}

fn same_face(actual: &str, wanted: &str) -> bool {
    let norm = |s: &str| {
        s.chars()
            .filter(|c| !c.is_whitespace())
            .flat_map(|c| c.to_lowercase())
            .collect::<String>()
    };
    let a = norm(actual);
    let b = norm(wanted);
    a == b || a.contains(&b) || b.contains(&a)
}
