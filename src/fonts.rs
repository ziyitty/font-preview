#![allow(unsafe_op_in_unsafe_fn)] // DirectWrite 全是不安全调用，统一收敛在 build/read_name 边界

use std::collections::HashMap;

use pinyin::ToPinyin;
use windows::core::{BOOL, HSTRING, Interface, Result as WResult};
use windows::Win32::Graphics::DirectWrite::{
    DWriteCreateFactory, IDWriteFactory, IDWriteFont, IDWriteFontFile, IDWriteLocalFontFileLoader,
    IDWriteLocalizedStrings, DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STYLE_NORMAL,
    DWRITE_FONT_WEIGHT_NORMAL,
};

pub struct Item {
    /// CSS/GDI 里用的族名（优先英文名）
    pub name: String,
    /// 展示名（优先中文名）
    pub label: String,
    /// 另一个语言的名字，没有则空
    pub alias: String,
    pub styles: String,
    /// 字体文件绝对路径，取不到时为 None
    pub file: Option<String>,
    font: IDWriteFont,
    /// 归一化后的匹配串：英文名、中文名、全拼、首字母
    keys: Vec<String>,
}

pub struct Catalog {
    items: Vec<Item>,
    /// (字体序号, 字符) -> 是否有字形，避免每次输入都走 COM
    cache: HashMap<(u32, char), bool>,
}

impl Catalog {
    pub fn new() -> WResult<Self> {
        unsafe { Self::build() }
    }

    unsafe fn build() -> WResult<Self> {
        let factory: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
        let mut collection = None;
        factory.GetSystemFontCollection(&mut collection, false)?;
        let collection = collection.expect("系统中没有字体集合");

        let total = collection.GetFontFamilyCount();
        let mut items = Vec::with_capacity(total as usize);

        for i in 0..total {
            let Ok(family) = collection.GetFontFamily(i) else {
                continue;
            };
            let Ok(names) = family.GetFamilyNames() else {
                continue;
            };
            let en = read_name(&names, "en-us").filter(|s| !s.is_empty());
            let zh = read_name(&names, "zh-cn").filter(|s| !s.is_empty());

            let (name, label, alias) = match (en, zh) {
                (Some(e), Some(z)) if e != z => (e.clone(), z, e),
                (Some(e), _) => (e.clone(), e.clone(), String::new()),
                (None, Some(z)) => (z.clone(), z, String::new()),
                (None, None) => continue,
            };

            let faces = family.GetFontCount();
            if faces == 0 {
                continue;
            }

            let mut weights: Vec<&'static str> = Vec::new();
            let mut regular: Option<IDWriteFont> = None;
            let mut first: Option<IDWriteFont> = None;
            for j in 0..faces {
                let Ok(f) = family.GetFont(j) else { continue };
                let w = f.GetWeight().0;
                if w == DWRITE_FONT_WEIGHT_NORMAL.0 && f.GetStyle().0 == DWRITE_FONT_STYLE_NORMAL.0 {
                    regular = Some(f.clone());
                }
                if first.is_none() {
                    first = Some(f.clone());
                }
                let wn = weight_name(w);
                if !weights.contains(&wn) {
                    weights.push(wn);
                }
            }
            // 覆盖检测用 Regular，缺失时退回第一款
            let Some(font) = regular.or(first) else { continue };

            let styles = if weights.len() > 3 {
                format!("{} +{}", weights[..3].join(" · "), weights.len() - 3)
            } else {
                weights.join(" · ")
            };

            let keys = search_keys(&name, &label, &alias);
            let file = file_path(&font);

            items.push(Item {
                name,
                label,
                alias,
                styles,
                file,
                font,
                keys,
            });
        }

        items.sort_by(|a, b| a.label.to_lowercase().cmp(&b.label.to_lowercase()));
        Ok(Self {
            items,
            cache: HashMap::new(),
        })
    }

    pub fn total(&self) -> usize {
        self.items.len()
    }

    pub fn item(&self, idx: usize) -> &Item {
        &self.items[idx]
    }

    /// 统一入口：text 按字形覆盖过滤，q 按名字/拼音模糊过滤，返回排好序的下标。
    pub fn query(&mut self, text: &str, q: &str) -> Vec<usize> {
        let chars = unique_chars(text);
        let needle = normalize(q);
        let items = &self.items;
        let cache = &mut self.cache;

        let mut hits: Vec<(usize, i32)> = Vec::new();
        for (idx, item) in items.iter().enumerate() {
            let score = if needle.is_empty() {
                0
            } else {
                match name_score(item, &needle) {
                    Some(s) => s,
                    None => continue,
                }
            };
            if !chars.is_empty() && !covers(&item.font, idx as u32, &chars, cache) {
                continue;
            }
            hits.push((idx, score));
        }

        if !needle.is_empty() {
            hits.sort_by(|a, b| {
                b.1.cmp(&a.1)
                    .then_with(|| items[a.0].label.to_lowercase().cmp(&items[b.0].label.to_lowercase()))
            });
        }
        hits.into_iter().map(|(i, _)| i).collect()
    }
}

/// 完整覆盖：每个字符都必须有字形；空白/控制符/变体选择符不参与判定。
fn covers(
    font: &IDWriteFont,
    idx: u32,
    chars: &[char],
    cache: &mut HashMap<(u32, char), bool>,
) -> bool {
    for &c in chars {
        let key = (idx, c);
        let ok = match cache.get(&key) {
            Some(v) => *v,
            None => {
                let v = unsafe { font.HasCharacter(c as u32) }
                    .map(|b| b.as_bool())
                    .unwrap_or(false);
                cache.insert(key, v);
                v
            }
        };
        if !ok {
            return false;
        }
    }
    true
}

fn unique_chars(text: &str) -> Vec<char> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for c in text.chars() {
        let cp = c as u32;
        let skip = c.is_whitespace()
            || c.is_control()
            || cp == 0x200D
            || (0xFE00..=0xFE0F).contains(&cp)
            || (0xE0100..=0xE01EF).contains(&cp);
        if skip {
            continue;
        }
        if seen.insert(c) {
            out.push(c);
        }
    }
    out
}

/// 小写化并去掉空格/连字符一类分隔符，只留下字母、数字和汉字。
fn normalize(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric() || (*c as u32) >= 0x2E80)
        .flat_map(|c| c.to_lowercase())
        .collect()
}

fn search_keys(name: &str, label: &str, alias: &str) -> Vec<String> {
    let mut keys: Vec<String> = Vec::new();
    let mut push = |s: &str| {
        let k = normalize(s);
        if !k.is_empty() && !keys.contains(&k) {
            keys.push(k);
        }
    };
    push(name);
    push(label);
    if !alias.is_empty() {
        push(alias);
    }

    // 只有含汉字的名字才需要拼音
    if label.chars().any(|c| (c as u32) >= 0x2E80) {
        let mut full = String::new();
        let mut initials = String::new();
        for ch in label.chars() {
            if let Some(p) = ch.to_pinyin() {
                full.push_str(p.plain());
                initials.push_str(p.first_letter());
            } else if ch.is_ascii_alphanumeric() {
                let c = ch.to_ascii_lowercase();
                full.push(c);
                initials.push(c);
            }
        }
        push(&full);
        push(&initials);
    }

    keys
}

/// 打分：前缀 > 子串 > 子序列（模糊），取所有匹配串里的最高分。
fn name_score(item: &Item, needle: &str) -> Option<i32> {
    let qlen = needle.chars().count() as i32;
    let mut best: Option<i32> = None;
    for key in &item.keys {
        let score = if let Some(rest) = key.strip_prefix(needle) {
            1000 - (rest.chars().count() as i32).min(150)
        } else if let Some(pos) = key.find(needle) {
            820 - (key[..pos].chars().count() as i32).min(120)
        } else if let Some(gaps) = subsequence_gaps(key, needle) {
            600 - (gaps as i32) * 3
        } else {
            0
        };
        if score > 0 {
            best = Some(best.map_or(score, |b: i32| b.max(score)));
        }
    }
    let _ = qlen;
    best
}

/// needle 作为子序列能在 hay 中按顺序找到时，返回跳过的字符数。
fn subsequence_gaps(hay: &str, needle: &str) -> Option<usize> {
    let h: Vec<char> = hay.chars().collect();
    let mut i = 0usize;
    let mut gaps = 0usize;
    for nc in needle.chars() {
        let mut found = false;
        while i < h.len() {
            if h[i] == nc {
                i += 1;
                found = true;
                break;
            }
            i += 1;
            gaps += 1;
        }
        if !found {
            return None;
        }
    }
    Some(gaps)
}

/// 通过 DirectWrite 的本地字体加载器拿到字体文件真实路径（TTC 会返回容器文件）。
fn file_path(font: &IDWriteFont) -> Option<String> {
    unsafe {
        let face = font.CreateFontFace().ok()?;
        let mut count = 0u32;
        face.GetFiles(&mut count, None).ok()?;
        if count == 0 {
            return None;
        }
        let mut files: Vec<Option<IDWriteFontFile>> = vec![None; count as usize];
        face.GetFiles(&mut count, Some(files.as_mut_ptr())).ok()?;
        let file = files.into_iter().flatten().next()?;

        let mut key: *mut core::ffi::c_void = std::ptr::null_mut();
        let mut key_size = 0u32;
        file.GetReferenceKey(&mut key, &mut key_size).ok()?;
        if key.is_null() {
            return None;
        }

        let loader: IDWriteLocalFontFileLoader = file.GetLoader().ok()?.cast().ok()?;
        let mut buf = vec![0u16; 1024];
        loader
            .GetFilePathFromKey(key as *const core::ffi::c_void, key_size, &mut buf)
            .ok()?;
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        if len == 0 {
            return None;
        }
        Some(String::from_utf16_lossy(&buf[..len]))
    }
}

unsafe fn read_name(names: &IDWriteLocalizedStrings, locale: &str) -> Option<String> {
    let mut index = 0u32;
    let mut exists = BOOL(0);
    names
        .FindLocaleName(&HSTRING::from(locale), &mut index, &mut exists)
        .ok()?;
    if !exists.as_bool() {
        return None;
    }
    let len = names.GetStringLength(index).ok()? as usize;
    let mut buf = vec![0u16; len + 1];
    names.GetString(index, &mut buf).ok()?;
    Some(String::from_utf16_lossy(&buf[..len]))
}

fn weight_name(weight: i32) -> &'static str {
    match weight {
        i32::MIN..=149 => "Thin",
        150..=249 => "ExtraLight",
        250..=349 => "Light",
        350..=399 => "SemiLight",
        400..=449 => "Regular",
        450..=549 => "Medium",
        550..=649 => "SemiBold",
        650..=749 => "Bold",
        750..=849 => "ExtraBold",
        850..=949 => "Black",
        _ => "ExtraBlack",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog() -> Catalog {
        unsafe {
            let _ = windows::Win32::System::Com::CoInitializeEx(
                None,
                windows::Win32::System::Com::COINIT_APARTMENTTHREADED,
            );
        }
        Catalog::new().expect("读取系统字体失败")
    }

    fn labels(c: &Catalog, idx: &[usize]) -> Vec<String> {
        idx.iter().map(|i| c.item(*i).label.clone()).collect()
    }

    #[test]
    fn empty_query_returns_all() {
        let mut c = catalog();
        let all = c.query("", "");
        assert_eq!(all.len(), c.total());
    }

    #[test]
    fn pinyin_full_name() {
        let mut c = catalog();
        let idx = c.query("", "weiruan");
        let hit = labels(&c, &idx);
        assert!(hit.iter().any(|l| l.contains("雅黑")), "got {hit:?}");
    }

    #[test]
    fn pinyin_initials() {
        let mut c = catalog();
        let idx = c.query("", "wryh");
        let hit = labels(&c, &idx);
        assert!(hit.iter().any(|l| l.contains("雅黑")), "got {hit:?}");
    }

    #[test]
    fn pinyin_fangsong() {
        let mut c = catalog();
        let idx = c.query("", "fangsong");
        let hit = labels(&c, &idx);
        assert!(hit.iter().any(|l| l.contains("仿宋")), "got {hit:?}");
    }

    #[test]
    fn fuzzy_subsequence_latin() {
        let mut c = catalog();
        let idx = c.query("", "msyh");
        let hit = labels(&c, &idx);
        assert!(hit.iter().any(|l| l.contains("YaHei")), "got {hit:?}");
    }

    #[test]
    fn coverage_filters_unsupported() {
        let mut c = catalog();
        let idx = c.query("永字八法", "");
        let hit = labels(&c, &idx);
        assert!(!hit.is_empty(), "没有任何字体支持中文，异常");
        assert!(!hit.iter().any(|l| l == "Arial"), "Arial 不该支持中文");
    }

    #[test]
    fn coverage_plus_search() {
        let mut c = catalog();
        let idx = c.query("永", "yahei");
        let hit = labels(&c, &idx);
        assert!(hit.iter().any(|l| l.contains("雅黑")), "got {hit:?}");
    }
}

