use anyhow::{Context, Result};
use mupdf::pdf::PdfDocument;
use mupdf::shape::{Shape, TextOptions};
use mupdf::{Colorspace, ImageFormat, Matrix, Point, Rect};
use std::path::Path;

pub fn poc_pdf_to_png(pdf_path: &Path, page_index: usize, output_path: &Path) -> Result<()> {
    let doc = mupdf::Document::open(pdf_path.to_str().unwrap())
        .with_context(|| format!("Failed to open PDF: {pdf_path:?}"))?;

    let page = doc
        .load_page(page_index as i32)
        .with_context(|| format!("Failed to load page {page_index}"))?;

    let dpi = 150.0;
    let matrix = Matrix::new_scale(dpi / 72.0, dpi / 72.0);
    let pixmap = page
        .to_pixmap(&matrix, &Colorspace::device_rgb(), false, false)
        .context("Failed to render page to pixmap")?;

    pixmap
        .save_as(output_path.to_str().unwrap(), ImageFormat::PNG)
        .context("Failed to save pixmap as PNG")?;

    tracing::info!(
        "PDF page {} rendered to {:?} ({}x{})",
        page_index,
        output_path,
        pixmap.width(),
        pixmap.height()
    );
    Ok(())
}

pub fn poc_redact_and_write(
    pdf_path: &Path,
    output_path: &Path,
    page_index: usize,
    text_to_place: &str,
    text_rect: (f32, f32, f32, f32),
) -> Result<()> {
    let mut doc = PdfDocument::open(pdf_path.to_str().unwrap())
        .with_context(|| format!("Failed to open PDF: {pdf_path:?}"))?;

    let mut page = doc
        .load_pdf_page(page_index as i32)
        .context("Failed to load page")?;

    let (x0, y0, x1, y1) = text_rect;
    let rect = Rect::new(x0, y0, x1, y1);

    page.add_redact_annotation(rect)
        .context("Failed to add redaction annotation")?;
    page.apply_redactions()
        .context("Failed to apply redactions")?;

    let opts = TextOptions {
        fontsize: 10.0,
        ..Default::default()
    };
    let mut shape = Shape::new(&mut page).context("Failed to create shape")?;
    shape
        .insert_text(Point::new(x0, y1), text_to_place, &opts)
        .context("Failed to insert text")?;
    shape
        .commit(&mut doc, true)
        .context("Failed to commit shape")?;

    doc.save(output_path.to_str().unwrap())
        .with_context(|| format!("Failed to save PDF: {output_path:?}"))?;

    tracing::info!("Redacted and wrote text to {:?}", output_path);
    Ok(())
}

pub fn poc_insert_image(
    pdf_path: &Path,
    output_path: &Path,
    page_index: usize,
    image_path: &Path,
    rect: (f32, f32, f32, f32),
) -> Result<()> {
    let mut doc = PdfDocument::open(pdf_path.to_str().unwrap())
        .with_context(|| format!("Failed to open PDF: {pdf_path:?}"))?;

    let mut page = doc
        .load_pdf_page(page_index as i32)
        .context("Failed to load page")?;

    let image_data = std::fs::read(image_path)
        .with_context(|| format!("Failed to read image: {image_path:?}"))?;

    let (x0, y0, x1, y1) = rect;
    let target_rect = Rect::new(x0, y0, x1, y1);

    use mupdf::pdf::page::{InsertImageOptions, PageImageSource};
    page.insert_image(
        &mut doc,
        target_rect,
        PageImageSource::Bytes {
            data: &image_data,
            format_hint: Some("png"),
        },
        InsertImageOptions::default(),
    )
    .context("Failed to insert image")?;

    doc.save(output_path.to_str().unwrap())
        .with_context(|| format!("Failed to save PDF: {output_path:?}"))?;

    tracing::info!("Inserted image into {:?}", output_path);
    Ok(())
}
