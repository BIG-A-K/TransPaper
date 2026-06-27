use anyhow::{Context, Result};
use mupdf::pdf::page::{InsertImageOptions, PageImageSource};
use mupdf::pdf::PdfDocument;
use mupdf::shape::{Shape, TextOptions};
use mupdf::{Colorspace, ImageFormat, Matrix, Point, Rect};
use std::path::Path;

use crate::schema::TranslatedPage;

const FONT_SIZE_SCALE: f32 = 0.95;
const MIN_FONT_SIZE: f32 = 5.5;
const MAX_FONT_SIZE: f32 = 28.0;
const COVER_PADDING: f32 = 1.2;

pub struct ComposeResult {
    pub output_path: std::path::PathBuf,
    pub page_count: usize,
    pub segment_count: usize,
    pub warnings: Vec<String>,
}

pub fn compose_pdf(
    original_pdf: &Path,
    translated_pages: &[TranslatedPage],
    output_pdf: &Path,
) -> Result<ComposeResult> {
    if translated_pages.is_empty() {
        anyhow::bail!("翻訳済みセグメントが空です");
    }

    let mut doc = PdfDocument::open(original_pdf.to_str().unwrap())
        .with_context(|| format!("Failed to open PDF: {original_pdf:?}"))?;

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

        // Strip original text via redaction
        strip_page_text(&mut page, &mut warnings, page_number);

        for segment in &entry.segments {
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
                    match place_text(&mut page, &mut doc, rect, text, font_size, &mut warnings, page_number) {
                        Ok(true) => segment_count += 1,
                        Ok(false) => {}
                        Err(e) => warnings.push(format!(
                            "Page {page_number}: text placement failed: {e}"
                        )),
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
        output_path: output_pdf.to_path_buf(),
        page_count: translated_pages.len(),
        segment_count,
        warnings,
    })
}

fn strip_page_text(
    page: &mut mupdf::pdf::PdfPage,
    warnings: &mut Vec<String>,
    page_number: usize,
) {
    let words = match page.words(mupdf::TextExtractOptions::default()) {
        Ok(w) => w,
        Err(e) => {
            warnings.push(format!("Page {page_number}: text extraction failed: {e}"));
            return;
        }
    };

    for word in &words {
        let rect = expand_rect(&word.bounds, COVER_PADDING);
        if let Err(e) = page.add_redact_annotation(rect) {
            warnings.push(format!("Page {page_number}: redact annotation failed: {e}"));
        }
    }

    if let Err(e) = page.apply_redactions() {
        warnings.push(format!("Page {page_number}: apply redactions failed: {e}"));
    }
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

fn place_text(
    page: &mut mupdf::pdf::PdfPage,
    doc: &mut PdfDocument,
    rect: Rect,
    text: &str,
    font_size: f32,
    warnings: &mut Vec<String>,
    page_number: usize,
) -> Result<bool> {
    let opts = TextOptions {
        fontsize: font_size,
        ..Default::default()
    };

    let mut shape = Shape::new(page).context("Failed to create shape")?;
    let pos = Point::new(rect.x0, rect.y0 + font_size);

    if let Err(e) = shape.insert_text(pos, text, &opts) {
        warnings.push(format!(
            "Page {page_number}: insert_text failed: {e}"
        ));
        return Ok(false);
    }

    shape.commit(doc, true).context("Failed to commit shape")?;
    Ok(true)
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
    let tgt_len = translated.len();
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
        anyhow::bail!(
            "PDFのページ数が一致しません (元: {orig_count}, 翻訳後: {trans_count})"
        );
    }

    // For comparison PDF, use PyMuPDF-style interleaving
    // This requires lower-level PDF operations; for now, use a simpler approach
    let mut out_doc = PdfDocument::new();

    for page_idx in 0..orig_count {
        // Insert original page
        let orig_page = orig_doc.load_page(page_idx)?;
        let bounds = orig_page.bounds()?;
        let mut new_page = out_doc.new_page_at(-1, (bounds.x1 - bounds.x0, bounds.y1 - bounds.y0))?;
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
            PageImageSource::Bytes { data: &png_buf, format_hint: Some("png") },
            InsertImageOptions::default(),
        )?;

        // Insert translated page
        let trans_page = trans_doc.load_page(page_idx)?;
        let mut new_page2 = out_doc.new_page_at(-1, (bounds.x1 - bounds.x0, bounds.y1 - bounds.y0))?;
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
            PageImageSource::Bytes { data: &png_buf2, format_hint: Some("png") },
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
