from __future__ import annotations

import json
from collections.abc import Callable, Mapping, Sequence
from dataclasses import dataclass
from pathlib import Path
from typing import cast

import fitz

from common.schema import TranslatedPage


@dataclass
class ComposeOptions:
    font_path: str | None = None
    font_name: str = "Times-Roman"
    font_size_scale: float = 0.95
    min_font_size: float = 5.5
    max_font_size: float = 28.0
    line_spacing: float = 1.05
    cover_original: bool = True
    cover_padding: float = 1.2
    text_color: tuple[float, float, float] = (0.0, 0.0, 0.0)
    background_color: tuple[float, float, float] = (1.0, 1.0, 1.0)
    target_types: Sequence[str] | None = None
    adaptive_length: bool = True
    length_ratio_power: float = 0.7
    length_ratio_cap: float = 4.0
    max_logged_warnings: int = 50


@dataclass
class ComposeResult:
    output_path: Path
    page_count: int
    segment_count: int
    warnings: list[str]
    warning_count: int


def compose_pdf(
    original_pdf: str | Path,
    translated_pages: Sequence[TranslatedPage] | Path | str,
    output_pdf: str | Path,
    options: ComposeOptions | None = None,
) -> ComposeResult:
    opts = options or ComposeOptions()
    original_path = Path(original_pdf)
    output_path = Path(output_pdf)
    pages = _load_translated_pages(translated_pages)
    if not pages:
        raise ValueError("翻訳済みセグメントが空です")
    if not original_path.exists():
        raise FileNotFoundError(f"原稿PDFが見つかりません: {original_path}")

    output_path.parent.mkdir(parents=True, exist_ok=True)
    font = _resolve_font(opts)
    warnings: list[str] = []
    warning_count = 0

    def record_warning(message: str) -> None:
        nonlocal warning_count
        warning_count += 1
        if len(warnings) < opts.max_logged_warnings:
            warnings.append(message)

    segment_count = 0

    with fitz.open(original_path) as src_doc:
        out_doc = fitz.open()
        try:
            for entry in pages:
                page_number = int(entry.get("page", 0))
                if page_number <= 0 or page_number > len(src_doc):
                    record_warning(f"ページ番号が不正のためスキップ: {entry!r}")
                    continue
                src_index = page_number - 1
                out_doc.insert_pdf(src_doc, from_page=src_index, to_page=src_index)
                page = out_doc[-1]
                page_rect = page.rect
                segments = entry.get("segments") or []
                src_page = src_doc[src_index]
                if opts.cover_original:
                    _strip_page_text(page, opts, record_warning)
                for segment in segments:
                    seg_type = segment.get("type")
                    if opts.target_types and seg_type not in opts.target_types:
                        continue
                    bbox = segment.get("bbox")
                    if not bbox or len(bbox) != 4:
                        record_warning(
                            f"bboxが不正のためセグメントをスキップ (page={page_number}, id={segment.get('id')})"
                        )
                        continue
                    rect = fitz.Rect(bbox)
                    rect = rect & page_rect
                    if rect.is_empty:
                        record_warning(
                            f"bboxがページ外のためセグメントをスキップ (page={page_number}, id={segment.get('id')})"
                        )
                        continue

                    if seg_type in {"image", "table", "math"}:
                        placed_raster = _place_region_snapshot(
                            page,
                            src_page,
                            rect,
                            record_warning,
                            segment.get("id"),
                            page_number,
                            seg_type,
                        )
                        if placed_raster:
                            segment_count += 1
                        continue
                    text = (segment.get("translated_text") or "").strip()
                    if not text:
                        continue
                    font_size = _determine_font_size(segment, opts)

                    placed = False
                    attempt_size = font_size
                    last_warning = None
                    for _ in range(12):
                        temp_writer = fitz.TextWriter(page_rect)
                        try:
                            leftovers = temp_writer.fill_textbox(
                                rect,
                                text,
                                font=font,
                                fontsize=attempt_size,
                                lineheight=opts.line_spacing,
                                align=0,
                                warn=False,
                            )
                        except ValueError as exc:
                            leftovers = [("error", 0.0)]
                            last_warning = str(exc)

                        if leftovers:
                            if attempt_size <= opts.min_font_size + 0.1:
                                temp_writer.write_text(page, color=opts.text_color, overlay=True)
                                detail = f" ({last_warning})" if last_warning else ""
                                record_warning(
                                    f"テキストが収まりきりませんでした (page={page_number}, id={segment.get('id')}){detail}"
                                )
                                placed = True
                                break
                            new_size = max(opts.min_font_size, attempt_size * 0.9)
                            if abs(new_size - attempt_size) < 0.1:
                                attempt_size = opts.min_font_size
                            else:
                                attempt_size = new_size
                            continue

                        temp_writer.write_text(page, color=opts.text_color, overlay=True)
                        placed = True
                        break

                    if placed:
                        segment_count += 1
                    else:
                        record_warning(
                            f"テキスト配置に失敗しました (page={page_number}, id={segment.get('id')})"
                        )
        finally:
            out_doc.save(output_path)
            out_doc.close()

    return ComposeResult(
        output_path=output_path,
        page_count=len(pages),
        segment_count=segment_count,
        warnings=warnings,
        warning_count=warning_count,
    )


def _strip_page_text(page: fitz.Page, opts: ComposeOptions, record_warning) -> None:
    """Remove original text content from the page via redactions."""
    try:
        raw = page.get_text("rawdict")
    except RuntimeError as exc:
        record_warning(f"原文テキスト抽出に失敗しました (page={page.number + 1}): {exc}")
        return

    if not raw:
        return

    page_rect = page.rect
    padding = max(opts.cover_padding, 0.0)
    fill_color = opts.background_color if opts.background_color is not None else (1.0, 1.0, 1.0)
    redactions = []

    for block in raw.get("blocks", []):
        if block.get("type") != 0:
            continue
        for line in block.get("lines", []):
            rect = _rect_from_spans(line.get("spans") or [])
            if rect is None:
                continue
            rect = rect & page_rect
            if rect.is_empty:
                continue
            if padding:
                rect = _expand_rect(rect, padding, page_rect)
            annot = page.add_redact_annot(rect, fill=fill_color)
            redactions.append(annot)

    if not redactions:
        return

    for annot in redactions:
        annot.update()

    try:
        page.apply_redactions(images=0, graphics=0, text=0)
    except RuntimeError as exc:
        record_warning(f"原文テキスト除去に失敗しました (page={page.number + 1}): {exc}")


def _place_region_snapshot(
    dest_page: fitz.Page,
    src_page: fitz.Page,
    rect: fitz.Rect,
    record_warning: Callable[[str], None],
    segment_id: object,
    page_number: int,
    label: str,
    zoom: float = 2.0,
) -> bool:
    clip = rect & src_page.rect
    if clip.is_empty:
        seg_label = f" (id={segment_id})" if segment_id else ""
        record_warning(
            f"{label}領域の切り出し範囲が空のためスキップ (page={page_number}{seg_label})"
        )
        return False
    try:
        pix = src_page.get_pixmap(matrix=fitz.Matrix(zoom, zoom), clip=clip, alpha=False)
    except RuntimeError as exc:
        seg_label = f" (id={segment_id})" if segment_id else ""
        record_warning(
            f"{label}領域の切り出しに失敗したためスキップ (page={page_number}{seg_label}): {exc}"
        )
        return False
    dest_page.insert_image(rect, pixmap=pix, overlay=True)
    return True


def _load_translated_pages(
    translated: Sequence[TranslatedPage] | Path | str,
) -> list[TranslatedPage]:
    if isinstance(translated, (str, Path)):
        path = Path(translated)
        if path.is_dir():
            path = path / "document_translation.json"
        if not path.exists():
            raise FileNotFoundError(f"翻訳JSONが見つかりません: {path}")
        with path.open("r", encoding="utf-8") as fh:
            data = json.load(fh)
        if not isinstance(data, list):
            raise ValueError("翻訳JSONがリスト形式ではありません")
        return [cast(TranslatedPage, dict(page)) for page in data]
    return [cast(TranslatedPage, dict(page)) for page in translated]  # shallow copy


def _resolve_font(opts: ComposeOptions) -> fitz.Font:
    if opts.font_path:
        font_path = Path(opts.font_path)
        if not font_path.exists():
            raise FileNotFoundError(f"フォントファイルが見つかりません: {font_path}")
        return fitz.Font(fontfile=str(font_path))
    return fitz.Font(opts.font_name)


def _determine_font_size(segment: Mapping[str, object], opts: ComposeOptions) -> float:
    base = segment.get("avg_font_size")
    if isinstance(base, (int, float)) and base > 0:
        size = float(base) * opts.font_size_scale
    else:
        size = opts.min_font_size

    if opts.adaptive_length:
        src_chars = segment.get("char_count")
        if not isinstance(src_chars, (int, float)) or src_chars <= 0:
            src_chars = len(segment.get("source_text") or "")
        tgt_len = len(segment.get("translated_text") or "")
        if src_chars:
            ratio = tgt_len / max(float(src_chars), 1.0)
            if ratio > 1.0:
                shrink = ratio**opts.length_ratio_power
                if opts.length_ratio_cap:
                    shrink = min(shrink, opts.length_ratio_cap)
                size /= max(shrink, 1.0)

    return max(opts.min_font_size, min(opts.max_font_size, size))


def _rect_from_spans(spans: Sequence[Mapping[str, object]]) -> fitz.Rect | None:
    if not spans:
        return None
    x0 = float("inf")
    y0 = float("inf")
    x1 = float("-inf")
    y1 = float("-inf")
    for span in spans:
        bbox = span.get("bbox")
        if not bbox:
            continue
        rect = fitz.Rect(bbox)
        x0 = min(x0, rect.x0)
        y0 = min(y0, rect.y0)
        x1 = max(x1, rect.x1)
        y1 = max(y1, rect.y1)
    if x0 == float("inf") or y0 == float("inf"):
        return None
    return fitz.Rect(x0, y0, x1, y1)


def _expand_rect(rect: fitz.Rect, padding: float, bounds: fitz.Rect) -> fitz.Rect:
    if padding <= 0:
        return rect
    expanded = fitz.Rect(
        rect.x0 - padding,
        rect.y0 - padding,
        rect.x1 + padding,
        rect.y1 + padding,
    )
    expanded.x0 = max(bounds.x0, expanded.x0)
    expanded.y0 = max(bounds.y0, expanded.y0)
    expanded.x1 = min(bounds.x1, expanded.x1)
    expanded.y1 = min(bounds.y1, expanded.y1)
    return expanded
