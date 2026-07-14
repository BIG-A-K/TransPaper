use anyhow::{Context, Result};
use mupdf::{Colorspace, Matrix};
use ort::session::Session;
use ort::value::Tensor;
use std::collections::{BTreeSet, HashMap};
use std::path::Path;

use crate::model::{self, ModelSource};
use crate::schema::{InlineMath, PageSize, SegmentBlock, SegmentPage, TextBlockMeta};

const INPUT_SIZE: u32 = 1024;
const PAD_VALUE: f32 = 114.0 / 255.0;
const INLINE_MATH_PLACEHOLDER_PREFIX: &str = "[[TRANSPAPER_INLINE_MATH_";

#[derive(Debug, Clone)]
struct TextRun {
    text: String,
    bbox: mupdf::Rect,
    font: String,
    size: f32,
    baseline: f32,
}

fn is_strong_math_font(font: &str) -> bool {
    let normalized = font.to_ascii_lowercase().replace(['-', '_', ' '], "");
    [
        "cmmi",
        "cmsy",
        "cmex",
        "msam",
        "msbm",
        "mathjax",
        "latinmodernmath",
        "asanamath",
        "xitsmath",
        "mtextra",
    ]
    .iter()
    .any(|needle| normalized.contains(needle))
        || (normalized.contains("stix") && normalized.contains("math"))
        || (normalized.contains("texgyre") && normalized.contains("math"))
}

fn is_math_symbol(ch: char) -> bool {
    matches!(
        ch as u32,
        0x00B1 | 0x00D7 | 0x00F7 | 0x2102 | 0x2113 | 0x2115 | 0x211A | 0x211D
            | 0x2124
            | 0x0370..=0x03FF
            | 0x1D6A8..=0x1D7CB
            | 0x2190..=0x22FF
            | 0x2308..=0x230B
            | 0x27E6..=0x27EF
    )
}

fn is_formula_connector(ch: char) -> bool {
    matches!(
        ch,
        '+' | '-'
            | '='
            | '*'
            | '/'
            | '<'
            | '>'
            | '^'
            | '_'
            | '|'
            | '('
            | ')'
            | '['
            | ']'
            | '{'
            | '}'
            | '.'
            | ','
            | ':'
            | '\''
            | '\u{2032}'
            | '\u{2033}'
    )
}

fn is_formula_context_run(run: &TextRun) -> bool {
    let text = run.text.trim();
    if text.is_empty() || text != run.text || text.chars().count() > 8 {
        return false;
    }
    if !text
        .chars()
        .all(|ch| ch.is_alphanumeric() || is_math_symbol(ch) || is_formula_connector(ch))
    {
        return false;
    }
    text.chars()
        .filter(|ch| ch.is_alphabetic() && !is_math_symbol(*ch))
        .count()
        <= 2
}

fn is_strong_math_run(run: &TextRun) -> bool {
    if run.text.trim().is_empty() {
        return false;
    }
    is_strong_math_font(&run.font)
        || (is_formula_context_run(run) && run.text.chars().any(is_math_symbol))
}

fn runs_are_close(left: &TextRun, right: &TextRun) -> bool {
    let gap = right.bbox.x0 - left.bbox.x1;
    gap <= left.size.max(right.size).max(1.0) * 0.45
}

fn inline_math_groups(runs: &[TextRun]) -> Vec<(usize, usize)> {
    let strong: Vec<usize> = runs
        .iter()
        .enumerate()
        .filter_map(|(index, run)| is_strong_math_run(run).then_some(index))
        .collect();
    if strong.is_empty() {
        return Vec::new();
    }

    let mut selected: BTreeSet<usize> = strong.iter().copied().collect();
    for index in strong {
        let mut cursor = index;
        while cursor > 0 {
            let previous = cursor - 1;
            if !is_formula_context_run(&runs[previous])
                || !runs_are_close(&runs[previous], &runs[cursor])
            {
                break;
            }
            selected.insert(previous);
            cursor = previous;
        }

        let mut cursor = index + 1;
        while cursor < runs.len() {
            if !is_formula_context_run(&runs[cursor])
                || !runs_are_close(&runs[cursor - 1], &runs[cursor])
            {
                break;
            }
            selected.insert(cursor);
            cursor += 1;
        }
    }

    let mut groups = Vec::new();
    for index in selected {
        match groups.last_mut() {
            Some((_, end)) if index == *end + 1 => *end = index,
            _ => groups.push((index, index)),
        }
    }
    groups
}

fn union_rect(left: mupdf::Rect, right: mupdf::Rect) -> mupdf::Rect {
    mupdf::Rect::new(
        left.x0.min(right.x0),
        left.y0.min(right.y0),
        left.x1.max(right.x1),
        left.y1.max(right.y1),
    )
}

fn normalize_extracted_text(text: &str) -> String {
    text.replace("-\n", "")
        .replace('\n', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn extract_text_metadata_from_page(
    text_page: &mupdf::TextPage,
    clip: &mupdf::Rect,
) -> Option<TextBlockMeta> {
    let mut text_parts = Vec::new();
    let mut inline_math = Vec::new();
    let mut all_fonts = Vec::new();
    let mut all_sizes = Vec::new();
    let mut math_index = 1usize;
    let mut line_index = 0usize;

    for block in text_page.blocks() {
        if !rects_overlap(clip, &block.bounds()) {
            continue;
        }
        for line in block.lines() {
            if !rects_overlap(clip, &line.bounds()) {
                continue;
            }
            let mut runs: Vec<TextRun> = Vec::new();
            for ch in line.chars() {
                let Some(value) = ch.char() else {
                    continue;
                };
                let bbox: mupdf::Rect = ch.quad().into();
                if !rects_overlap(clip, &bbox) {
                    continue;
                }
                let font = ch
                    .font()
                    .map(|font| font.name().to_string())
                    .unwrap_or_default();
                let size = ch.size();
                let baseline = ch.origin().y;
                let can_merge = runs.last().is_some_and(|run| {
                    run.font == font
                        && (run.size - size).abs() <= 0.1
                        && (run.baseline - baseline).abs() <= size.max(1.0) * 0.15
                });
                if can_merge {
                    let run = runs.last_mut().expect("run exists");
                    run.text.push(value);
                    run.bbox = union_rect(run.bbox, bbox);
                    run.baseline = (run.baseline + baseline) / 2.0;
                } else {
                    runs.push(TextRun {
                        text: value.to_string(),
                        bbox,
                        font: font.clone(),
                        size,
                        baseline,
                    });
                }
                all_fonts.push(font);
                all_sizes.push(size as f64);
            }
            if runs.is_empty() {
                continue;
            }

            let groups = inline_math_groups(&runs);
            let group_by_start: HashMap<usize, usize> = groups.into_iter().collect();
            let original_line = runs.iter().map(|run| run.text.as_str()).collect::<String>();
            let mut line_text = String::new();
            let mut run_index = 0usize;
            while run_index < runs.len() {
                let Some(group_end) = group_by_start.get(&run_index).copied() else {
                    line_text.push_str(&runs[run_index].text);
                    run_index += 1;
                    continue;
                };
                let group = &runs[run_index..=group_end];
                let mut placeholder = format!("{INLINE_MATH_PLACEHOLDER_PREFIX}{math_index:04}]]");
                while original_line.contains(&placeholder) {
                    math_index += 1;
                    placeholder = format!("{INLINE_MATH_PLACEHOLDER_PREFIX}{math_index:04}]]");
                }
                let math_bbox = group
                    .iter()
                    .skip(1)
                    .fold(group[0].bbox, |bbox, run| union_rect(bbox, run.bbox));
                let mut fonts = Vec::new();
                for run in group {
                    if !run.font.is_empty() && !fonts.contains(&run.font) {
                        fonts.push(run.font.clone());
                    }
                }
                inline_math.push(InlineMath {
                    id: format!("m{math_index:04}"),
                    placeholder: placeholder.clone(),
                    text: group.iter().map(|run| run.text.as_str()).collect(),
                    bbox: (
                        math_bbox.x0 as f64,
                        math_bbox.y0 as f64,
                        math_bbox.x1 as f64,
                        math_bbox.y1 as f64,
                    ),
                    baseline: Some(
                        group.iter().map(|run| run.baseline as f64).sum::<f64>()
                            / group.len() as f64,
                    ),
                    fonts,
                    font_size: Some(
                        group.iter().map(|run| run.size as f64).sum::<f64>() / group.len() as f64,
                    ),
                    line_index: Some(line_index),
                });
                line_text.push_str(&placeholder);
                math_index += 1;
                run_index = group_end + 1;
            }
            text_parts.push(line_text);
            line_index += 1;
        }
    }

    let text = normalize_extracted_text(&text_parts.join("\n"));
    if text.is_empty() {
        return None;
    }
    let mut display_text = text.clone();
    for math in &inline_math {
        display_text = display_text.replace(&math.placeholder, &math.text);
    }
    let mut font_counts: HashMap<String, usize> = HashMap::new();
    for font in all_fonts.into_iter().filter(|font| !font.is_empty()) {
        *font_counts.entry(font).or_default() += 1;
    }
    let mut fonts_top: Vec<_> = font_counts.into_iter().collect();
    fonts_top.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    fonts_top.truncate(3);

    Some(TextBlockMeta {
        text_preview: Some(text.chars().take(200).collect()),
        char_count: Some(display_text.chars().count()),
        avg_font_size: (!all_sizes.is_empty())
            .then(|| all_sizes.iter().sum::<f64>() / all_sizes.len() as f64),
        fonts_top: (!fonts_top.is_empty()).then_some(fonts_top),
        inline_math_status: (!inline_math.is_empty()).then(|| "protected".to_string()),
        inline_math: (!inline_math.is_empty()).then_some(inline_math),
        text: Some(text),
        ..Default::default()
    })
}

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
    if DOC_LAYOUT_CAPTION_CLASSES.contains(&n) {
        return "caption";
    }
    if DOC_LAYOUT_TABLE_CLASSES.contains(&n) {
        return "table";
    }
    if DOC_LAYOUT_IMAGE_CLASSES.contains(&n) {
        return "image";
    }
    if DOC_LAYOUT_MATH_CLASSES.contains(&n) {
        return "math";
    }
    if DOC_LAYOUT_TEXT_CLASSES.contains(&n) {
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

pub fn create_session(model_path: &Path) -> Result<Session> {
    tracing::info!("Loading ONNX model from {:?}", model_path);
    let session = Session::builder()
        .map_err(|e| anyhow::anyhow!("Failed to create ORT session builder: {e}"))?
        .with_intra_threads(4)
        .map_err(|e| anyhow::anyhow!("Failed to set intra threads: {e}"))?
        .commit_from_file(model_path)
        .map_err(|e| anyhow::anyhow!("Failed to load ONNX model {model_path:?}: {e}"))?;
    Ok(session)
}

pub fn run_onnx_inference(
    session: &mut Session,
    image_path: &Path,
    page_index: usize,
    page_size: PageSize,
    conf_threshold: f64,
) -> Result<SegmentPage> {
    let class_names = parse_class_names(session);
    tracing::info!("Class names: {:?}", class_names);

    let img =
        image::open(image_path).with_context(|| format!("Failed to open image: {image_path:?}"))?;

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

            let (bx1, by1, bx2, by2) = scale_box(
                x1,
                y1,
                x2,
                y2,
                preprocess.gain,
                preprocess.pad_w,
                preprocess.pad_h,
            );

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
        doclayout_model: None,
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

    let mut ort_session = match &model_source {
        ModelSource::Local(mp) | ModelSource::HuggingFace(mp) => Some(create_session(mp)?),
        ModelSource::Fallback => None,
    };

    let doc = mupdf::Document::open(pdf_path.to_str().unwrap())
        .with_context(|| format!("Failed to open PDF: {pdf_path:?}"))?;

    let page_count = doc.page_count().context("Failed to get page count")?;
    let zoom = dpi as f32 / 72.0;
    let matrix = Matrix::new_scale(zoom, zoom);

    let mut results = Vec::with_capacity(page_count as usize);

    let pb = indicatif::ProgressBar::new(page_count as u64);
    pb.set_style(
        indicatif::ProgressStyle::with_template("  Segmenting [{bar:30}] {pos}/{len} pages")
            .unwrap()
            .progress_chars("█▓░"),
    );

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

        let mut seg_page = match &mut ort_session {
            None => {
                tracing::warn!("Page {}: using fallback (no model)", page_number);
                fallback_segment_page(page_number, page_size)
            }
            Some(session) => {
                match run_onnx_inference(
                    session,
                    &png_path,
                    page_number,
                    page_size.clone(),
                    conf_threshold,
                ) {
                    Ok(mut sp) => {
                        // Convert bboxes from image pixel space to PDF point space
                        let scale = 1.0 / zoom as f64;
                        for block in sp.blocks.iter_mut() {
                            let (x0, y0, x1, y1) = block.bbox;
                            block.bbox = (x0 * scale, y0 * scale, x1 * scale, y1 * scale);
                        }
                        sp
                    }
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

        // Save JSON
        let json_path = outdir.join(format!("page_{page_number:03}.json"));
        seg_page.json = Some(json_path.to_string_lossy().to_string());
        seg_page.dpi = Some(dpi);
        seg_page.doclayout_model = Some(model_desc.clone());

        let json = serde_json::to_string_pretty(&seg_page)?;
        std::fs::write(&json_path, &json)
            .with_context(|| format!("Failed to write JSON: {json_path:?}"))?;

        results.push(seg_page);
        pb.inc(1);
    }

    pb.finish_and_clear();
    Ok(results)
}

pub fn extract_text_metadata(
    doc: &mupdf::Document,
    page_index: usize,
    seg_pages: &mut [SegmentPage],
) -> Result<()> {
    for seg_page in seg_pages.iter_mut() {
        if seg_page.page != page_index + 1 {
            continue;
        }
        let page = doc
            .load_page(page_index as i32)
            .context("Failed to load page")?;

        let text_page = match page.to_text_page(
            mupdf::TextPageFlags::PRESERVE_SPANS | mupdf::TextPageFlags::PRESERVE_WHITESPACE,
        ) {
            Ok(text_page) => text_page,
            Err(_) => continue,
        };

        for block in seg_page.blocks.iter_mut() {
            if block.block_type != "text" && block.block_type != "caption" {
                continue;
            }
            let (x0, y0, x1, y1) = block.bbox;
            let rect = mupdf::Rect::new(x0 as f32, y0 as f32, x1 as f32, y1 as f32);

            if let Some(extracted) = extract_text_metadata_from_page(&text_page, &rect) {
                let meta = block.meta.get_or_insert_with(TextBlockMeta::default);
                let doclayout_label = meta.doclayout_label.clone();
                let confidence = meta.confidence;
                *meta = extracted;
                meta.doclayout_label = doclayout_label;
                meta.confidence = confidence;
            }
        }
    }
    Ok(())
}

fn rects_overlap(a: &mupdf::Rect, b: &mupdf::Rect) -> bool {
    a.x0 < b.x1 && a.x1 > b.x0 && a.y0 < b.y1 && a.y1 > b.y0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(text: &str, font: &str, x0: f32, x1: f32) -> TextRun {
        TextRun {
            text: text.to_string(),
            bbox: mupdf::Rect::new(x0, 10.0, x1, 22.0),
            font: font.to_string(),
            size: 10.0,
            baseline: 20.0,
        }
    }

    #[test]
    fn math_font_and_symbol_form_one_inline_group() {
        let runs = vec![
            run("The shape is ", "Times-Roman", 10.0, 70.0),
            run("H", "CMR10", 70.0, 77.0),
            run("×", "CMSY10", 77.0, 84.0),
            run("W", "CMMI10", 84.0, 92.0),
            run(" pixels.", "Times-Roman", 92.0, 130.0),
        ];

        assert_eq!(inline_math_groups(&runs), vec![(1, 3)]);
    }

    #[test]
    fn symbol_inside_long_normal_prose_is_not_math() {
        let prose = run(
            "The process A → B is described below.",
            "Times-Roman",
            10.0,
            190.0,
        );

        assert!(!is_strong_math_run(&prose));
        assert!(inline_math_groups(&[prose]).is_empty());
    }
}
