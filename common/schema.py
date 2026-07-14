from __future__ import annotations

from collections.abc import Sequence
from typing import Any, Literal, NotRequired, TypedDict

SegmentType = Literal["text", "image", "table", "caption", "math", "merged"]
NumericBBox = tuple[float, float, float, float]
FontCount = tuple[str, int]


class InlineMath(TypedDict, total=False):
    id: str
    placeholder: str
    text: str
    bbox: NumericBBox
    baseline: float
    fonts: Sequence[str]
    font_size: float
    line_index: int


class PageSize(TypedDict):
    width: float
    height: float


class TextBlockMeta(TypedDict, total=False):
    text: str
    text_preview: str
    char_count: int
    avg_font_size: float
    italic_ratio: float
    math_font_ratio: float
    fonts_top: Sequence[FontCount]
    span_fonts: Sequence[FontCount]
    inline_math: Sequence[InlineMath]
    inline_math_status: str
    translation_warnings: Sequence[str]


class ImageBlockMeta(TypedDict, total=False):
    source: str


SegmentBlockMeta = TextBlockMeta | ImageBlockMeta | dict[str, Any]


class SegmentBlock(TypedDict, total=False):
    id: str
    type: SegmentType
    bbox: NumericBBox
    meta: SegmentBlockMeta


class SegmentPage(TypedDict, total=False):
    page: int
    size: PageSize
    blocks: Sequence[SegmentBlock]
    png_overlay: str
    json: str
    granularity: str
    math_threshold: float
    doclayout_model: str
    doclayout_confidence: float
    doclayout_iou: float
    dpi: int


class TranslationSegment(TypedDict, total=False):
    id: NotRequired[str]
    type: SegmentType
    bbox: NumericBBox
    source_text: NotRequired[str]
    char_count: NotRequired[int]
    avg_font_size: NotRequired[float]
    inline_math: NotRequired[Sequence[InlineMath]]
    inline_math_status: NotRequired[str]
    translation_warnings: NotRequired[Sequence[str]]
    translated_text: str


class TranslatedPage(TypedDict, total=False):
    page: int
    segments: Sequence[TranslationSegment]


__all__ = [
    "FontCount",
    "ImageBlockMeta",
    "InlineMath",
    "NumericBBox",
    "PageSize",
    "SegmentBlock",
    "SegmentBlockMeta",
    "SegmentPage",
    "SegmentType",
    "TextBlockMeta",
    "TranslatedPage",
    "TranslationSegment",
]
