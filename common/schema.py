from __future__ import annotations

from collections.abc import Sequence
from typing import Any, Literal, NotRequired, TypedDict

SegmentType = Literal["text", "image", "table", "caption", "math", "merged"]
NumericBBox = tuple[float, float, float, float]
FontCount = tuple[str, int]


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
    translated_text: str


class TranslatedPage(TypedDict, total=False):
    page: int
    segments: Sequence[TranslationSegment]


__all__ = [
    "FontCount",
    "ImageBlockMeta",
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
