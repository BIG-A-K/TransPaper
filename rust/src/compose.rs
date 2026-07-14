use anyhow::{Context, Result};
use mupdf::pdf::page::{InsertImageOptions, PageImageSource};
use mupdf::pdf::PdfDocument;
use mupdf::shape::{Shape, TextOptions, TextboxOptions};
use mupdf::{Colorspace, Font, ImageFormat, Matrix, Pixmap, Point, Quad, Rect};
use std::collections::HashSet;
use std::path::Path;

use crate::schema::{InlineMath, TranslatedPage, TranslationSegment};

const FONT_SIZE_SCALE: f32 = 0.95;
const MIN_FONT_SIZE: f32 = 5.5;
const MAX_FONT_SIZE: f32 = 28.0;
const COVER_PADDING: f32 = 1.2;
const LINE_SPACING: f32 = 1.05;
const DEDUP_IOS_THRESHOLD: f32 = 0.6;
const INLINE_MATH_ZOOM: f32 = 6.0;

pub struct ComposeResult {
    pub segment_count: usize,
    pub warnings: Vec<String>,
}

pub fn compose_pdf(
    original_pdf: &Path,
    translated_pages: &[TranslatedPage],
    output_pdf: &Path,
    dedup_enabled: bool,
) -> Result<ComposeResult> {
    if translated_pages.is_empty() {
        anyhow::bail!("翻訳済みセグメントが空です");
    }

    let mut doc = PdfDocument::open(original_pdf.to_str().unwrap())
        .with_context(|| format!("Failed to open PDF: {original_pdf:?}"))?;
    let source_doc = mupdf::Document::open(original_pdf.to_str().unwrap())
        .with_context(|| format!("Failed to open source PDF: {original_pdf:?}"))?;

    let mut warnings = Vec::new();
    let mut segment_count = 0;

    for entry in translated_pages {
        let page_number = entry.page;
        if page_number == 0 {
            warnings.push(format!("ページ番号が不正のためスキップ: {page_number}"));
            continue;
        }
        let page_idx = (page_number - 1) as i32;

        let mut page = match doc.load_pdf_page(page_idx) {
            Ok(p) => p,
            Err(e) => {
                warnings.push(format!("ページ{page_number}の読み込みに失敗: {e}"));
                continue;
            }
        };
        let source_page = source_doc
            .load_page(page_idx)
            .with_context(|| format!("Failed to load source page {page_number}"))?;
        let source_pixmap = if entry.segments.iter().any(|segment| {
            segment
                .inline_math
                .as_ref()
                .is_some_and(|items| !items.is_empty())
        }) {
            Some(
                source_page
                    .to_pixmap(
                        &Matrix::new_scale(INLINE_MATH_ZOOM, INLINE_MATH_ZOOM),
                        &Colorspace::device_rgb(),
                        false,
                        false,
                    )
                    .with_context(|| format!("Failed to render source page {page_number}"))?,
            )
        } else {
            None
        };

        // Collect text/caption bboxes to limit redaction scope
        let text_rects: Vec<Rect> = entry
            .segments
            .iter()
            .filter(|s| s.seg_type == "text" || s.seg_type == "caption")
            .filter(|s| {
                s.translated_text
                    .as_ref()
                    .map(|t| !t.trim().is_empty())
                    .unwrap_or(false)
            })
            .map(|s| {
                let (x0, y0, x1, y1) = s.bbox;
                Rect::new(x0 as f32, y0 as f32, x1 as f32, y1 as f32)
            })
            .collect();

        // Strip original text within text/caption regions
        strip_page_text(&mut page, &text_rects, &mut warnings, page_number);

        // Dedup after redaction — dropped segments have original text removed and no translation placed
        let dropped: HashSet<usize> = if dedup_enabled {
            let page_bounds = page.bounds().unwrap_or(Rect::new(0.0, 0.0, 612.0, 792.0));
            let page_area = page_bounds.width() * page_bounds.height();
            dedup_text_segments(
                &entry.segments,
                DEDUP_IOS_THRESHOLD,
                page_area,
                page_number,
                &mut warnings,
            )
        } else {
            HashSet::new()
        };

        for (i, segment) in entry.segments.iter().enumerate() {
            if dropped.contains(&i) {
                continue;
            }
            let (x0, y0, x1, y1) = segment.bbox;
            let rect = Rect::new(x0 as f32, y0 as f32, x1 as f32, y1 as f32);

            match segment.seg_type.as_str() {
                "image" | "table" | "math" => {
                    match place_region_snapshot(&mut page, &doc, page_idx, rect) {
                        Ok(true) => segment_count += 1,
                        Ok(false) => {}
                        Err(e) => warnings.push(format!(
                            "Page {page_number}: {} region placement failed: {e}",
                            segment.seg_type
                        )),
                    }
                }
                _ => {
                    let text = segment
                        .translated_text
                        .as_ref()
                        .map(|t| t.trim())
                        .unwrap_or("");
                    if text.is_empty() {
                        continue;
                    }

                    let font_size = determine_font_size(segment, text);
                    if let (Some(inline_math), Some(source_pixmap)) =
                        (segment.inline_math.as_deref(), source_pixmap.as_ref())
                    {
                        if !inline_math.is_empty() {
                            if let Some(segment_warnings) = &segment.translation_warnings {
                                warnings.extend(segment_warnings.iter().map(|warning| {
                                    format!(
                                        "{warning} (page={page_number}, id={})",
                                        segment.id.as_deref().unwrap_or("?")
                                    )
                                }));
                            }
                            match place_inline_math_segment(
                                &mut page,
                                &mut doc,
                                source_pixmap,
                                rect,
                                text,
                                inline_math,
                                font_size,
                                &mut warnings,
                                page_number,
                                segment.id.as_deref(),
                            ) {
                                Ok(Some(true)) => {
                                    segment_count += 1;
                                    continue;
                                }
                                Ok(Some(false)) => continue,
                                Ok(None) => {}
                                Err(error) => warnings.push(format!(
                                    "Page {page_number}: inline math placement failed: {error}"
                                )),
                            }
                        }
                    }
                    let fallback_text;
                    let text = if segment
                        .inline_math
                        .as_ref()
                        .is_some_and(|items| !items.is_empty())
                    {
                        fallback_text = replace_inline_placeholders(
                            text,
                            segment.inline_math.as_deref().unwrap(),
                        );
                        fallback_text.as_str()
                    } else {
                        text
                    };
                    match place_text(
                        &mut page,
                        &mut doc,
                        rect,
                        text,
                        font_size,
                        &mut warnings,
                        page_number,
                    ) {
                        Ok(true) => segment_count += 1,
                        Ok(false) => {}
                        Err(e) => {
                            warnings.push(format!("Page {page_number}: text placement failed: {e}"))
                        }
                    }
                }
            }
        }
    }

    if let Some(parent) = output_pdf.parent() {
        std::fs::create_dir_all(parent)?;
    }
    doc.save(output_pdf.to_str().unwrap())
        .with_context(|| format!("Failed to save PDF: {output_pdf:?}"))?;

    Ok(ComposeResult {
        segment_count,
        warnings,
    })
}

fn strip_page_text(
    page: &mut mupdf::pdf::PdfPage,
    text_rects: &[Rect],
    warnings: &mut Vec<String>,
    page_number: usize,
) {
    if text_rects.is_empty() {
        return;
    }

    let words = match page.words(mupdf::TextExtractOptions::default()) {
        Ok(w) => w,
        Err(e) => {
            warnings.push(format!("Page {page_number}: text extraction failed: {e}"));
            return;
        }
    };

    for word in &words {
        let wb = &word.bounds;
        let in_text_region = text_rects.iter().any(|tr| rects_overlap(tr, wb));
        if !in_text_region {
            continue;
        }
        let rect = expand_rect(wb, COVER_PADDING);
        if let Err(e) = page.add_redact_annotation(rect) {
            warnings.push(format!("Page {page_number}: redact annotation failed: {e}"));
        }
    }

    if let Err(e) = page.apply_redactions() {
        warnings.push(format!("Page {page_number}: apply redactions failed: {e}"));
    }
}

fn rects_overlap(a: &Rect, b: &Rect) -> bool {
    a.x0 < b.x1 && a.x1 > b.x0 && a.y0 < b.y1 && a.y1 > b.y0
}

/// セグメントがテキスト配置対象か（重複間引きの参加条件）。
/// image/table/math・空テキストは対象外。
fn is_text_placement_target(seg: &TranslationSegment) -> bool {
    match seg.seg_type.as_str() {
        "image" | "table" | "math" => return false,
        _ => {}
    }
    seg.translated_text
        .as_ref()
        .map(|t| !t.trim().is_empty())
        .unwrap_or(false)
}

/// 2つの矩形の共通面積を返す（重なり無しは 0.0）。
fn rect_intersection_area(a: &Rect, b: &Rect) -> f32 {
    let x0 = a.x0.max(b.x0);
    let y0 = a.y0.max(b.y0);
    let x1 = a.x1.min(b.x1);
    let y1 = a.y1.min(b.y1);
    if x1 <= x0 || y1 <= y0 {
        return 0.0;
    }
    (x1 - x0) * (y1 - y0)
}

/// 配置前のテキストセグメント重複を間引き、間引き対象 index のセットを返す（案A: 2パス方式）。
///
/// IoS（共通面積 / 小さい方の面積）>= threshold のペアを重複とみなし、
/// テキスト長（chars().count()）が短い方を除外する。同長の場合は登場順が早い方を残す。
/// 実際にテキスト配置されるセグメントのみが対象。
///
/// 巨大 bbox（ページ面積の40%超）は seg の異常検出とみなし処理順を後回しにする。
/// 通常の重複では長い方を残し（issue #0002 の b016/b017 は同サイズ→長い方 b016）、
/// 巨大 bbox では先に accepted に入った個別セグメントを残して巨大 bbox を間引く。
fn dedup_text_segments(
    segments: &[TranslationSegment],
    threshold: f32,
    page_area: f32,
    page_number: usize,
    warnings: &mut Vec<String>,
) -> HashSet<usize> {
    let mut dropped = HashSet::new();
    if threshold <= 0.0 || segments.len() < 2 {
        return dropped;
    }

    let giant_area_threshold = page_area * 0.4; // ページ面積の40%超で巨大 bbox 扱い

    // (idx, text_len, area) を収集。テキスト長は translated_text の文字数。
    let mut candidates: Vec<(usize, usize, f32)> = Vec::new();
    for (idx, seg) in segments.iter().enumerate() {
        if !is_text_placement_target(seg) {
            continue;
        }
        let text_len = seg
            .translated_text
            .as_deref()
            .map(|s| s.chars().count())
            .unwrap_or(0);
        let bbox = seg.bbox;
        let rect_tmp = Rect::new(bbox.0 as f32, bbox.1 as f32, bbox.2 as f32, bbox.3 as f32);
        let area_tmp = rect_tmp.width() * rect_tmp.height();
        candidates.push((idx, text_len, area_tmp));
    }

    // 通常セグメント（面積 ≤ giant_threshold）と巨大セグメントで分ける
    let mut normal: Vec<(usize, usize, f32)> = Vec::new();
    let mut giant: Vec<(usize, usize, f32)> = Vec::new();
    for c in candidates {
        if c.2 <= giant_area_threshold {
            normal.push(c);
        } else {
            giant.push(c);
        }
    }

    // 通常は「テキスト長降順（長い方優先）→ idx 昇順」で残す優先度を決定
    normal.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    // 巨大は後回し（idx 昇順で安定）
    giant.sort_by(|a, b| a.0.cmp(&b.0));

    let ordered: Vec<(usize, usize, f32)> = normal.into_iter().chain(giant).collect();

    let mut accepted: Vec<(usize, Rect, f32)> = Vec::new();
    let mut dropped_pairs: Vec<(usize, usize)> = Vec::new(); // (dropped_idx, kept_idx)

    for (idx, _text_len, area) in &ordered {
        let bbox = segments[*idx].bbox;
        let rect = Rect::new(bbox.0 as f32, bbox.1 as f32, bbox.2 as f32, bbox.3 as f32);
        if *area <= 0.0 {
            continue;
        }
        let mut kept_idx: Option<usize> = None;
        for (acc_idx, acc_rect, acc_area) in &accepted {
            let inter = rect_intersection_area(&rect, acc_rect);
            if inter <= 0.0 {
                continue;
            }
            let ios = inter / area.min(*acc_area);
            if ios >= threshold {
                kept_idx = Some(*acc_idx);
                break;
            }
        }
        match kept_idx {
            Some(k) => dropped_pairs.push((*idx, k)),
            None => accepted.push((*idx, rect, *area)),
        }
    }

    if dropped_pairs.is_empty() {
        return dropped;
    }

    for (dropped_idx, kept_idx) in &dropped_pairs {
        dropped.insert(*dropped_idx);
        let dropped_id = segments[*dropped_idx].id.as_deref().unwrap_or("?");
        let kept_id = segments[*kept_idx].id.as_deref().unwrap_or("?");
        warnings.push(format!(
            "重複セグメントを間引きました (page={page_number}, dropped_id={dropped_id}, kept_id={kept_id}, IoS≥{threshold})"
        ));
    }

    dropped
}

fn place_region_snapshot(
    _page: &mut mupdf::pdf::PdfPage,
    _doc: &PdfDocument,
    _page_idx: i32,
    rect: Rect,
) -> Result<bool> {
    if rect.is_empty() {
        return Ok(false);
    }
    // Image/table/math regions are preserved by not redacting them.
    // The original content stays in place since we only redact text regions.
    Ok(true)
}

#[derive(Debug, Clone)]
enum InlineLayoutKind {
    Text(String),
    Space,
    Math(usize),
}

#[derive(Debug, Clone)]
struct InlineLayoutItem {
    kind: InlineLayoutKind,
    width: f32,
    height: f32,
    ascent: f32,
    descent: f32,
    x: f32,
    y: f32,
    baseline: f32,
}

fn replace_inline_placeholders(text: &str, inline_math: &[InlineMath]) -> String {
    inline_math.iter().fold(text.to_string(), |result, math| {
        if math.placeholder.is_empty() {
            result
        } else {
            result.replace(&math.placeholder, &math.text)
        }
    })
}

fn split_inline_text(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut current_kind: Option<u8> = None;
    for ch in text.chars() {
        let kind = if ch.is_whitespace() {
            0
        } else if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '/' | ':' | '_' | '-') {
            1
        } else {
            2
        };
        if kind == 2 {
            if !current.is_empty() {
                tokens.push(std::mem::take(&mut current));
                current_kind = None;
            }
            tokens.push(ch.to_string());
            continue;
        }
        if current_kind.is_some_and(|existing| existing != kind) && !current.is_empty() {
            tokens.push(std::mem::take(&mut current));
        }
        current.push(ch);
        current_kind = Some(kind);
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

fn measure_text(font: &Font, text: &str, font_size: f32) -> f32 {
    let measured = text
        .chars()
        .map(|ch| {
            font.encode_character(ch as i32)
                .and_then(|glyph| font.advance_glyph(glyph))
                .unwrap_or(if ch.is_whitespace() { 0.35 } else { 1.0 })
        })
        .sum::<f32>()
        * font_size;
    measured.max(if text.is_empty() {
        0.0
    } else {
        font_size * 0.1
    })
}

fn math_dimensions(math: &InlineMath, font_size: f32, max_width: f32) -> (f32, f32, f32) {
    let original_width = (math.bbox.2 - math.bbox.0).max(0.1) as f32;
    let original_height = (math.bbox.3 - math.bbox.1).max(0.1) as f32;
    let mut height = math
        .font_size
        .filter(|size| *size > 0.0)
        .map(|size| original_height * font_size / size as f32)
        .unwrap_or(font_size)
        .clamp(font_size * 0.75, font_size * 1.8);
    let mut width = height * original_width / original_height;
    if width > max_width {
        let scale = max_width / width;
        width *= scale;
        height *= scale;
    }
    let baseline_ratio = math
        .baseline
        .map(|baseline| ((baseline - math.bbox.1) / (math.bbox.3 - math.bbox.1)) as f32)
        .unwrap_or(0.8)
        .clamp(0.55, 0.95);
    (width, height, baseline_ratio)
}

fn append_plain_inline_items(
    items: &mut Vec<InlineLayoutItem>,
    text: &str,
    font: &Font,
    font_size: f32,
    max_width: f32,
) {
    for token in split_inline_text(text) {
        let width = measure_text(font, &token, font_size);
        let parts: Vec<String> =
            if width > max_width && token.chars().count() > 1 && !token.trim().is_empty() {
                token.chars().map(|ch| ch.to_string()).collect()
            } else {
                vec![token]
            };
        for part in parts {
            let is_space = part.chars().all(char::is_whitespace);
            items.push(InlineLayoutItem {
                width: measure_text(font, &part, font_size),
                height: font_size,
                ascent: font_size * 0.82,
                descent: font_size * 0.18,
                kind: if is_space {
                    InlineLayoutKind::Space
                } else {
                    InlineLayoutKind::Text(part)
                },
                x: 0.0,
                y: 0.0,
                baseline: 0.0,
            });
        }
    }
}

fn make_inline_items(
    text: &str,
    inline_math: &[InlineMath],
    font: &Font,
    font_size: f32,
    max_width: f32,
) -> Option<Vec<InlineLayoutItem>> {
    if inline_math.is_empty()
        || inline_math
            .iter()
            .any(|math| math.placeholder.is_empty() || text.matches(&math.placeholder).count() != 1)
    {
        return None;
    }

    let mut items = Vec::new();
    let mut cursor = 0usize;
    while cursor < text.len() {
        let next = inline_math
            .iter()
            .enumerate()
            .filter_map(|(index, math)| {
                text[cursor..]
                    .find(&math.placeholder)
                    .map(|offset| (cursor + offset, index))
            })
            .min_by_key(|(position, _)| *position);
        let Some((position, math_index)) = next else {
            append_plain_inline_items(&mut items, &text[cursor..], font, font_size, max_width);
            break;
        };
        append_plain_inline_items(
            &mut items,
            &text[cursor..position],
            font,
            font_size,
            max_width,
        );
        let math = &inline_math[math_index];
        let (width, height, baseline_ratio) = math_dimensions(math, font_size, max_width);
        let ascent = height * baseline_ratio;
        items.push(InlineLayoutItem {
            kind: InlineLayoutKind::Math(math_index),
            width,
            height,
            ascent,
            descent: height - ascent,
            x: 0.0,
            y: 0.0,
            baseline: 0.0,
        });
        cursor = position + math.placeholder.len();
    }
    Some(items)
}

fn layout_inline_items(items: &mut [InlineLayoutItem], rect: Rect, font_size: f32) -> bool {
    let mut lines: Vec<Vec<usize>> = Vec::new();
    let mut line: Vec<usize> = Vec::new();
    let mut line_width = 0.0f32;
    for index in 0..items.len() {
        let is_space = matches!(items[index].kind, InlineLayoutKind::Space);
        if is_space && line.is_empty() {
            continue;
        }
        if !line.is_empty() && line_width + items[index].width > rect.width() {
            while line
                .last()
                .is_some_and(|last| matches!(items[*last].kind, InlineLayoutKind::Space))
            {
                line.pop();
            }
            if !line.is_empty() {
                lines.push(std::mem::take(&mut line));
            }
            line_width = 0.0;
            if is_space {
                continue;
            }
        }
        line.push(index);
        line_width += items[index].width;
    }
    while line
        .last()
        .is_some_and(|last| matches!(items[*last].kind, InlineLayoutKind::Space))
    {
        line.pop();
    }
    if !line.is_empty() {
        lines.push(line);
    }

    let mut y = rect.y0;
    for line in lines {
        let ascent = line
            .iter()
            .map(|index| items[*index].ascent)
            .fold(0.0f32, f32::max);
        let descent = line
            .iter()
            .map(|index| items[*index].descent)
            .fold(0.0f32, f32::max);
        let line_height = (ascent + descent).max(font_size * LINE_SPACING);
        let baseline = y + ascent;
        let mut x = rect.x0;
        for index in line {
            let item = &mut items[index];
            item.x = x;
            item.baseline = baseline;
            item.y = baseline - item.ascent;
            x += item.width;
        }
        y += line_height;
    }
    y <= rect.y1 + 0.01
}

#[allow(clippy::too_many_arguments)]
fn place_inline_math_segment(
    page: &mut mupdf::pdf::PdfPage,
    doc: &mut PdfDocument,
    source_pixmap: &Pixmap,
    rect: Rect,
    text: &str,
    inline_math: &[InlineMath],
    font_size: f32,
    warnings: &mut Vec<String>,
    page_number: usize,
    segment_id: Option<&str>,
) -> Result<Option<bool>> {
    let font_data =
        mupdf_fonts_droid::cjk_font(0, false).context("Droid CJK font should be available")?;
    let measure_font = Font::from_bytes_with_index(font_data.name, font_data.index, font_data.data)
        .context("Failed to load CJK font for inline math layout")?;
    let mut attempt_size = font_size * 0.9;
    let mut items = Vec::new();
    let mut fits = false;
    for _ in 0..14 {
        let Some(mut attempt_items) =
            make_inline_items(text, inline_math, &measure_font, attempt_size, rect.width())
        else {
            warnings.push(format!(
                "文中数式プレースホルダーが不正なためテキスト配置へフォールバック (page={page_number}, id={})",
                segment_id.unwrap_or("?")
            ));
            return Ok(None);
        };
        fits = layout_inline_items(&mut attempt_items, rect, attempt_size);
        items = attempt_items;
        if fits || attempt_size <= MIN_FONT_SIZE + 0.1 {
            break;
        }
        attempt_size = (attempt_size * 0.9).max(MIN_FONT_SIZE);
    }
    if items.is_empty() {
        return Ok(Some(false));
    }
    if !fits {
        warnings.push(format!(
            "文中数式を含むテキストが収まりきりませんでした (page={page_number}, id={})",
            segment_id.unwrap_or("?")
        ));
    }

    let text_options = TextOptions {
        fontsize: attempt_size,
        lineheight: LINE_SPACING,
        fontname: font_data.name.to_string(),
        fontfile: Some(font_data.data),
        simple: false,
        ..Default::default()
    };
    let mut drew_text = false;
    {
        let mut shape = Shape::new(page).context("Failed to create inline text shape")?;
        for item in &items {
            if item.y + item.height > rect.y1 + 0.01 {
                continue;
            }
            if let InlineLayoutKind::Text(value) = &item.kind {
                shape.insert_text(Point::new(item.x, item.baseline), value, &text_options)?;
                drew_text = true;
            }
        }
        if drew_text {
            shape
                .commit(doc, true)
                .context("Failed to commit inline text")?;
        }
    }

    let scale = Matrix::new_scale(INLINE_MATH_ZOOM, INLINE_MATH_ZOOM);
    let page_bounds = page.bounds().context("Failed to get page bounds")?;
    let mut drew_math = false;
    for item in &items {
        if item.y + item.height > rect.y1 + 0.01 {
            continue;
        }
        let InlineLayoutKind::Math(math_index) = &item.kind else {
            continue;
        };
        let math = &inline_math[*math_index];
        let source_rect = Rect::new(
            math.bbox.0 as f32,
            math.bbox.1 as f32,
            math.bbox.2 as f32,
            math.bbox.3 as f32,
        );
        if source_rect.is_empty() {
            continue;
        }
        let pixel_rect = source_rect.transform(&scale);
        let crop_width = pixel_rect.width().ceil().max(1.0) as i32;
        let crop_height = pixel_rect.height().ceil().max(1.0) as i32;
        // fz_warp_pixmap expects corners clockwise from NW: NW, NE, SE, SW.
        // Rect::quad() produces NW, NE, SW, SE (ll/lr swapped), so build manually.
        let warp_quad = Quad::new(
            Point::new(pixel_rect.x0, pixel_rect.y0),
            Point::new(pixel_rect.x1, pixel_rect.y0),
            Point::new(pixel_rect.x1, pixel_rect.y1),
            Point::new(pixel_rect.x0, pixel_rect.y1),
        );
        let cropped = source_pixmap
            .warp(warp_quad, crop_width, crop_height)
            .context("Failed to crop source inline math")?;
        let mut png = Vec::new();
        cropped.write_to(&mut png, ImageFormat::PNG)?;
        let destination =
            Rect::new(item.x, item.y, item.x + item.width, item.y + item.height).intersect(&rect);
        if destination.is_empty() {
            continue;
        }
        // PdfPage::insert_image expects PDF bottom-left coordinates, while text
        // extraction and Shape use MuPDF's top-left page coordinates.
        let pdf_destination = Rect::new(
            destination.x0,
            page_bounds.y0 + page_bounds.y1 - destination.y1,
            destination.x1,
            page_bounds.y0 + page_bounds.y1 - destination.y0,
        );
        page.insert_image(
            doc,
            pdf_destination,
            PageImageSource::Bytes {
                data: &png,
                format_hint: Some("png"),
            },
            InsertImageOptions::default(),
        )?;
        drew_math = true;
    }
    Ok(Some(drew_text || drew_math))
}

fn place_text(
    page: &mut mupdf::pdf::PdfPage,
    doc: &mut PdfDocument,
    rect: Rect,
    text: &str,
    font_size: f32,
    warnings: &mut Vec<String>,
    page_number: usize,
) -> Result<bool> {
    let mut attempt_size = font_size;
    let mut last_overflow: f32 = 0.0;
    let font =
        mupdf_fonts_droid::cjk_font(0, false).context("Droid CJK font should be available")?;

    for _ in 0..12 {
        let opts = TextboxOptions {
            fontsize: attempt_size,
            lineheight: LINE_SPACING,
            fontname: font.name.to_owned(),
            fontfile: Some(font.data),
            simple: false,
            ..Default::default()
        };

        let mut shape = Shape::new(page).context("Failed to create shape")?;
        match shape.insert_textbox(rect, text, &opts) {
            Ok(unused) if unused >= 0.0 => {
                shape.commit(doc, true).context("Failed to commit shape")?;
                return Ok(true);
            }
            Ok(overflow) => {
                last_overflow = -overflow;
                if attempt_size <= MIN_FONT_SIZE + 0.1 {
                    break;
                }
                let new_size = (attempt_size * 0.9).max(MIN_FONT_SIZE);
                attempt_size = if (new_size - attempt_size).abs() < 0.1 {
                    MIN_FONT_SIZE
                } else {
                    new_size
                };
            }
            Err(e) => {
                warnings.push(format!("Page {page_number}: insert_textbox failed: {e}"));
                return Ok(false);
            }
        }
    }

    let opts = TextboxOptions {
        fontsize: MIN_FONT_SIZE,
        lineheight: LINE_SPACING,
        fontname: font.name.to_owned(),
        fontfile: Some(font.data),
        simple: false,
        ..Default::default()
    };
    let expanded = Rect::new(rect.x0, rect.y0, rect.x1, rect.y1 + last_overflow + 1.0);
    let mut shape = Shape::new(page).context("Failed to create shape")?;
    match shape.insert_textbox(expanded, text, &opts) {
        Ok(_) => {
            shape.commit(doc, true).context("Failed to commit shape")?;
            warnings.push(format!(
                "Page {page_number}: min font size {MIN_FONT_SIZE} overflowed, expanded rect"
            ));
            Ok(true)
        }
        Err(e) => {
            warnings.push(format!("Page {page_number}: force-place failed: {e}"));
            Ok(false)
        }
    }
}

fn determine_font_size(segment: &crate::schema::TranslationSegment, translated: &str) -> f32 {
    let base = segment
        .avg_font_size
        .map(|s| s as f32)
        .filter(|&s| s > 0.0)
        .unwrap_or(MIN_FONT_SIZE);

    let mut size = base * FONT_SIZE_SCALE;

    // Adaptive: shrink if translated text is much longer than source
    let src_chars = segment.char_count.unwrap_or(0);
    let visible_text = segment
        .inline_math
        .as_deref()
        .map(|inline_math| replace_inline_placeholders(translated, inline_math))
        .unwrap_or_else(|| translated.to_string());
    let tgt_len = visible_text.chars().count();
    if src_chars > 0 {
        let ratio = tgt_len as f32 / src_chars as f32;
        if ratio > 1.0 {
            let shrink = ratio.powf(0.7).min(4.0);
            size /= shrink.max(1.0);
        }
    }

    size.clamp(MIN_FONT_SIZE, MAX_FONT_SIZE)
}

fn expand_rect(rect: &Rect, padding: f32) -> Rect {
    Rect::new(
        rect.x0 - padding,
        rect.y0 - padding,
        rect.x1 + padding,
        rect.y1 + padding,
    )
}

pub fn create_comparison_pdf(
    original_pdf: &Path,
    translated_pdf: &Path,
    output_pdf: &Path,
) -> Result<std::path::PathBuf> {
    let orig_doc = mupdf::Document::open(original_pdf.to_str().unwrap())?;
    let trans_doc = mupdf::Document::open(translated_pdf.to_str().unwrap())?;

    let orig_count = orig_doc.page_count()?;
    let trans_count = trans_doc.page_count()?;

    if orig_count != trans_count {
        anyhow::bail!("PDFのページ数が一致しません (元: {orig_count}, 翻訳後: {trans_count})");
    }

    // For comparison PDF, use PyMuPDF-style interleaving
    // This requires lower-level PDF operations; for now, use a simpler approach
    let mut out_doc = PdfDocument::new();

    for page_idx in 0..orig_count {
        // Insert original page
        let orig_page = orig_doc.load_page(page_idx)?;
        let bounds = orig_page.bounds()?;
        let mut new_page =
            out_doc.new_page_at(-1, (bounds.x1 - bounds.x0, bounds.y1 - bounds.y0))?;
        let pixmap = orig_page.to_pixmap(
            &Matrix::new_scale(2.0, 2.0),
            &Colorspace::device_rgb(),
            false,
            false,
        )?;
        let mut png_buf = Vec::new();
        pixmap.write_to(&mut png_buf, ImageFormat::PNG)?;
        new_page.insert_image(
            &mut out_doc,
            Rect::new(bounds.x0, bounds.y0, bounds.x1, bounds.y1),
            PageImageSource::Bytes {
                data: &png_buf,
                format_hint: Some("png"),
            },
            InsertImageOptions::default(),
        )?;

        // Insert translated page
        let trans_page = trans_doc.load_page(page_idx)?;
        let mut new_page2 =
            out_doc.new_page_at(-1, (bounds.x1 - bounds.x0, bounds.y1 - bounds.y0))?;
        let pixmap2 = trans_page.to_pixmap(
            &Matrix::new_scale(2.0, 2.0),
            &Colorspace::device_rgb(),
            false,
            false,
        )?;
        let mut png_buf2 = Vec::new();
        pixmap2.write_to(&mut png_buf2, ImageFormat::PNG)?;
        new_page2.insert_image(
            &mut out_doc,
            Rect::new(bounds.x0, bounds.y0, bounds.x1, bounds.y1),
            PageImageSource::Bytes {
                data: &png_buf2,
                format_hint: Some("png"),
            },
            InsertImageOptions::default(),
        )?;
    }

    if let Some(parent) = output_pdf.parent() {
        std::fs::create_dir_all(parent)?;
    }
    out_doc.save(output_pdf.to_str().unwrap())?;

    Ok(output_pdf.to_path_buf())
}

// PoC functions kept for testing
pub fn poc_pdf_to_png(pdf_path: &Path, page_index: usize, output_path: &Path) -> Result<()> {
    let doc = mupdf::Document::open(pdf_path.to_str().unwrap())
        .with_context(|| format!("Failed to open PDF: {pdf_path:?}"))?;
    let page = doc.load_page(page_index as i32)?;
    let dpi = 150.0;
    let matrix = Matrix::new_scale(dpi / 72.0, dpi / 72.0);
    let pixmap = page.to_pixmap(&matrix, &Colorspace::device_rgb(), false, false)?;
    pixmap.save_as(output_path.to_str().unwrap(), ImageFormat::PNG)?;
    tracing::info!("PDF page {} rendered to {:?}", page_index, output_path);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::TranslationSegment;

    fn make_seg(
        id: &str,
        seg_type: &str,
        bbox: (f64, f64, f64, f64),
        translated: Option<&str>,
        source: Option<&str>,
    ) -> TranslationSegment {
        TranslationSegment {
            id: Some(id.to_string()),
            seg_type: seg_type.to_string(),
            bbox,
            source_text: source.map(|s| s.to_string()),
            char_count: None,
            avg_font_size: None,
            translated_text: translated.map(|s| s.to_string()),
            inline_math: None,
            inline_math_status: None,
            translation_warnings: None,
        }
    }

    #[test]
    fn rect_intersection_area_disjoint_is_zero() {
        let a = Rect::new(0.0, 0.0, 10.0, 10.0);
        let b = Rect::new(20.0, 20.0, 30.0, 30.0);
        assert_eq!(rect_intersection_area(&a, &b), 0.0);
    }

    #[test]
    fn rect_intersection_area_identical_is_full_area() {
        let a = Rect::new(0.0, 0.0, 10.0, 20.0);
        assert_eq!(rect_intersection_area(&a, &a), 200.0);
    }

    #[test]
    fn rect_intersection_area_partial_overlap() {
        let a = Rect::new(0.0, 0.0, 10.0, 10.0); // area 100
        let b = Rect::new(5.0, 5.0, 15.0, 15.0); // 共通 5x5 = 25
        assert_eq!(rect_intersection_area(&a, &b), 25.0);
    }

    #[test]
    fn rect_intersection_area_fully_contained() {
        let outer = Rect::new(0.0, 0.0, 10.0, 10.0); // 100
        let inner = Rect::new(2.0, 2.0, 4.0, 4.0); // 4, 共通 = 4
        assert_eq!(rect_intersection_area(&outer, &inner), 4.0);
    }

    #[test]
    fn is_text_placement_target_excludes_non_text_types() {
        for t in ["image", "table", "math"] {
            let seg = make_seg("x", t, (0.0, 0.0, 1.0, 1.0), Some("hello"), None);
            assert!(
                !is_text_placement_target(&seg),
                "type {t} should be excluded"
            );
        }
    }

    #[test]
    fn is_text_placement_target_excludes_empty_text() {
        for txt in [None, Some(""), Some("   ")] {
            let seg = make_seg("x", "text", (0.0, 0.0, 1.0, 1.0), txt, None);
            assert!(
                !is_text_placement_target(&seg),
                "empty text {txt:?} excluded"
            );
        }
    }

    #[test]
    fn is_text_placement_target_accepts_text_with_content() {
        let seg = make_seg("x", "caption", (0.0, 0.0, 1.0, 1.0), Some("hello"), None);
        assert!(is_text_placement_target(&seg));
    }

    #[test]
    fn dedup_noop_for_short_input() {
        let mut warns = Vec::new();
        let segs = vec![make_seg(
            "a",
            "text",
            (0.0, 0.0, 10.0, 10.0),
            Some("x"),
            None,
        )];
        let dropped = dedup_text_segments(&segs, 0.6, 1_000_000.0, 1, &mut warns);
        assert!(dropped.is_empty());
        assert!(warns.is_empty());

        // threshold 無効化
        let segs2 = vec![
            make_seg("a", "text", (0.0, 0.0, 10.0, 10.0), Some("x"), None),
            make_seg("b", "text", (0.0, 0.0, 10.0, 10.0), Some("y"), None),
        ];
        let dropped = dedup_text_segments(&segs2, 0.0, 1_000_000.0, 1, &mut warns);
        assert!(dropped.is_empty());
    }

    #[test]
    fn dedup_drops_larger_bbox_keeps_smaller_when_contained() {
        // 案Aの巨大 bbox ケース。giant(ページ面積の50%) に small(4%) が完全包含（IoS=1.0）。
        // giant は「異常検出」とみなし処理を後回し→先に accepted に入った small を残し、giant を dropped とする。
        // （p11_b001 問題: ページ全体を覆う巨大 bbox が個別セグメントを吸収するのを防ぐ）
        let segs = vec![
            make_seg(
                "giant",
                "text",
                (0.0, 0.0, 100.0, 50.0),
                Some("giant bbox text"),
                None,
            ),
            make_seg(
                "small",
                "text",
                (10.0, 10.0, 30.0, 30.0),
                Some("small"),
                None,
            ),
        ];
        let mut warns = Vec::new();
        // page_area = 10000, giant_threshold = 4000
        // giant(5000) > 4000 → 巨大、small(400) ≤ 4000 → 通常
        let dropped = dedup_text_segments(&segs, 0.6, 10_000.0, 1, &mut warns);
        assert_eq!(dropped.len(), 1);
        assert!(dropped.contains(&0), "giant bbox (idx 0) should be dropped");
        assert_eq!(warns.len(), 1);
        assert!(warns[0].contains("dropped_id=giant"));
        assert!(warns[0].contains("kept_id=small"));
    }

    #[test]
    fn dedup_same_area_keeps_longer_text() {
        // 同面積で重複（IoS=1.0）の場合はテキスト長が長い方を残す。
        // issue #0002 の本来ケース（b016/b017: 同サイズ・長い方を残す）に相当。
        let segs = vec![
            make_seg("short", "text", (0.0, 0.0, 100.0, 10.0), Some("abc"), None),
            make_seg(
                "long",
                "text",
                (0.0, 0.0, 100.0, 10.0),
                Some("abcdefghij"),
                None,
            ),
        ];
        let mut warns = Vec::new();
        let dropped = dedup_text_segments(&segs, 0.6, 1_000_000.0, 1, &mut warns);
        assert!(
            dropped.contains(&0),
            "shorter (idx 0) should be dropped at same area"
        );
        assert!(warns[0].contains("kept_id=long"));
    }

    #[test]
    fn dedup_keeps_earlier_when_same_length() {
        // 同長なら登場順早い方（idx 0）を残す
        let segs = vec![
            make_seg(
                "first",
                "text",
                (0.0, 0.0, 100.0, 10.0),
                Some("12345"),
                None,
            ),
            make_seg(
                "second",
                "text",
                (0.0, 0.0, 100.0, 10.0),
                Some("abcde"),
                None,
            ),
        ];
        let mut warns = Vec::new();
        let dropped = dedup_text_segments(&segs, 0.6, 1_000_000.0, 1, &mut warns);
        assert!(dropped.contains(&1), "later (idx 1) should be dropped");
        assert!(warns[0].contains("kept_id=first"));
    }

    #[test]
    fn dedup_threshold_boundary_exactly_06_is_dropped() {
        // IoS 丁度 0.6 → 間引き対象（>= threshold）
        // a: (0,0)-(10,10)=100, b: (4,0)-(10,10)=60, 共通=(4,0)-(10,10)=60, IoS=60/60=1.0
        // IoS=1.0 ではなく正確に0.6を作る例:
        // a: (0,0)-(10,10)=100, b: (4,0)-(10,10)=60, 共通=60, IoS=60/min(100,60)=1.0
        // 0.6 を作るには: small area S, 共通 = 0.6*S
        // small=(0,0)-(10,6)=60, large=(0,0)-(10,10)=100, 共通=(0,0)-(10,6)=60, IoS=60/60=1.0
        // → 包含は常に IoS=1.0。0.6 を作るには部分重複:
        // small=(0,0)-(10,10)=100, other=(4,0)-(14,10)=100, 共通=(4,0)-(10,10)=60, IoS=60/100=0.6
        let segs = vec![
            make_seg(
                "a",
                "text",
                (0.0, 0.0, 10.0, 10.0),
                Some("longer text here!!"),
                None,
            ),
            make_seg("b", "text", (4.0, 0.0, 14.0, 10.0), Some("short"), None),
        ];
        let mut warns = Vec::new();
        let dropped = dedup_text_segments(&segs, 0.6, 1_000_000.0, 1, &mut warns);
        assert!(
            dropped.contains(&1),
            "IoS=0.6 should be dropped at threshold 0.6"
        );
    }

    #[test]
    fn dedup_below_threshold_not_dropped() {
        // 共通 < 0.6*min → 間引き無し
        // a:(0,0)-(10,10)=100, b:(6,0)-(16,10)=100, 共通=(6,0)-(10,10)=40, IoS=40/100=0.4
        let segs = vec![
            make_seg("a", "text", (0.0, 0.0, 10.0, 10.0), Some("longer"), None),
            make_seg(
                "b",
                "text",
                (6.0, 0.0, 16.0, 10.0),
                Some("short but different"),
                None,
            ),
        ];
        let mut warns = Vec::new();
        let dropped = dedup_text_segments(&segs, 0.6, 1_000_000.0, 1, &mut warns);
        assert!(dropped.is_empty(), "IoS=0.4 < 0.6 should not be dropped");
    }

    #[test]
    fn dedup_ignores_non_target_segments() {
        // image/table/math は候補に入らない → 间引き対象外
        let segs = vec![
            make_seg("img", "image", (0.0, 0.0, 10.0, 10.0), Some("x"), None),
            make_seg("cap", "caption", (0.0, 0.0, 10.0, 10.0), Some("y"), None),
        ];
        let mut warns = Vec::new();
        let dropped = dedup_text_segments(&segs, 0.6, 1_000_000.0, 1, &mut warns);
        assert!(dropped.is_empty(), "non-text targets are not deduped");
    }

    #[test]
    fn dedup_uses_translated_then_source_length() {
        // translated_text 未設定 → source_text の長さで比較
        let segs = vec![
            make_seg("a", "text", (0.0, 0.0, 10.0, 10.0), None, Some("short")),
            make_seg(
                "b",
                "text",
                (0.0, 0.0, 10.0, 10.0),
                None,
                Some("much longer source text"),
            ),
        ];
        let mut warns = Vec::new();
        let dropped = dedup_text_segments(&segs, 0.6, 1_000_000.0, 1, &mut warns);
        // a も b も translated_text 未設定 → is_text_placement_target は false（空扱い）
        // → 候補に入らず、間引き無し
        assert!(dropped.is_empty());
    }

    #[test]
    fn dedup_counts_chars_not_bytes() {
        // 日本語: chars().count() で比較（バイト数ではない）
        // a: 5文字 "あいうえお" = 15 byte, b: 3文字 "abc" = 3 byte → a(5) > b(3) で a を残す
        let segs = vec![
            make_seg(
                "a",
                "text",
                (0.0, 0.0, 10.0, 10.0),
                Some("あいうえお"),
                None,
            ),
            make_seg("b", "text", (0.0, 0.0, 10.0, 10.0), Some("abc"), None),
        ];
        let mut warns = Vec::new();
        let dropped = dedup_text_segments(&segs, 0.6, 1_000_000.0, 1, &mut warns);
        assert!(
            dropped.contains(&1),
            "shorter (b, idx1) dropped; char-count based"
        );
        assert!(warns[0].contains("kept_id=a"));
    }

    #[test]
    fn inline_math_items_replace_placeholder_with_math_item() {
        let font_data = mupdf_fonts_droid::cjk_font(0, false).unwrap();
        let font =
            Font::from_bytes_with_index(font_data.name, font_data.index, font_data.data).unwrap();
        let placeholder = "[[TRANSPAPER_INLINE_MATH_0001]]";
        let math = InlineMath {
            id: "m0001".to_string(),
            placeholder: placeholder.to_string(),
            text: "H×W".to_string(),
            bbox: (100.0, 20.0, 125.0, 40.0),
            baseline: Some(36.0),
            font_size: Some(12.0),
            ..Default::default()
        };

        let mut items = make_inline_items(
            &format!("結果 {placeholder} です"),
            &[math],
            &font,
            12.0,
            240.0,
        )
        .unwrap();

        assert!(items
            .iter()
            .any(|item| matches!(item.kind, InlineLayoutKind::Math(0))));
        assert!(layout_inline_items(
            &mut items,
            Rect::new(10.0, 10.0, 250.0, 60.0),
            12.0
        ));
    }

    #[test]
    fn invalid_inline_math_placeholder_uses_legacy_fallback_text() {
        let math = InlineMath {
            placeholder: "[[TRANSPAPER_INLINE_MATH_0001]]".to_string(),
            text: "H×W".to_string(),
            ..Default::default()
        };

        assert_eq!(
            replace_inline_placeholders("shape broken", std::slice::from_ref(&math)),
            "shape broken"
        );
        assert_eq!(
            replace_inline_placeholders("shape [[TRANSPAPER_INLINE_MATH_0001]]", &[math]),
            "shape H×W"
        );
    }

    #[test]
    fn compose_inserts_source_snapshot_for_inline_math() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_dir = std::env::temp_dir().join(format!("transpaper-inline-math-{unique}"));
        std::fs::create_dir_all(&temp_dir).unwrap();
        let source = temp_dir.join("source.pdf");
        let output = temp_dir.join("translated.pdf");

        let mut source_doc = PdfDocument::new();
        let mut source_page = source_doc.new_page_at(-1, (300.0, 160.0)).unwrap();
        let mut source_shape = Shape::new(&mut source_page).unwrap();
        source_shape
            .insert_text(
                Point::new(20.0, 40.0),
                "Original formula HxW and prose",
                &TextOptions {
                    fontsize: 12.0,
                    ..Default::default()
                },
            )
            .unwrap()
            .commit(&mut source_doc, true)
            .unwrap();
        source_doc.save(source.to_str().unwrap()).unwrap();

        let placeholder = "[[TRANSPAPER_INLINE_MATH_0001]]";
        let pages = vec![TranslatedPage {
            page: 1,
            segments: vec![TranslationSegment {
                id: Some("inline".to_string()),
                seg_type: "text".to_string(),
                bbox: (20.0, 20.0, 280.0, 85.0),
                source_text: Some(format!("formula {placeholder}")),
                char_count: Some(12),
                avg_font_size: Some(12.0),
                translated_text: Some(format!("結果 {placeholder} です")),
                inline_math: Some(vec![InlineMath {
                    id: "m0001".to_string(),
                    placeholder: placeholder.to_string(),
                    text: "HxW".to_string(),
                    bbox: (108.0, 27.1, 134.0, 43.6),
                    baseline: Some(40.0),
                    font_size: Some(12.0),
                    ..Default::default()
                }]),
                inline_math_status: Some("preserved".to_string()),
                translation_warnings: None,
            }],
        }];

        let result = compose_pdf(&source, &pages, &output, true).unwrap();
        let output_doc = mupdf::Document::open(output.to_str().unwrap()).unwrap();
        let output_page = output_doc.load_page(0).unwrap();
        let text = output_page
            .text(mupdf::TextExtractOptions::default())
            .unwrap();
        let structured = output_page
            .to_text_page(mupdf::TextPageFlags::PRESERVE_IMAGES)
            .unwrap()
            .structured();
        let text_sizes: Vec<f32> = structured
            .blocks
            .iter()
            .flat_map(|block| match &block.content {
                mupdf::TextBlockContent::Text { lines } => lines
                    .iter()
                    .flat_map(|line| line.chars.iter().map(|ch| ch.size))
                    .collect::<Vec<_>>(),
                _ => Vec::new(),
            })
            .collect();
        let image_bounds: Vec<Rect> = structured
            .blocks
            .iter()
            .filter_map(|block| {
                matches!(block.content, mupdf::TextBlockContent::Image { .. })
                    .then_some(block.bounds)
            })
            .collect();

        assert_eq!(result.segment_count, 1);
        assert!(!text.contains("TRANSPAPER_INLINE_MATH"));
        assert!(!text_sizes.is_empty());
        assert!(text_sizes.iter().all(|size| *size >= 10.0));
        assert!(
            !image_bounds.is_empty(),
            "inline math should be inserted as an image"
        );
        assert!(
            image_bounds
                .iter()
                .any(|bounds| bounds.y0 >= 20.0 && bounds.y1 <= 85.0),
            "inline math image should stay inside the translated text bbox: {image_bounds:?}"
        );
    }
}
