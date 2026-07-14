from __future__ import annotations

import argparse
import json
import os
import re
import sys
from collections import Counter
from collections.abc import Sequence
from dataclasses import dataclass
from pathlib import Path
from typing import Any

import fitz  # PyMuPDF
import numpy as np
from loguru import logger
from PIL import Image, ImageDraw, ImageFont

try:
    from doclayout_yolo import YOLO as DocYOLO
except ImportError:  # pragma: no cover - optional dependency
    DocYOLO = None  # type: ignore[assignment]

try:
    from doclayout_yolo import YOLOv10 as DocYOLOv10
except ImportError:  # pragma: no cover - optional dependency
    DocYOLOv10 = None  # type: ignore[assignment]

try:
    from ultralytics import YOLO
except ImportError:  # pragma: no cover - optional dependency
    YOLO = None  # type: ignore[assignment]

try:
    from ultralytics import YOLOv10
except ImportError:  # pragma: no cover - optional dependency
    YOLOv10 = None  # type: ignore[assignment]

try:
    from huggingface_hub import snapshot_download
    from huggingface_hub.utils import HFValidationError
except ImportError:  # pragma: no cover - handled at runtime
    snapshot_download = None  # type: ignore[assignment]
    HFValidationError = Exception  # type: ignore[assignment]

if __package__ is None:  # pragma: no cover - package-relative import fallback
    sys.path.append(str(Path(__file__).resolve().parent.parent))

from common.schema import InlineMath, SegmentBlock, SegmentPage

INLINE_MATH_PLACEHOLDER_TEMPLATE = "[[TRANSPAPER_INLINE_MATH_{index:04d}]]"
_STRONG_MATH_FONT_RE = re.compile(
    r"(?:cmmi|cmsy|cmex|msam|msbm|stix.*math|mathjax|latinmodernmath|"
    r"asana[-_ ]?math|xits[-_ ]?math|texgyre.*math|mt[-_ ]?extra)",
    re.IGNORECASE,
)
_GREEK_RANGES = ((0x0370, 0x03FF), (0x1D6A8, 0x1D7CB))
_MATH_SYMBOL_RANGES = ((0x2190, 0x22FF), (0x2308, 0x230B), (0x27E6, 0x27EF))
_MATH_SYMBOL_CODEPOINTS = frozenset(
    {0x00B1, 0x00D7, 0x00F7, 0x2102, 0x2113, 0x2115, 0x211A, 0x211D, 0x2124}
)
_FORMULA_CONNECTORS = frozenset("+-=*/<>^_|()[]{}.,:'\u2032\u2033")


@dataclass
class _TextRun:
    text: str
    bbox: tuple[float, float, float, float]
    font: str
    size: float
    baseline: float
    flags: int


def _span_text(span: dict[str, Any]) -> str:
    text = span.get("text") or ""
    if text:
        return str(text)
    return "".join(str(ch.get("c", "")) for ch in span.get("chars") or [])


def _span_bbox(span: dict[str, Any]) -> tuple[float, float, float, float] | None:
    chars = span.get("chars") or []
    char_boxes = [ch.get("bbox") for ch in chars if ch.get("bbox")]
    if char_boxes:
        return (
            min(float(box[0]) for box in char_boxes),
            min(float(box[1]) for box in char_boxes),
            max(float(box[2]) for box in char_boxes),
            max(float(box[3]) for box in char_boxes),
        )
    bbox = span.get("bbox")
    if bbox and len(bbox) == 4:
        return tuple(float(value) for value in bbox)
    return None


def _span_baseline(span: dict[str, Any], bbox: tuple[float, float, float, float]) -> float:
    origins = [ch.get("origin") for ch in span.get("chars") or [] if ch.get("origin")]
    if origins:
        return float(sum(float(origin[1]) for origin in origins) / len(origins))
    origin = span.get("origin")
    if origin and len(origin) >= 2:
        return float(origin[1])
    return bbox[3]


def _is_math_symbol(char: str) -> bool:
    codepoint = ord(char)
    if codepoint in _MATH_SYMBOL_CODEPOINTS:
        return True
    ranges = _GREEK_RANGES + _MATH_SYMBOL_RANGES
    return any(start <= codepoint <= end for start, end in ranges)


def _is_strong_math_run(run: _TextRun) -> bool:
    stripped = run.text.strip()
    if not stripped:
        return False
    if _STRONG_MATH_FONT_RE.search(run.font):
        return True
    # A normal-font prose span can contain an arrow or a Greek character. Treat
    # symbols as strong evidence only when the whole run is short and
    # formula-shaped, otherwise a single symbol could protect an entire sentence.
    return _is_formula_context_run(run) and any(_is_math_symbol(char) for char in stripped)


def _is_formula_context_run(run: _TextRun) -> bool:
    """Return whether a short neighbouring run can safely belong to a formula."""
    text = run.text.strip()
    if not text or len(text) > 8:
        return False
    if text != run.text:
        return False
    if not all(char.isalnum() or _is_math_symbol(char) or char in _FORMULA_CONNECTORS for char in text):
        return False
    alpha_count = sum(char.isalpha() and not _is_math_symbol(char) for char in text)
    return alpha_count <= 2


def _runs_are_close(left: _TextRun, right: _TextRun) -> bool:
    gap = right.bbox[0] - left.bbox[2]
    return gap <= max(left.size, right.size, 1.0) * 0.45


def _inline_math_groups(runs: list[_TextRun]) -> list[tuple[int, int]]:
    strong = {index for index, run in enumerate(runs) if _is_strong_math_run(run)}
    if not strong:
        return []

    selected = set(strong)
    for index in tuple(strong):
        cursor = index - 1
        while cursor >= 0:
            if not _is_formula_context_run(runs[cursor]) or not _runs_are_close(
                runs[cursor], runs[cursor + 1]
            ):
                break
            selected.add(cursor)
            cursor -= 1
        cursor = index + 1
        while cursor < len(runs):
            if not _is_formula_context_run(runs[cursor]) or not _runs_are_close(
                runs[cursor - 1], runs[cursor]
            ):
                break
            selected.add(cursor)
            cursor += 1

    # Superscript / subscript runs are weak evidence. Include them only next to an
    # already recognised formula so citation markers and footnotes are not captured.
    for index, run in enumerate(runs):
        if not (run.flags & fitz.TEXT_FONT_SUPERSCRIPT) or not _is_formula_context_run(run):
            continue
        neighbours = (index - 1, index + 1)
        if any(neighbour in selected for neighbour in neighbours):
            selected.add(index)

    groups: list[tuple[int, int]] = []
    for index in sorted(selected):
        if not groups or index > groups[-1][1] + 1:
            groups.append((index, index))
        else:
            groups[-1] = (groups[-1][0], index)
    return groups


def _normalise_extracted_text(text: str) -> str:
    text = text.strip()
    if not text:
        return ""
    return " ".join(text.replace("-\n", "").replace("\n", " ").split())


def _extract_text_metadata_from_raw(raw: dict[str, Any]) -> dict[str, Any]:
    text_parts: list[str] = []
    fonts: list[str] = []
    sizes: list[float] = []
    inline_math: list[InlineMath] = []
    math_index = 1

    for block in raw.get("blocks", []):
        if block.get("type") != 0:
            continue
        for line_index, line in enumerate(block.get("lines", [])):
            runs: list[_TextRun] = []
            for span in line.get("spans", []):
                text = _span_text(span)
                bbox = _span_bbox(span)
                if not text or bbox is None:
                    continue
                font = str(span.get("font", ""))
                size = float(span.get("size", 0.0))
                runs.append(
                    _TextRun(
                        text=text,
                        bbox=bbox,
                        font=font,
                        size=size,
                        baseline=_span_baseline(span, bbox),
                        flags=int(span.get("flags", 0)),
                    )
                )
                fonts.append(font)
                sizes.append(size)
            if not runs:
                continue

            groups = _inline_math_groups(runs)
            group_by_start = {start: end for start, end in groups}
            line_parts: list[str] = []
            run_index = 0
            while run_index < len(runs):
                group_end = group_by_start.get(run_index)
                if group_end is None:
                    line_parts.append(runs[run_index].text)
                    run_index += 1
                    continue
                group = runs[run_index : group_end + 1]
                placeholder = INLINE_MATH_PLACEHOLDER_TEMPLATE.format(index=math_index)
                while placeholder in "".join(run.text for run in runs):
                    math_index += 1
                    placeholder = INLINE_MATH_PLACEHOLDER_TEMPLATE.format(index=math_index)
                math_bbox = (
                    min(run.bbox[0] for run in group),
                    min(run.bbox[1] for run in group),
                    max(run.bbox[2] for run in group),
                    max(run.bbox[3] for run in group),
                )
                inline_math.append(
                    {
                        "id": f"m{math_index:04d}",
                        "placeholder": placeholder,
                        "text": "".join(run.text for run in group),
                        "bbox": math_bbox,
                        "baseline": float(sum(run.baseline for run in group) / len(group)),
                        "fonts": list(dict.fromkeys(run.font for run in group if run.font)),
                        "font_size": float(sum(run.size for run in group) / len(group)),
                        "line_index": line_index,
                    }
                )
                line_parts.append(placeholder)
                math_index += 1
                run_index = group_end + 1
            text_parts.append("".join(line_parts))
            text_parts.append("\n")

    text = _normalise_extracted_text("".join(text_parts))
    if not text:
        return {}
    display_text = text
    for math in inline_math:
        display_text = display_text.replace(math["placeholder"], math["text"])
    meta: dict[str, Any] = {
        "text": text,
        "text_preview": text[:200],
        "char_count": len(display_text),
    }
    if sizes:
        meta["avg_font_size"] = float(sum(sizes) / len(sizes))
    if fonts:
        meta["fonts_top"] = list(Counter(fonts).most_common(3))
    if inline_math:
        meta["inline_math"] = inline_math
        meta["inline_math_status"] = "protected"
    return meta


# TODO: DocLayout-YOLOのラベルセットに合わせて調整する
DOC_LAYOUT_CAPTION_CLASSES = {
    "caption",
    "caption_figure",
    "caption_table",
    "figure_caption",
    "table_caption",
}
DOC_LAYOUT_TABLE_CLASSES = {"table"}
DOC_LAYOUT_IMAGE_CLASSES = {"figure", "image", "picture", "graphic", "photo", "table"}
DOC_LAYOUT_MATH_CLASSES = {"equation", "equations", "formula", "math", "title"}
DOC_LAYOUT_TEXT_CLASSES = {
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
}


def pdf2images(
    pdf_path: str | os.PathLike[str],
    outdir: str | os.PathLike[str],
    dpi: int = 300,
) -> list[Path]:
    """
    PDF を指定 DPI で PNG 画像に変換し、生成された画像パスを返す。
    既存のディレクトリが無ければ作成する。
    """
    pdf_path = Path(pdf_path)
    outdir_path = Path(outdir)
    outdir_path.mkdir(parents=True, exist_ok=True)

    if not pdf_path.is_file():
        raise FileNotFoundError(f"PDFファイルが見つかりません: {pdf_path}")

    zoom = dpi / 72.0
    matrix = fitz.Matrix(zoom, zoom)
    image_paths: list[Path] = []

    with fitz.open(pdf_path) as doc:
        for page_index, page in enumerate(doc, start=1):
            pix = page.get_pixmap(matrix=matrix, alpha=False)
            image_path = outdir_path / f"{pdf_path.stem}_page_{page_index:03d}.png"
            pix.save(image_path)
            image_paths.append(image_path)

    return image_paths


def _normalise_label(label: str) -> str:
    return label.strip().lower().replace(" ", "_")


def _map_doclayout_label(label: str) -> str:
    normalized = _normalise_label(label)
    if normalized in DOC_LAYOUT_CAPTION_CLASSES:
        return "caption"
    if normalized in DOC_LAYOUT_TABLE_CLASSES:
        return "table"
    if normalized in DOC_LAYOUT_IMAGE_CLASSES:
        return "image"
    if normalized in DOC_LAYOUT_MATH_CLASSES:
        return "math"
    if normalized in DOC_LAYOUT_TEXT_CLASSES:
        return "text"
    return "math"


def _extract_text_metadata(
    page: fitz.Page, bbox: tuple[float, float, float, float]
) -> dict[str, Any]:
    rect = fitz.Rect(bbox)
    raw = page.get_text("rawdict", clip=rect)
    return _extract_text_metadata_from_raw(raw)


def _clamp_bbox(
    bbox: tuple[float, float, float, float],
    width: float,
    height: float,
) -> tuple[float, float, float, float]:
    x0, y0, x1, y1 = bbox
    x0 = max(0.0, min(width, x0))
    y0 = max(0.0, min(height, y0))
    x1 = max(0.0, min(width, x1))
    y1 = max(0.0, min(height, y1))
    if x1 < x0:
        x0, x1 = x1, x0
    if y1 < y0:
        y0, y1 = y1, y0
    return (x0, y0, x1, y1)


def _download_pretrained_weights(repo_id: str) -> Path:
    if snapshot_download is None:
        raise RuntimeError(
            "DocLayout-YOLOモデルを取得するには huggingface-hub が必要です。"
            " `uv add huggingface_hub` を実行するか、--model-path でローカル重みを指定してください。"
        )
    try:
        cache_dir = Path(
            snapshot_download(
                repo_id=repo_id,
                allow_patterns=("*.pt", "*.safetensors"),
            )
        )
    except HFValidationError as err:  # pragma: no cover - network dependent
        raise RuntimeError(f"Hugging Face からモデルを取得できませんでした: {err}") from err

    candidates = sorted(list(cache_dir.rglob("*.pt")) + list(cache_dir.rglob("*.safetensors")))
    if not candidates:
        raise RuntimeError(
            "Hugging Face リポジトリ内に `.pt` または `.safetensors` ファイルが見つかりませんでした。"
            " --model-path で明示的に指定してください。"
        )
    return candidates[0]


@dataclass
class DocLayoutYoloSegmenter:
    conf: float = 0.25
    iou: float = 0.5
    device: str | None = None
    image_size: int | None = 1024

    def __post_init__(self) -> None:
        # モデルローダは遅延初期化し、失敗時はフォールバックする
        self._model: Any = None
        self.model_source: str = "uninitialized"

    def _load_local(self, path: Path) -> Any:
        """
        ローカルのDocLayout-YOLOモデルをロードする
        いずれかの対応するモデルローダーで試行し、成功した最初のものを返す
        """
        errors: list[str] = []
        for loader in (DocYOLOv10, DocYOLO, YOLOv10, YOLO):
            if loader is None:
                continue
            try:
                return loader(str(path))  # type: ignore[call-arg]
            except Exception as exc:  # pragma: no cover - loader fallback
                errors.append(f"{loader.__name__}: {exc}")
        raise RuntimeError("DocLayout-YOLOモデルをロードできませんでした。\n" + "\n".join(errors))

    def _install_doclayout_yolo(
        self, default_model: str = "juliozhao/DocLayout-YOLO-DocStructBench"
    ) -> str:
        """
        DocLayout-YOLOのモデルを解決する。
        優先度: (1) プロジェクト直下の実体ファイル -> (2) Hugging Face キャッシュパス
        """
        project_root = Path(__file__).resolve().parent.parent
        default_model_path = project_root / "models" / "doclayout_yolo_docstructbench_imgsz1024.pt"
        # 実体ファイルが存在する場合のみ使用（壊れたシンボリックリンクは無視）
        if default_model_path.is_file():
            return str(default_model_path)
        import huggingface_hub

        try:
            logger.info("Resolving DocLayout-YOLO model via Hugging Face cache...")
            filepath = huggingface_hub.hf_hub_download(
                repo_id="juliozhao/DocLayout-YOLO-DocStructBench",
                filename="doclayout_yolo_docstructbench_imgsz1024.pt",
                local_files_only=False,
            )
            # ダウンロードしたキャッシュのパスをそのまま返す
            return str(Path(filepath))
        except Exception as e:
            # ネットワーク不可や権限問題などで失敗するケースがある
            logger.warning(f"DocLayout-YOLOの自動取得に失敗しました。フォールバックします: {e}")
            # フォールバック用に存在しないパスは返さない
            raise

    def predict(self, image: np.ndarray):
        """モデル推論。失敗時は全画面テキスト1ブロックにフォールバックする。"""

        class _PseudoBoxes:
            def __init__(self, w: int, h: int) -> None:
                self.xyxy = np.array([[0.0, 0.0, float(w), float(h)]], dtype=float)
                self.cls = np.array([0], dtype=int)
                self.conf = np.array([1.0], dtype=float)

        class _PseudoDetections:
            def __init__(self, w: int, h: int) -> None:
                self.boxes = _PseudoBoxes(w, h)
                self.names = {0: "text"}

        def _fallback_result(img: np.ndarray):
            h, w = (img.shape[0], img.shape[1]) if img.ndim >= 2 else (1024, 768)
            return [_PseudoDetections(w, h)]

        if self._model is None:
            try:
                # 依存が無い／壊れている場合も例外で拾う
                if all(mod is None for mod in (DocYOLO, DocYOLOv10, YOLO, YOLOv10)):
                    raise RuntimeError("DocLayout-YOLOがインストールされていません")
                model_path = self._install_doclayout_yolo()
                self.model_source = model_path
                self._model = self._load_local(Path(model_path))
            except Exception as e:  # フォールバック
                logger.warning(f"DocLayout-YOLOのロードに失敗したためフォールバックします: {e}")
                self.model_source = "fallback:text-full-page"
                # フォールバックモードを記録（以降のページで再試行しない）
                self._model = "__FALLBACK__"
                return _fallback_result(image)[0]
        if self._model == "__FALLBACK__":
            return _fallback_result(image)[0]

        try:
            results = self._model.predict(
                image,
                conf=self.conf,
                iou=self.iou,
                device=self.device,
                imgsz=self.image_size,
                verbose=False,
            )
            return results[0]
        except Exception as e:  # 推論失敗時もフォールバック
            logger.warning(f"DocLayout-YOLO推論に失敗したためフォールバックします: {e}")
            self.model_source = "fallback:text-full-page"
            return _fallback_result(image)[0]


def _draw_overlay(
    image: Image.Image,
    blocks: Sequence[SegmentBlock],
    zoom: float,
    save_path: Path,
) -> None:
    draw = ImageDraw.Draw(image, "RGBA")
    color_map: dict[str, tuple[int, int, int, int]] = {
        "text": (0, 128, 255, 60),
        "image": (255, 128, 0, 60),
        "table": (0, 200, 0, 60),
        "caption": (200, 0, 200, 60),
        "math": (200, 200, 0, 60),
    }
    border_map = {k: (c[0], c[1], c[2], 255) for k, c in color_map.items()}
    font = ImageFont.load_default()

    for block in blocks:
        bbox = block.get("bbox")
        if not bbox:
            continue
        x0, y0, x1, y1 = bbox
        x0p, y0p, x1p, y1p = [v * zoom for v in (x0, y0, x1, y1)]
        segment_type = block.get("type", "text")
        overlay_color = color_map.get(segment_type, (255, 0, 0, 60))
        border_color = border_map.get(segment_type, (255, 0, 0, 255))
        draw.rectangle(
            [x0p, y0p, x1p, y1p],
            outline=border_color,
            fill=overlay_color,
            width=2,
        )

        meta = block.get("meta") or {}
        label = meta.get("doclayout_label", segment_type)
        score = meta.get("confidence")
        label_text = f"{label}"
        if isinstance(score, (int, float)):
            label_text = f"{label_text} ({score:.2f})"
        text_pos = (x0p + 4, y0p + 4)
        text_bbox = draw.textbbox(text_pos, label_text, font=font)
        draw.rectangle(
            [text_bbox[0] - 2, text_bbox[1] - 2, text_bbox[2] + 2, text_bbox[3] + 2],
            fill=(255, 255, 255, 200),
        )
        draw.text(text_pos, label_text, fill=(0, 0, 0, 255), font=font)

    image.save(save_path)


def _build_segment_blocks(
    page: fitz.Page,
    detections,
    zoom: float,
    page_index: int,
) -> list[SegmentBlock]:
    blocks: list[SegmentBlock] = []
    if detections is None or getattr(detections, "boxes", None) is None:
        return blocks

    boxes = detections.boxes
    names_map = detections.names or {}
    xyxy = boxes.xyxy.cpu().numpy() if hasattr(boxes.xyxy, "cpu") else boxes.xyxy
    cls_indices = boxes.cls.cpu().numpy() if hasattr(boxes.cls, "cpu") else boxes.cls
    confidences = boxes.conf.cpu().numpy() if hasattr(boxes.conf, "cpu") else boxes.conf

    width = float(page.rect.width)
    height = float(page.rect.height)
    scale = 1.0 / max(zoom, 1e-6)

    for det_idx, (bbox_xyxy, cls_idx, conf_val) in enumerate(zip(xyxy, cls_indices, confidences)):
        x0, y0, x1, y1 = [float(v) * scale for v in bbox_xyxy.tolist()]
        bbox_pdf = _clamp_bbox((x0, y0, x1, y1), width, height)
        area = (bbox_pdf[2] - bbox_pdf[0]) * (bbox_pdf[3] - bbox_pdf[1])
        if area <= 1e-2:
            continue

        label = names_map.get(int(cls_idx), str(int(cls_idx)))
        segment_type = _map_doclayout_label(label)
        meta: dict[str, Any] = {"doclayout_label": label, "confidence": float(conf_val)}
        if segment_type in {"text", "caption"}:
            meta.update(_extract_text_metadata(page, bbox_pdf))

        block: SegmentBlock = {
            "id": f"p{page_index:03d}_b{det_idx:03d}",
            "type": segment_type,
            "bbox": bbox_pdf,
            "meta": meta,
        }
        blocks.append(block)
    return blocks


def segment_pdf(
    pdf_path: str | os.PathLike[str],
    outdir: str = "out",
    dpi: int = 150,
    conf_threshold: float = 0.25,
    iou_threshold: float = 0.5,
    device: str | None = "cpu",
    image_size: int | None = 1024,
) -> list[SegmentPage]:
    """
    DocLayout-YOLOを用いてPDFをレイアウトセグメントに分割し、検出結果をJSONとPNGで保存する。
    """
    pdf_path = Path(pdf_path)
    if not pdf_path.is_file():
        raise FileNotFoundError(f"PDFファイルが見つかりません: {pdf_path}")

    outdir_path = Path(outdir)
    outdir_path.mkdir(parents=True, exist_ok=True)

    segmenter = DocLayoutYoloSegmenter(
        conf=conf_threshold,
        iou=iou_threshold,
        device=device,
        image_size=image_size,
    )

    doc = fitz.open(pdf_path)
    results: list[SegmentPage] = []

    for page_index, page in enumerate(doc, start=1):
        zoom = dpi / 72.0
        pix = page.get_pixmap(matrix=fitz.Matrix(zoom, zoom), alpha=False)
        image = Image.frombytes("RGB", [pix.width, pix.height], pix.samples)
        detections = segmenter.predict(np.asarray(image))

        blocks = _build_segment_blocks(
            page,
            detections,
            zoom=zoom,
            page_index=page_index,
        )

        overlay_path = outdir_path / f"page_{page_index:03d}_seg.png"
        _draw_overlay(image.copy(), blocks, zoom=zoom, save_path=overlay_path)

        json_path = outdir_path / f"page_{page_index:03d}.json"
        page_payload: SegmentPage = {
            "page": page_index,
            "size": {
                "width": float(page.rect.width),
                "height": float(page.rect.height),
            },
            "blocks": blocks,
            "png_overlay": str(overlay_path),
            "json": str(json_path),
            "doclayout_model": segmenter.model_source,
            "doclayout_confidence": float(conf_threshold),
            "doclayout_iou": float(iou_threshold),
            "dpi": dpi,
            "granularity": "block",
        }

        with json_path.open("w", encoding="utf-8") as fh:
            json.dump(page_payload, fh, ensure_ascii=False, indent=2)

        results.append(page_payload)

    return results


def main() -> None:
    parser = argparse.ArgumentParser(description="DocLayout-YOLOによるPDFセグメント抽出")
    parser.add_argument("pdf", help="入力PDFパス")
    parser.add_argument("--outdir", default="out", help="結果出力ディレクトリ")
    parser.add_argument("--dpi", type=int, default=150, help="ページのレンダリングDPI")
    parser.add_argument(
        "--model-path",
        dest="model_path",
        help="DocLayout-YOLOの重みファイルパス(未指定時は既定ロケーションを検索)",
    )
    parser.add_argument("--conf", type=float, default=0.25, help="YOLO信頼度しきい値")
    parser.add_argument("--iou", type=float, default=0.5, help="YOLO IoUしきい値")
    parser.add_argument("--device", help="推論デバイス (例: cpu, cuda:0)")
    parser.add_argument("--imgsz", type=int, default=1024, help="推論時の画像サイズ")
    args = parser.parse_args()

    results = segment_pdf(
        args.pdf,
        outdir=args.outdir,
        dpi=args.dpi,
        model_path=args.model_path,
        conf_threshold=args.conf,
        iou_threshold=args.iou,
        device=args.device,
        image_size=args.imgsz,
    )
    for page in results:
        print(f"[OK] page {page['page']}: {page['png_overlay']}  {page['json']}")


if __name__ == "__main__":  # pragma: no cover
    main()
