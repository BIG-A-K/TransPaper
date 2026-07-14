# スキーマ定義モジュール（`common/schema.py`）

PDF 翻訳パイプラインでやり取りされる JSON 構造を型としてまとめたモジュールです。`TypedDict` を用いた軽量な定義により、エディタの補完や型チェックでデータ構造の齟齬を早期に検知できます。

## 提供している主な型
- `SegmentPage`: セグメント抽出 (`common/seg.py`) が返す 1 ページ分の辞書。`page` / `size` / `blocks` に加えて、`granularity` や `math_threshold` など実行パラメータも保持します。
- `SegmentBlock`: セグメント一覧の 1 要素。`type`（`text` / `image` / `table` / `caption` / `math` / `merged`）と `bbox`（PDF 座標系の `[x0, y0, x1, y1]`）を必須とし、`meta` にテキストや画像固有の追加情報を格納します。
- `TextBlockMeta` / `ImageBlockMeta`: ブロックごとの補助情報。テキスト系は原文テキストやフォント統計値、画像系は抽出経路などを含みます。
- `TranslatedPage`: 翻訳済みセグメントをページ単位でまとめた構造。`compose_pdf` などの後段処理が期待する JSON 形式と一致します。
- `TranslationSegment`: 翻訳セグメント 1 件を表す辞書。原文テキスト関連のメタ情報に加え、`translated_text` を必須フィールドとしています。
- `InlineMath`: 文中数式1件のプレースホルダー、原文字列、原稿bbox、baseline、フォント情報を表します。`TextBlockMeta.inline_math` から `TranslationSegment.inline_math` へ伝播します。Python版の `common/schema.py` とRust版の `rust/src/schema.rs` は同じJSONフィールド名を使用します。

## セグメント JSON の例
```json
{
  "page": 1,
  "size": {"width": 595.0, "height": 842.0},
  "granularity": "line",
  "math_threshold": 0.15,
  "blocks": [
    {
      "type": "text",
      "bbox": [60.5, 120.3, 540.2, 150.8],
      "meta": {
        "text": "Self-attention allows the model to focus on relevant parts.",
        "text_preview": "Self-attention allows the model...",
        "char_count": 68,
        "avg_font_size": 11.0
      }
    },
    {
      "type": "image",
      "bbox": [80.1, 200.0, 500.0, 420.0],
      "meta": {"source": "rawdict"}
    }
  ],
  "png_overlay": "out/_attention_is_all_you_need/seg/page_001_seg.png",
  "json": "out/_attention_is_all_you_need/seg/page_001.json"
}
```

## 翻訳 JSON の例
```json
[
  {
    "page": 1,
    "segments": [
      {
        "type": "text",
        "bbox": [60.5, 120.3, 540.2, 150.8],
        "source_text": "Self-attention allows the model to focus on relevant parts.",
        "char_count": 68,
        "avg_font_size": 11.0,
        "translated_text": "自己注意により、モデルは関連する部分に着目できます。"
      }
    ]
  }
]
```
`compose_pdf` はこの配列形式を受け取り、ページ毎の `segments` を参照して翻訳テキストを配置します。

## 利用方法
Python コード側では `from common.schema import SegmentPage` のようにインポートして利用します。型を明示するだけでも、翻訳パイプラインの各ステップで扱うデータが揃っているかを確認しやすくなります。`main.py` と `common/seg.py` は既に本モジュールを参照するように更新されています。後続の翻訳器やレイアウト処理を追加する際も、このスキーマを介してやり取りすることで、JSON 形式の変化に即応できます。
