use anyhow::{Context, Result};
use ort::session::Session;
use ort::value::Tensor;
use std::path::Path;

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
