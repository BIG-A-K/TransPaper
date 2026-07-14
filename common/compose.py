from __future__ import annotations

import json
import re
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
    dedup_enabled: bool = True
    dedup_ios_threshold: float = 0.6
    max_logged_warnings: int = 50
    inline_math_zoom: float = 2.0


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
                if opts.dedup_enabled:
                    page_area = page_rect.width * page_rect.height
                    segments = _dedup_text_segments(
                        segments, opts, page_number, page_area, record_warning
                    )
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

                    inline_math = segment.get("inline_math") or []
                    if inline_math:
                        for warning in segment.get("translation_warnings") or []:
                            record_warning(
                                f"{warning} (page={page_number}, id={segment.get('id')})"
                            )
                        inline_placed = _place_inline_math_segment(
                            page=page,
                            src_page=src_page,
                            page_rect=page_rect,
                            rect=rect,
                            text=text,
                            inline_math=inline_math,
                            font=font,
                            font_size=font_size,
                            opts=opts,
                            record_warning=record_warning,
                            segment_id=segment.get("id"),
                            page_number=page_number,
                        )
                        if inline_placed is not None:
                            if inline_placed:
                                segment_count += 1
                            continue
                        for math in inline_math:
                            placeholder = str(math.get("placeholder") or "")
                            if placeholder:
                                text = text.replace(placeholder, str(math.get("text") or ""))

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


def _is_text_placement_target(segment: Mapping[str, object], opts: ComposeOptions) -> bool:
    """セグメントがテキスト配置対象か（重複間引きの参加条件）。"""
    seg_type = segment.get("type")
    if seg_type in {"image", "table", "math"}:
        return False
    if opts.target_types and seg_type not in opts.target_types:
        return False
    text = (segment.get("translated_text") or "").strip()
    return bool(text)


def _rect_intersection_area(a: fitz.Rect, b: fitz.Rect) -> float:
    inter = a & b
    if inter.is_empty:
        return 0.0
    return inter.width * inter.height


def _dedup_text_segments(
    segments: Sequence[Mapping[str, object]],
    opts: ComposeOptions,
    page_number: int,
    page_area: float,
    record_warning: Callable[[str], None],
) -> list[Mapping[str, object]]:
    """配置前のテキストセグメント重複を間引く（案A: 2パス方式）。

    IoS（共通面積 / 小さい方の面積）が opts.dedup_ios_threshold 以上のペアを重複とみなし、
    テキスト長が短い方を除外する。残す方が同じ長さの場合は登場順が早い方を残す。
    実際にテキスト配置されるセグメントのみが対象（image/table/math・空テキスト・
    target_types 外はそのまま残す）。

    巨大 bbox（ページ面積の40%超）は seg の異常検出とみなし処理順を後回しにする。
    これにより通常の重複では長い方を残し（issue #0002 の b016/b017 は同サイズ→長い方 b016）、
    巨大 bbox では先に accepted に入った個別セグメントを残して巨大 bbox を間引く
    （p11_b001 問題への対策）。
    """
    threshold = opts.dedup_ios_threshold
    if threshold <= 0.0 or len(segments) < 2:
        return list(segments)

    giant_area_threshold = page_area * 0.4  # ページ面積の40%超で巨大 bbox 扱い

    candidates: list[tuple[int, int, float]] = []  # (idx, text_len, area)
    for idx, seg in enumerate(segments):
        if not _is_text_placement_target(seg, opts):
            continue
        bbox = seg.get("bbox")
        if not bbox or len(bbox) != 4:
            continue
        text_len = len(seg.get("translated_text") or "")
        rect_tmp = fitz.Rect(bbox)
        area_tmp = rect_tmp.width * rect_tmp.height
        candidates.append((idx, text_len, area_tmp))

    # 通常セグメントと巨大セグメントで分ける
    normal = [c for c in candidates if c[2] <= giant_area_threshold]
    giant = [c for c in candidates if c[2] > giant_area_threshold]

    # 通常は「テキスト長降順（長い方優先）→ idx 昇順」で残す優先度を決定
    normal.sort(key=lambda t: (-t[1], t[0]))
    # 巨大は後回し（idx 昇順で安定）
    giant.sort(key=lambda t: t[0])

    ordered = normal + giant  # 通常を先に処理

    accepted: list[tuple[int, fitz.Rect, float]] = []
    dropped: list[tuple[int, int]] = []  # (dropped_idx, kept_idx)
    for idx, _text_len, area in ordered:
        rect = fitz.Rect(segments[idx]["bbox"])
        if area <= 0:
            continue
        kept_idx: int | None = None
        for acc_idx, acc_rect, acc_area in accepted:
            inter = _rect_intersection_area(rect, acc_rect)
            if inter <= 0:
                continue
            ios = inter / min(area, acc_area)
            if ios >= threshold:
                kept_idx = acc_idx
                break
        if kept_idx is None:
            accepted.append((idx, rect, area))
        else:
            dropped.append((idx, kept_idx))

    if not dropped:
        return list(segments)

    dropped_set = {d[0] for d in dropped}
    result = [seg for idx, seg in enumerate(segments) if idx not in dropped_set]
    for dropped_idx, kept_idx in dropped:
        record_warning(
            f"重複セグメントを間引きました (page={page_number}, "
            f"dropped_id={segments[dropped_idx].get('id')}, "
            f"kept_id={segments[kept_idx].get('id')}, IoS≥{threshold})"
        )
    return result


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


@dataclass
class _InlineLayoutItem:
    kind: str
    width: float
    height: float
    ascent: float
    descent: float
    text: str = ""
    math: Mapping[str, object] | None = None
    x: float = 0.0
    y: float = 0.0
    baseline: float = 0.0


def _split_inline_text(text: str) -> list[str]:
    return re.findall(r"\s+|[A-Za-z0-9]+(?:[./:_-][A-Za-z0-9]+)*|.", text, re.DOTALL)


def _math_dimensions(math: Mapping[str, object], font_size: float, max_width: float) -> tuple[float, float, float]:
    bbox = math.get("bbox")
    if bbox and len(bbox) == 4:
        original_width = max(float(bbox[2]) - float(bbox[0]), 0.1)
        original_height = max(float(bbox[3]) - float(bbox[1]), 0.1)
    else:
        original_width = font_size
        original_height = font_size
    source_font_size = math.get("font_size")
    if isinstance(source_font_size, (int, float)) and source_font_size > 0:
        height = original_height * font_size / float(source_font_size)
    else:
        height = font_size
    height = max(font_size * 0.75, min(font_size * 1.8, height))
    width = height * original_width / original_height
    if width > max_width:
        scale = max_width / width
        width *= scale
        height *= scale
    baseline = math.get("baseline")
    if isinstance(baseline, (int, float)) and bbox and len(bbox) == 4:
        baseline_ratio = (float(baseline) - float(bbox[1])) / original_height
    else:
        baseline_ratio = 0.8
    return width, height, max(0.55, min(0.95, baseline_ratio))


def _make_inline_items(
    text: str,
    inline_math: Sequence[Mapping[str, object]],
    font: fitz.Font,
    font_size: float,
    max_width: float,
) -> list[_InlineLayoutItem] | None:
    math_by_placeholder = {
        str(item.get("placeholder")): item
        for item in inline_math
        if item.get("placeholder")
    }
    if not math_by_placeholder or any(
        text.count(placeholder) != 1 for placeholder in math_by_placeholder
    ):
        return None
    pattern = "(" + "|".join(
        re.escape(placeholder)
        for placeholder in sorted(math_by_placeholder, key=len, reverse=True)
    ) + ")"
    parts = re.split(pattern, text)
    items: list[_InlineLayoutItem] = []
    for part in parts:
        if not part:
            continue
        math = math_by_placeholder.get(part)
        if math is not None:
            width, height, baseline_ratio = _math_dimensions(math, font_size, max_width)
            ascent = height * baseline_ratio
            items.append(
                _InlineLayoutItem(
                    kind="math",
                    width=width,
                    height=height,
                    ascent=ascent,
                    descent=height - ascent,
                    math=math,
                )
            )
            continue
        for token in _split_inline_text(part):
            width = font.text_length(token, fontsize=font_size)
            if width > max_width and len(token) > 1 and not token.isspace():
                sub_tokens = list(token)
            else:
                sub_tokens = [token]
            for sub_token in sub_tokens:
                sub_width = font.text_length(sub_token, fontsize=font_size)
                items.append(
                    _InlineLayoutItem(
                        kind="space" if sub_token.isspace() else "text",
                        width=sub_width,
                        height=font_size,
                        ascent=font_size * 0.82,
                        descent=font_size * 0.18,
                        text=sub_token,
                    )
                )
    return items


def _layout_inline_items(
    items: list[_InlineLayoutItem],
    rect: fitz.Rect,
    font_size: float,
    line_spacing: float,
) -> bool:
    lines: list[list[_InlineLayoutItem]] = []
    line: list[_InlineLayoutItem] = []
    line_width = 0.0
    for item in items:
        if item.kind == "space" and not line:
            continue
        if line and line_width + item.width > rect.width:
            while line and line[-1].kind == "space":
                line.pop()
            if line:
                lines.append(line)
            line = []
            line_width = 0.0
            if item.kind == "space":
                continue
        line.append(item)
        line_width += item.width
    while line and line[-1].kind == "space":
        line.pop()
    if line:
        lines.append(line)

    y = rect.y0
    for current_line in lines:
        ascent = max(item.ascent for item in current_line)
        descent = max(item.descent for item in current_line)
        line_height = max(ascent + descent, font_size * line_spacing)
        baseline = y + ascent
        x = rect.x0
        for item in current_line:
            item.x = x
            item.baseline = baseline
            item.y = baseline - item.ascent
            x += item.width
        y += line_height
    return y <= rect.y1 + 0.01


def _place_inline_math_segment(
    *,
    page: fitz.Page,
    src_page: fitz.Page,
    page_rect: fitz.Rect,
    rect: fitz.Rect,
    text: str,
    inline_math: Sequence[Mapping[str, object]],
    font: fitz.Font,
    font_size: float,
    opts: ComposeOptions,
    record_warning: Callable[[str], None],
    segment_id: object,
    page_number: int,
) -> bool | None:
    """Place mixed translated text and source formula snapshots.

    ``None`` means metadata is inconsistent and lets the caller use legacy text
    placement. ``False`` means no drawable item could be placed.
    """
    attempt_size = font_size
    items: list[_InlineLayoutItem] | None = None
    fits = False
    for _ in range(12):
        items = _make_inline_items(text, inline_math, font, attempt_size, rect.width)
        if items is None:
            record_warning(
                f"文中数式プレースホルダーが不正なためテキスト配置へフォールバック "
                f"(page={page_number}, id={segment_id})"
            )
            return None
        fits = _layout_inline_items(items, rect, attempt_size, opts.line_spacing)
        if fits or attempt_size <= opts.min_font_size + 0.1:
            break
        attempt_size = max(opts.min_font_size, attempt_size * 0.9)
    if not items:
        return False
    if not fits:
        record_warning(
            f"文中数式を含むテキストが収まりきりませんでした "
            f"(page={page_number}, id={segment_id})"
        )

    writer = fitz.TextWriter(page_rect)
    drawn = False
    for item in items:
        if item.y + item.height > rect.y1 + 0.01:
            continue
        if item.kind == "text":
            writer.append(
                (item.x, item.baseline),
                item.text,
                font=font,
                fontsize=attempt_size,
            )
            drawn = True
        elif item.kind == "math" and item.math is not None:
            source_bbox = item.math.get("bbox")
            if not source_bbox or len(source_bbox) != 4:
                continue
            source_rect = fitz.Rect(source_bbox) & src_page.rect
            dest_rect = fitz.Rect(item.x, item.y, item.x + item.width, item.y + item.height) & rect
            if source_rect.is_empty or dest_rect.is_empty:
                continue
            try:
                pix = src_page.get_pixmap(
                    matrix=fitz.Matrix(opts.inline_math_zoom, opts.inline_math_zoom),
                    clip=source_rect,
                    alpha=False,
                )
                page.insert_image(dest_rect, pixmap=pix, overlay=True)
                drawn = True
            except RuntimeError as exc:
                record_warning(
                    f"文中数式の切り出しに失敗しました "
                    f"(page={page_number}, id={segment_id}): {exc}"
                )
    writer.write_text(page, color=opts.text_color, overlay=True)
    return drawn


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
        visible_text = str(segment.get("translated_text") or "")
        for math in segment.get("inline_math") or []:
            placeholder = str(math.get("placeholder") or "")
            if placeholder:
                visible_text = visible_text.replace(placeholder, str(math.get("text") or ""))
        tgt_len = len(visible_text)
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


def create_comparison_pdf(
    original_pdf: str | Path,
    translated_pdf: str | Path,
    output_pdf: str | Path,
) -> Path:
    """
    元のPDFと翻訳後のPDFを交互に配置した比較用PDFを作成する。

    Args:
        original_pdf: 元のPDFファイルパス
        translated_pdf: 翻訳後のPDFファイルパス
        output_pdf: 出力PDFファイルパス

    Returns:
        出力PDFのPath
    """
    original_path = Path(original_pdf)
    translated_path = Path(translated_pdf)
    output_path = Path(output_pdf)

    if not original_path.exists():
        raise FileNotFoundError(f"元のPDFが見つかりません: {original_path}")
    if not translated_path.exists():
        raise FileNotFoundError(f"翻訳後のPDFが見つかりません: {translated_path}")

    output_path.parent.mkdir(parents=True, exist_ok=True)

    with fitz.open(original_path) as orig_doc, fitz.open(translated_path) as trans_doc:
        compare_doc = fitz.open()
        try:
            # 元のPDFと翻訳後のPDFのページ数が同じであることを確認
            if len(orig_doc) != len(trans_doc):
                raise ValueError(
                    f"PDFのページ数が一致しません (元: {len(orig_doc)}, 翻訳後: {len(trans_doc)})"
                )

            # 各ページを元→翻訳→元→翻訳...の順で挿入
            for page_idx in range(len(orig_doc)):
                # 元のページを挿入
                compare_doc.insert_pdf(orig_doc, from_page=page_idx, to_page=page_idx)
                # 翻訳後のページを挿入
                compare_doc.insert_pdf(trans_doc, from_page=page_idx, to_page=page_idx)

            compare_doc.save(output_path)
        finally:
            compare_doc.close()

    return output_path
