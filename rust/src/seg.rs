use anyhow::{Context, Result};
use mupdf::{Colorspace, Matrix};
use ort::session::Session;
use ort::value::Tensor;
use std::path::Path;

use crate::model::{self, ModelSource};
use crate::schema::{PageSize, SegmentBlock, SegmentPage, TextBlockMeta};

const INPUT_SIZE: u32 = 1024;
const PAD_VALUE: f32 = 114.0 / 255.0;

const DOC_LAYOUT_CAPTION_CLASSES: &[&str] = &[
    "caption",
    "caption_figure",
    "caption_table",
    "figure_caption",
    "table_caption",
];
const DOC_LAYOUT_TABLE_CLASSES: &[&str] = &["table"];
const DOC_LAYOUT_IMAGE_CLASSES: &[&str] =
    &["figure", "image", "picture", "graphic", "photo", "table"];
const DOC_LAYOUT_MATH_CLASSES: &[&str] = &["equation", "equations", "formula", "math", "title"];
const DOC_LAYOUT_TEXT_CLASSES: &[&str] = &[
    "text",
    "plain_text",
    "heading",
    "subheading",
    "section_heading",
    "chapter_title",
    "paragraph",
    "body",
    "body_text",
    "list",
    "list_item",
    "page_header",
    "page_footer",
    "reference",
    "references",
    "footnote",
    "footnotes",
];

const DEFAULT_CLASS_NAMES: &[&str] = &[
    "title",
    "plain_text",
    "abandon",
    "figure",
    "figure_caption",
    "table",
    "table_caption",
    "table_footnote",
    "isolate_formula",
    "formula_caption",
];

fn map_doclayout_label(label: &str) -> &'static str {
    let normalized = label.trim().to_lowercase().replace(' ', "_");
    let n = normalized.as_str();
    if DOC_LAYOUT_CAPTION_CLASSES.iter().any(|&c| c == n) {
        return "caption";
    }
    if DOC_LAYOUT_TABLE_CLASSES.iter().any(|&c| c == n) {
        return "table";
    }
    if DOC_LAYOUT_IMAGE_CLASSES.iter().any(|&c| c == n) {
        return "image";
    }
    if DOC_LAYOUT_MATH_CLASSES.iter().any(|&c| c == n) {
        return "math";
    }
    if DOC_LAYOUT_TEXT_CLASSES.iter().any(|&c| c == n) {
        return "text";
    }
    "math"
}

struct PreprocessResult {
    data: Vec<f32>,
    gain: f32,
    pad_w: f32,
    pad_h: f32,
}

fn preprocess_image(img: &image::DynamicImage) -> PreprocessResult {
    let rgb = img.to_rgb8();
    let (orig_w, orig_h) = (rgb.width(), rgb.height());

    let gain = (INPUT_SIZE as f32 / orig_w as f32).min(INPUT_SIZE as f32 / orig_h as f32);
    let new_w = (orig_w as f32 * gain).round() as u32;
    let new_h = (orig_h as f32 * gain).round() as u32;

    let resized =
        image::imageops::resize(&rgb, new_w, new_h, image::imageops::FilterType::Triangle);

    let pad_w = (INPUT_SIZE - new_w) as f32 / 2.0;
    let pad_h = (INPUT_SIZE - new_h) as f32 / 2.0;
    let pad_left = pad_w.round() as u32;
    let pad_top = pad_h.round() as u32;

    let s = INPUT_SIZE as usize;
    let mut data = vec![PAD_VALUE; 3 * s * s];

    for y in 0..new_h {
        for x in 0..new_w {
            let pixel = resized.get_pixel(x, y);
            let px = (x + pad_left) as usize;
            let py = (y + pad_top) as usize;
            data[py * s + px] = pixel[0] as f32 / 255.0;
            data[s * s + py * s + px] = pixel[1] as f32 / 255.0;
            data[2 * s * s + py * s + px] = pixel[2] as f32 / 255.0;
        }
    }

    PreprocessResult {
        data,
        gain,
        pad_w,
        pad_h,
    }
}

fn scale_box(
    x1: f32,
    y1: f32,
    x2: f32,
    y2: f32,
    gain: f32,
    pad_w: f32,
    pad_h: f32,
) -> (f64, f64, f64, f64) {
    let x1 = ((x1 - pad_w) / gain) as f64;
    let y1 = ((y1 - pad_h) / gain) as f64;
    let x2 = ((x2 - pad_w) / gain) as f64;
    let y2 = ((y2 - pad_h) / gain) as f64;
    (x1.max(0.0), y1.max(0.0), x2.max(0.0), y2.max(0.0))
}

fn parse_class_names(session: &Session) -> Vec<String> {
    let metadata = match session.metadata() {
        Ok(m) => m,
        Err(_) => return DEFAULT_CLASS_NAMES.iter().map(|s| s.to_string()).collect(),
    };
    let names_str: String = match metadata.custom("names") {
        Some(s) => s,
        None => return DEFAULT_CLASS_NAMES.iter().map(|s| s.to_string()).collect(),
    };

    let cleaned = names_str
        .trim_matches(|c: char| c == '{' || c == '}')
        .replace('\'', "");
    let mut result: Vec<(usize, String)> = Vec::new();
    for pair in cleaned.split(',') {
        let kv: Vec<&str> = pair.splitn(2, ':').collect();
        if kv.len() == 2 {
            if let Ok(id) = kv[0].trim().parse::<usize>() {
                result.push((id, kv[1].trim().to_string()));
            }
        }
    }
    if result.is_empty() {
        return DEFAULT_CLASS_NAMES.iter().map(|s| s.to_string()).collect();
    }
    result.sort_by_key(|(id, _)| *id);
    result.into_iter().map(|(_, name)| name).collect()
}

pub fn run_onnx_inference(
    model_path: &Path,
    image_path: &Path,
    page_index: usize,
    page_size: PageSize,
    conf_threshold: f64,
) -> Result<SegmentPage> {
    tracing::info!("Loading ONNX model from {:?}", model_path);
    let mut session = Session::builder()
        .map_err(|e| anyhow::anyhow!("Failed to create ORT session builder: {e}"))?
        .with_intra_threads(4)
        .map_err(|e| anyhow::anyhow!("Failed to set intra threads: {e}"))?
        .commit_from_file(model_path)
        .map_err(|e| anyhow::anyhow!("Failed to load ONNX model {model_path:?}: {e}"))?;

    let class_names = parse_class_names(&session);
    tracing::info!("Class names: {:?}", class_names);

    let img = image::open(image_path)
        .with_context(|| format!("Failed to open image: {image_path:?}"))?;

    let preprocess = preprocess_image(&img);

    let s = INPUT_SIZE as i64;
    let input_tensor = Tensor::from_array(([1i64, 3, s, s], preprocess.data))
        .map_err(|e| anyhow::anyhow!("Failed to create input tensor: {e}"))?;

    tracing::info!("Running inference...");
    let outputs = session
        .run(ort::inputs![input_tensor])
        .map_err(|e| anyhow::anyhow!("ONNX inference failed: {e}"))?;

    let (shape, output_data) = outputs[0]
        .try_extract_tensor::<f32>()
        .map_err(|e| anyhow::anyhow!("Failed to extract output tensor: {e}"))?;

    let dims: Vec<i64> = shape.iter().copied().collect();
    tracing::info!("Output shape: {:?}", dims);

    let mut blocks = Vec::new();
    let conf_thresh = conf_threshold as f32;

    // Output shape: [batch, num_detections, 6]
    if dims.len() == 3 {
        let num_dets = dims[1] as usize;
        let det_size = dims[2] as usize;
        for det_idx in 0..num_dets {
            if det_size < 6 {
                continue;
            }
            let base = det_idx * det_size;
            let conf = output_data[base + 4];
            if conf < conf_thresh {
                continue;
            }

            let x1 = output_data[base];
            let y1 = output_data[base + 1];
            let x2 = output_data[base + 2];
            let y2 = output_data[base + 3];
            let class_id = output_data[base + 5] as usize;

            let (bx1, by1, bx2, by2) =
                scale_box(x1, y1, x2, y2, preprocess.gain, preprocess.pad_w, preprocess.pad_h);

            let area = (bx2 - bx1) * (by2 - by1);
            if area <= 0.01 {
                continue;
            }

            let label: &str = class_names
                .get(class_id)
                .map(String::as_str)
                .unwrap_or("unknown");
            let segment_type = map_doclayout_label(label);

            let block = SegmentBlock {
                id: Some(format!("p{page_index:03}_b{det_idx:03}")),
                block_type: segment_type.to_string(),
                bbox: (bx1, by1, bx2, by2),
                meta: Some(TextBlockMeta {
                    doclayout_label: Some(label.to_string()),
                    confidence: Some(conf as f64),
                    ..Default::default()
                }),
            };
            blocks.push(block);
        }
    }

    tracing::info!(
        "Page {}: detected {} layout elements",
        page_index,
        blocks.len()
    );

    Ok(SegmentPage {
        page: page_index,
        size: page_size,
        blocks,
        png_overlay: None,
        json: None,
        doclayout_model: Some(model_path.to_string_lossy().to_string()),
        doclayout_confidence: Some(conf_threshold),
        doclayout_iou: None,
        dpi: None,
        granularity: Some("block".to_string()),
        math_threshold: None,
    })
}

fn fallback_segment_page(page_index: usize, page_size: PageSize) -> SegmentPage {
    let block = SegmentBlock {
        id: Some(format!("p{page_index:03}_b000")),
        block_type: "text".to_string(),
        bbox: (0.0, 0.0, page_size.width, page_size.height),
        meta: Some(TextBlockMeta {
            doclayout_label: Some("text".to_string()),
            confidence: Some(1.0),
            ..Default::default()
        }),
    };
    SegmentPage {
        page: page_index,
        size: page_size,
        blocks: vec![block],
        png_overlay: None,
        json: None,
        doclayout_model: Some("fallback:text-full-page".to_string()),
        doclayout_confidence: Some(0.25),
        doclayout_iou: None,
        dpi: None,
        granularity: Some("block".to_string()),
        math_threshold: None,
    }
}

pub fn segment_pdf(
    pdf_path: &Path,
    outdir: &Path,
    dpi: u32,
    conf_threshold: f64,
    model_path_override: Option<&Path>,
) -> Result<Vec<SegmentPage>> {
    std::fs::create_dir_all(outdir)
        .with_context(|| format!("Failed to create output directory: {outdir:?}"))?;

    let model_source = model::resolve_model(model_path_override);
    let model_desc = model_source.description();

    let doc = mupdf::Document::open(pdf_path.to_str().unwrap())
        .with_context(|| format!("Failed to open PDF: {pdf_path:?}"))?;

    let page_count = doc.page_count().context("Failed to get page count")?;
    let zoom = dpi as f32 / 72.0;
    let matrix = Matrix::new_scale(zoom, zoom);

    let mut results = Vec::with_capacity(page_count as usize);

    for page_idx in 0..page_count {
        let page_number = (page_idx + 1) as usize;
        let page = doc
            .load_page(page_idx)
            .with_context(|| format!("Failed to load page {page_number}"))?;

        let bounds = page.bounds().context("Failed to get page bounds")?;
        let page_size = PageSize {
            width: bounds.x1 as f64 - bounds.x0 as f64,
            height: bounds.y1 as f64 - bounds.y0 as f64,
        };

        let pixmap = page
            .to_pixmap(&matrix, &Colorspace::device_rgb(), false, false)
            .with_context(|| format!("Failed to render page {page_number}"))?;

        let png_path = outdir.join(format!("page_{page_number:03}.png"));
        pixmap
            .save_as(png_path.to_str().unwrap(), mupdf::ImageFormat::PNG)
            .with_context(|| format!("Failed to save page PNG: {png_path:?}"))?;

        let mut seg_page = match &model_source {
            ModelSource::Fallback => {
                tracing::warn!("Page {}: using fallback (no model)", page_number);
                fallback_segment_page(page_number, page_size)
            }
            ModelSource::Local(mp) | ModelSource::HuggingFace(mp) => {
                match run_onnx_inference(mp, &png_path, page_number, page_size.clone(), conf_threshold)
                {
                    Ok(sp) => sp,
                    Err(e) => {
                        tracing::warn!(
                            "Page {}: inference failed, using fallback: {e}",
                            page_number
                        );
                        fallback_segment_page(page_number, page_size)
                    }
                }
            }
        };

        // Save overlay PNG
        let overlay_path = outdir.join(format!("page_{page_number:03}_seg.png"));
        seg_page.png_overlay = Some(overlay_path.to_string_lossy().to_string());

        // Save JSON
        let json_path = outdir.join(format!("page_{page_number:03}.json"));
        seg_page.json = Some(json_path.to_string_lossy().to_string());
        seg_page.dpi = Some(dpi);
        seg_page.doclayout_model = Some(model_desc.clone());

        let json = serde_json::to_string_pretty(&seg_page)?;
        std::fs::write(&json_path, &json)
            .with_context(|| format!("Failed to write JSON: {json_path:?}"))?;

        tracing::info!(
            "Page {}/{}: {} blocks",
            page_number,
            page_count,
            seg_page.blocks.len()
        );
        results.push(seg_page);
    }

    Ok(results)
}

pub fn extract_text_metadata(
    pdf_path: &Path,
    page_index: usize,
    seg_pages: &mut [SegmentPage],
) -> Result<()> {
    let doc = mupdf::Document::open(pdf_path.to_str().unwrap())
        .with_context(|| format!("Failed to open PDF: {pdf_path:?}"))?;

    for seg_page in seg_pages.iter_mut() {
        if seg_page.page != page_index + 1 {
            continue;
        }
        let page = doc
            .load_page(page_index as i32)
            .context("Failed to load page")?;

        for block in seg_page.blocks.iter_mut() {
            if block.block_type != "text" && block.block_type != "caption" {
                continue;
            }
            let (x0, y0, x1, y1) = block.bbox;
            let rect = mupdf::Rect::new(x0 as f32, y0 as f32, x1 as f32, y1 as f32);
            let words = match page.words(mupdf::TextExtractOptions::default()) {
                Ok(w) => w,
                Err(_) => continue,
            };

            let mut text_parts = Vec::new();
            let mut font_sizes = Vec::new();
            for word in &words {
                let wr = word.bounds;
                if rects_overlap(&rect, &wr) {
                    text_parts.push(word.text.clone());
                    let h = (wr.y1 - wr.y0) as f64;
                    if h > 0.0 {
                        font_sizes.push(h);
                    }
                }
            }

            let text = text_parts.join(" ");
            if text.is_empty() {
                continue;
            }

            let meta = block.meta.get_or_insert_with(TextBlockMeta::default);
            let preview = if text.len() > 200 {
                text[..200].to_string()
            } else {
                text.clone()
            };
            meta.char_count = Some(text.len());
            meta.text_preview = Some(preview);
            meta.text = Some(text);
            if !font_sizes.is_empty() {
                meta.avg_font_size =
                    Some(font_sizes.iter().sum::<f64>() / font_sizes.len() as f64);
            }
        }
    }
    Ok(())
}

fn rects_overlap(a: &mupdf::Rect, b: &mupdf::Rect) -> bool {
    a.x0 < b.x1 && a.x1 > b.x0 && a.y0 < b.y1 && a.y1 > b.y0
}
