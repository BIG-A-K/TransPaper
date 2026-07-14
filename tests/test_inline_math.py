from __future__ import annotations

import json
import tempfile
import unittest
from pathlib import Path

import fitz

from common.compose import ComposeOptions, compose_pdf
from common.seg import _extract_text_metadata_from_raw
from common.translate import _store_translation_result, collect_translated_pages


def _span(
    text: str,
    font: str,
    x0: float,
    x1: float,
    *,
    flags: int = 0,
    y0: float = 10.0,
    y1: float = 22.0,
    baseline: float = 20.0,
) -> dict:
    width = (x1 - x0) / max(len(text), 1)
    chars = [
        {
            "c": char,
            "bbox": (x0 + index * width, y0, x0 + (index + 1) * width, y1),
            "origin": (x0 + index * width, baseline),
        }
        for index, char in enumerate(text)
    ]
    return {
        "font": font,
        "size": 10.0,
        "flags": flags,
        "bbox": (x0, y0, x1, y1),
        "origin": (x0, baseline),
        "chars": chars,
    }


class InlineMathExtractionTests(unittest.TestCase):
    def test_math_font_and_symbol_are_replaced_as_one_formula(self) -> None:
        raw = {
            "blocks": [
                {
                    "type": 0,
                    "lines": [
                        {
                            "spans": [
                                _span("The shape is ", "Times-Roman", 10, 70),
                                _span("H", "CMR10", 70, 77),
                                _span("\u00d7", "CMSY10", 77, 84),
                                _span("W", "CMMI10", 84, 92),
                                _span(" pixels.", "Times-Roman", 92, 130),
                            ]
                        }
                    ],
                }
            ]
        }

        meta = _extract_text_metadata_from_raw(raw)

        self.assertEqual(meta["text"], "The shape is [[TRANSPAPER_INLINE_MATH_0001]] pixels.")
        self.assertEqual(len(meta["inline_math"]), 1)
        math = meta["inline_math"][0]
        self.assertEqual(math["text"], "H\u00d7W")
        self.assertEqual(tuple(math["bbox"]), (70.0, 10.0, 92.0, 22.0))
        self.assertEqual(math["baseline"], 20.0)
        self.assertEqual(math["fonts"], ["CMR10", "CMSY10", "CMMI10"])

    def test_normal_english_and_standalone_superscript_are_not_math(self) -> None:
        raw = {
            "blocks": [
                {
                    "type": 0,
                    "lines": [
                        {
                            "spans": [
                                _span("Attention is all you need", "Times-Roman", 10, 140),
                                _span("1", "Times-Roman", 141, 145, flags=fitz.TEXT_FONT_SUPERSCRIPT),
                            ]
                        }
                    ],
                }
            ]
        }

        meta = _extract_text_metadata_from_raw(raw)

        self.assertNotIn("inline_math", meta)
        self.assertEqual(meta["text"], "Attention is all you need1")

    def test_symbol_inside_long_normal_font_prose_is_not_math(self) -> None:
        raw = {
            "blocks": [
                {
                    "type": 0,
                    "lines": [
                        {
                            "spans": [
                                _span(
                                    "The process A → B is described below.",
                                    "Times-Roman",
                                    10,
                                    190,
                                )
                            ]
                        }
                    ],
                }
            ]
        }

        meta = _extract_text_metadata_from_raw(raw)

        self.assertNotIn("inline_math", meta)
        self.assertEqual(meta["text"], "The process A → B is described below.")

    def test_superscript_is_included_only_next_to_detected_math(self) -> None:
        raw = {
            "blocks": [
                {
                    "type": 0,
                    "lines": [
                        {
                            "spans": [
                                _span("value ", "Times-Roman", 10, 45),
                                _span("x", "CMMI10", 45, 52),
                                _span(
                                    "2",
                                    "CMR7",
                                    52,
                                    56,
                                    flags=fitz.TEXT_FONT_SUPERSCRIPT,
                                    y0=7,
                                    y1=16,
                                    baseline=14,
                                ),
                                _span(" follows", "Times-Roman", 56, 95),
                            ]
                        }
                    ],
                }
            ]
        }

        meta = _extract_text_metadata_from_raw(raw)

        self.assertEqual(meta["inline_math"][0]["text"], "x2")
        self.assertEqual(meta["text"], "value [[TRANSPAPER_INLINE_MATH_0001]] follows")


class InlineMathTranslationTests(unittest.TestCase):
    def test_placeholder_validation_and_source_fallback(self) -> None:
        placeholder = "[[TRANSPAPER_INLINE_MATH_0001]]"
        meta = {
            "text": f"shape {placeholder}",
            "inline_math": [{"placeholder": placeholder, "text": "H\u00d7W"}],
        }

        _store_translation_result(meta, f"形状は {placeholder} です")
        self.assertEqual(meta["inline_math_status"], "preserved")
        self.assertEqual(meta["translated_text"], f"形状は {placeholder} です")

        _store_translation_result(meta, "形状は H x W です")
        self.assertEqual(meta["inline_math_status"], "fallback_source")
        self.assertEqual(meta["translated_text"], f"shape {placeholder}")
        self.assertTrue(meta["translation_warnings"])

    def test_collect_propagates_inline_math_metadata(self) -> None:
        placeholder = "[[TRANSPAPER_INLINE_MATH_0001]]"
        with tempfile.TemporaryDirectory() as temp_dir:
            directory = Path(temp_dir)
            payload = {
                "page": 1,
                "blocks": [
                    {
                        "id": "p001_b001",
                        "type": "text",
                        "bbox": [10, 10, 200, 40],
                        "meta": {
                            "text": f"shape {placeholder}",
                            "translated_text": f"result {placeholder}",
                            "inline_math_status": "preserved",
                            "inline_math": [
                                {
                                    "id": "m0001",
                                    "placeholder": placeholder,
                                    "text": "H\u00d7W",
                                    "bbox": [50, 10, 70, 22],
                                }
                            ],
                        },
                    }
                ],
            }
            (directory / "page_001.json").write_text(
                json.dumps(payload, ensure_ascii=False), encoding="utf-8"
            )

            pages = collect_translated_pages(directory)

        segment = pages[0]["segments"][0]
        self.assertEqual(segment["inline_math_status"], "preserved")
        self.assertEqual(segment["inline_math"][0]["text"], "H\u00d7W")


class InlineMathComposeTests(unittest.TestCase):
    def _create_source(self, path: Path) -> None:
        doc = fitz.open()
        page = doc.new_page(width=300, height=160)
        page.insert_text((20, 40), "Original formula HxW and prose", fontsize=12)
        doc.save(path)
        doc.close()

    def test_compose_without_math_keeps_legacy_text_path(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            directory = Path(temp_dir)
            source = directory / "source.pdf"
            output = directory / "plain.pdf"
            self._create_source(source)
            pages = [
                {
                    "page": 1,
                    "segments": [
                        {
                            "id": "plain",
                            "type": "text",
                            "bbox": (20, 20, 280, 70),
                            "translated_text": "Plain translated text",
                            "avg_font_size": 12,
                        }
                    ],
                }
            ]

            result = compose_pdf(source, pages, output, ComposeOptions(font_name="helv"))
            with fitz.open(output) as doc:
                page_text = doc[0].get_text()
                images = doc[0].get_images(full=True)

        self.assertEqual(result.segment_count, 1)
        self.assertIn("Plain translated text", page_text)
        self.assertEqual(images, [])

    def test_compose_inserts_source_snapshot_at_placeholder(self) -> None:
        placeholder = "[[TRANSPAPER_INLINE_MATH_0001]]"
        with tempfile.TemporaryDirectory() as temp_dir:
            directory = Path(temp_dir)
            source = directory / "source.pdf"
            output = directory / "inline.pdf"
            self._create_source(source)
            pages = [
                {
                    "page": 1,
                    "segments": [
                        {
                            "id": "inline",
                            "type": "text",
                            "bbox": (20, 20, 280, 85),
                            "translated_text": f"Result {placeholder} remains exact.",
                            "avg_font_size": 12,
                            "inline_math_status": "preserved",
                            "inline_math": [
                                {
                                    "id": "m0001",
                                    "placeholder": placeholder,
                                    "text": "HxW",
                                    "bbox": (100, 25, 125, 44),
                                    "baseline": 40,
                                    "font_size": 12,
                                }
                            ],
                        }
                    ],
                }
            ]

            result = compose_pdf(source, pages, output, ComposeOptions(font_name="helv"))
            with fitz.open(output) as doc:
                page_text = doc[0].get_text()
                images = doc[0].get_images(full=True)
                text_sizes = [
                    span["size"]
                    for block in doc[0].get_text("dict")["blocks"]
                    for line in block.get("lines", [])
                    for span in line.get("spans", [])
                ]

        self.assertEqual(result.segment_count, 1)
        self.assertIn("Result", page_text)
        self.assertIn("remains exact.", page_text)
        self.assertNotIn("TRANSPAPER_INLINE_MATH", page_text)
        self.assertGreaterEqual(len(images), 1)
        self.assertTrue(text_sizes)
        self.assertGreaterEqual(min(text_sizes), 10.0)


if __name__ == "__main__":
    unittest.main()
