# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

TransPaperは英語のPDF論文を日本語に翻訳するPythonプロジェクトです。PDFをレイアウト要素（text, caption, figure, tableなど）にセグメント分割し、テキスト要素を翻訳した後、元のレイアウトで再統合します。

## Development Environment

- Python 3.12以上
- uvパッケージマネージャで管理（`pyproject.toml`で定義）
- 実行は`uv run`コマンドを使用
- M4 Mac 16GBで開発されていますが、Linux/macOS環境で動作します
- Docker環境も用意されています（`docker/`）

## Essential Commands

```bash
# Run translation (requires DEEPL_API environment variable)
DEEPL_API={your-key} uv run main.py --input file.pdf --output translated.pdf --model deepl

# Run translation with comparison PDF (original | translated | original | translated...)
DEEPL_API={your-key} uv run main.py --input file.pdf --compare

# Quick test with no translation (identity function)
uv run main.py --input attention_is_all_you_need.pdf --model idx

# Run automated test
make test  # Downloads attention_is_all_you_need.pdf and runs translation

# Lint and format code
make lint  # Runs ruff check and format

# CI checks (includes wrkflw validation)
make ci

# Docker environment
make build  # Build container
make up     # Start container
make in     # Enter container
```

## Adding Dependencies

**重要**: 新しいモジュールを追加する際は`uv add <package>`を使用してください。`uv pip`は使用禁止です（`uv pip list`は可）。

## Core Architecture

TransPaperは4つのステージで構成されるパイプラインです：

1. **Segmentation** (`common/seg.py`)
   - DocLayout-YOLOを使用してPDFページをレイアウト要素に分割
   - 各要素にbbox（bounding box）、type（text/image/table/caption/math）、metadataを付与
   - 出力: `SegmentPage`のリスト（JSON + PNG overlayで保存）

2. **Translation** (`common/translate.py`)
   - テキスト要素（text, caption）を翻訳
   - サポートモデル: DeepL API（デフォルト）、HuggingFace（未実装）、idx（翻訳なし、テスト用）
   - バッチ処理対応：短いテキスト（<50単語）をバッチでまとめて翻訳
   - 出力: 翻訳されたセグメントをJSONで保存

3. **Collection** (`common/translate.py:collect_translated_pages`)
   - 翻訳済みJSONファイルを収集し`TranslatedPage`形式に変換
   - document_translation.jsonに統合

4. **Composition** (`common/compose.py`)
   - 元のPDFと翻訳結果を統合
   - 原文テキストをredaction（塗りつぶし）で除去
   - 翻訳テキストをPyMuPDFのTextWriterで配置
   - image/table/math領域は元のページから切り出して配置

### Data Flow

```
PDF → seg.segment_pdf()
    → list[SegmentPage]
    → translate.translate()
    → translated JSON files
    → translate.collect_translated_pages()
    → list[TranslatedPage]
    → compose.compose_pdf()
    → translated PDF
```

### Type Definitions

主要な型は`common/schema.py`で定義：
- `SegmentBlock`: セグメント分割された1つのブロック（bbox, type, meta）
- `SegmentPage`: 1ページ分のセグメント（blocks, size, DPI等）
- `TranslationSegment`: 翻訳された1つのセグメント（bbox, translated_text）
- `TranslatedPage`: 1ページ分の翻訳結果（segments）

## Code Structure

```
common/
├── seg.py       # PDF segmentation (DocLayout-YOLO)
├── translate.py # Translation (DeepL/HuggingFace)
├── compose.py   # PDF composition (PyMuPDF)
└── schema.py    # TypedDict definitions
main.py          # CLI entry point
docs/            # Module documentation
```

`main.py`が`common/`配下のモジュールをimportして実行します。各モジュールは独立して動作可能です。

## Coding Guidelines

- **コード規約**: Ruff (line-length=100, target-version=py312)
- **型アノテーション**: 厳密な型チェック（ANN ルール有効）
- **日本語コミュニケーション**: チャットは日本語で対応
- **ドキュメント**: 新規モジュールは`docs/{モジュール名}.md`で説明を作成

## Important Restrictions

1. **`rm`コマンド使用禁止**: ファイル削除は`gomi/`ディレクトリへのmvで代替
2. **`Makefile`編集禁止**: 変更が必要な場合は相談
3. **`docker/`編集禁止**: Docker環境の変更は`AGENTS.md`に記載して相談
4. **`uv pip`使用禁止**: 依存関係追加は`uv add`のみ

## Working Directory Structure

- `/tmp/_<pdf_stem>/`: 一時ファイルの作業ディレクトリ
  - `segments/`: セグメント分割結果（JSON + PNG）
  - `translated/`: 翻訳結果（JSON）
  - `composed/`: 統合処理用（現在未使用）
- `out/`: プロジェクトルートの出力ディレクトリ
- `gomi/`: 削除ファイルの移動先

## Testing

テストPDFとして`attention_is_all_you_need.pdf`を使用します：
- `make test`で自動ダウンロード＆翻訳実行（`--model idx`で翻訳スキップ）

## Key Dependencies

- `pymupdf` (fitz): PDF読み込み・書き込み
- `doclayout-yolo`: レイアウトセグメンテーション
- `ultralytics`: YOLOモデル推論
- `huggingface-hub`: モデル自動ダウンロード
- `requests`: DeepL API呼び出し
- `click`: CLIインターフェース
- `loguru`: ロギング
- `tqdm`: 進捗表示

## Model Management

DocLayout-YOLOモデルは以下の優先順で解決：
1. `models/doclayout_yolo_docstructbench_imgsz1024.pt`（プロジェクト直下）
2. Hugging Face cache (`juliozhao/DocLayout-YOLO-DocStructBench`)
3. フォールバック: 全ページを単一のtextブロックとして扱う

モデルが見つからない場合でもクラッシュせず、フォールバックモードで動作します。
