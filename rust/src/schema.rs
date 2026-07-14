use serde::{Deserialize, Serialize};

pub type NumericBBox = (f64, f64, f64, f64);
pub type FontCount = (String, usize);

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct InlineMath {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub placeholder: String,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub bbox: NumericBBox,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub baseline: Option<f64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fonts: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub font_size: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line_index: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageSize {
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TextBlockMeta {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text_preview: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub char_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avg_font_size: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub italic_ratio: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub math_font_ratio: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fonts_top: Option<Vec<FontCount>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub span_fonts: Option<Vec<FontCount>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub doclayout_label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub translated_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inline_math: Option<Vec<InlineMath>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inline_math_status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub translation_warnings: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SegmentBlock {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(rename = "type")]
    pub block_type: String,
    pub bbox: NumericBBox,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub meta: Option<TextBlockMeta>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SegmentPage {
    pub page: usize,
    pub size: PageSize,
    pub blocks: Vec<SegmentBlock>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub png_overlay: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub json: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub granularity: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub math_threshold: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub doclayout_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub doclayout_confidence: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub doclayout_iou: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dpi: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranslationSegment {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(rename = "type")]
    pub seg_type: String,
    pub bbox: NumericBBox,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub char_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avg_font_size: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub translated_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inline_math: Option<Vec<InlineMath>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inline_math_status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub translation_warnings: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranslatedPage {
    pub page: usize,
    pub segments: Vec<TranslationSegment>,
}
