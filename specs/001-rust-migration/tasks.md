# Tasks: rust-migration

`spec.md` + `plan.md` から生成された実行チェックリスト。**各タスクはファイルパスを明示し、spec↔コードの契約となる。**

## 凡例

- `[ ]` / `[x]`: 未完了 / 完了
- `[P]`: 並列実行可能（依存の無いタスク）
- 依存順に並べる（前のタスクが後のタスクの前提）

## Phase 1: Rust プロジェクト初期化

- [x] 1.1 `rust/` ディレクトリに Cargo プロジェクトを作成。candle-core, candle-nn, serde, serde_json, image, hf-hub を依存に追加 (`rust/Cargo.toml`)
- [x] 1.2 Python版と互換の型定義を serde derive で実装: SegmentPage, SegmentBlock, TranslationSegment, TranslatedPage (`rust/src/schema.rs`)
- [x] 1.3 main.rs に仮の CLI エントリポイントを作成 (`rust/src/main.rs`)

**チェックポイント**: `cargo build` が通ること

## Phase 2: PoC — candle で DocLayout-YOLO 推論（Go/No-Go）

- [x] 2.1 DocLayout-YOLO のONNX形式モデルを `wybxc/DocLayout-YOLO-DocStructBench-onnx` から取得（candle-onnxがMaxPool未対応のためortに変更）
- [x] 2.2 ort で DocLayout-YOLO ONNXモデルをロード (`rust/src/seg.rs`)
- [x] 2.3 PNG画像を読み込み、前処理（リサイズ・パディング・正規化）→推論実行。bbox + class + confidence を出力 (`rust/src/seg.rs`)
- [x] 2.4 推論結果を SegmentBlock に変換し、Python版の出力JSONと目視比較 → 17ブロック検出、構成一致を確認 (`rust/src/seg.rs`)

**チェックポイント**: ✅ Go判定。Rust版・Python版ともに17ブロック検出、要素種類・構成が概ね一致。

## Phase 3: PoC — PDF操作ライブラリ選定

- [x] 3.1 mupdf-rs v0.8.0 で PDF → PNG レンダリングを検証 → 1275x1650で正常出力 (`rust/src/compose.rs`)
- [x] 3.2 mupdf-rs で redaction（テキスト塗りつぶし）+ テキスト配置を検証 → Shape API で成功 (`rust/src/compose.rs`)
- [x] 3.3 mupdf-rs で画像挿入を検証 → PageImageSource::Bytes で成功 (`rust/src/compose.rs`)
- [x] 3.4 ライブラリ選定結果: mupdf-rs v0.8.0（AGPL）に確定

**チェックポイント**: ✅ PDF→PNG、redaction、テキスト配置、画像挿入の4機能すべて実現可能。

## Phase 4: Segmentation 完成

- [x] 4.1 モデル解決ロジックを実装: ローカルファイル優先 → hf-hub ダウンロード → フォールバック (`rust/src/model.rs`)
- [x] 4.2 PDF → PNG レンダリングを seg.rs に統合（mupdf使用） (`rust/src/seg.rs`)
- [x] 4.3 segment_pdf 関数を実装: PDF入力 → YOLO推論 → SegmentPage JSON出力 + テキスト抽出 (`rust/src/seg.rs`)

**チェックポイント**: `cargo run -- --input attention_is_all_you_need.pdf` で segmentation JSON が出力され、Python版と互換フォーマットであること

## Phase 5: Translation

- [P] 5.1 DeepL API 呼び出し関数を実装: reqwest blocking で POST (`rust/src/translate.rs`)
- [ ] 5.2 バッチ処理ロジックを実装: 短文（< 50語）をまとめて送信 (`rust/src/translate.rs`)
- [ ] 5.3 translate 関数を実装: SegmentPage → 翻訳済み JSON 出力 (`rust/src/translate.rs`)
- [ ] 5.4 collect_translated_pages を実装: 翻訳済み JSON → Vec<TranslatedPage> (`rust/src/translate.rs`)

**チェックポイント**: segmentation JSON を入力として翻訳済み JSON が出力されること

## Phase 6: Composition

- [ ] 6.1 compose_pdf を実装: 原文PDF + 翻訳済みJSON → 翻訳PDF出力 (`rust/src/compose.rs`)
- [ ] 6.2 redaction（原文テキスト塗りつぶし）を実装 (`rust/src/compose.rs`)
- [ ] 6.3 翻訳テキスト配置（フォントサイズ自動調整含む）を実装 (`rust/src/compose.rs`)
- [ ] 6.4 画像/表/数式領域の再配置を実装 (`rust/src/compose.rs`)
- [P] 6.5 compare PDF 生成（元→翻訳交互配置）を実装 (`rust/src/compose.rs`)

**チェックポイント**: attention_is_all_you_need.pdf のエンドツーエンド翻訳（--model idx）が Python 版と同等の出力PDFを生成すること

## Phase 7: CLI 統合・ビルド

- [ ] 7.1 clap で CLI を完成: --input, --output, --model, --compare オプション (`rust/src/main.rs`)
- [ ] 7.2 4ステージパイプラインを main.rs で統合 (`rust/src/main.rs`)
- [P] 7.3 macOS (Apple Silicon / x86_64) + Linux (x86_64) 向けビルド検証 (`rust/Cargo.toml`, CI)

**チェックポイント**: シングルバイナリで `./transpaper --input paper.pdf --output translated.pdf --model deepl` が動作すること

## converge で追記されたタスク

`converge` フェーズで差分が見つかった場合にここへ追記し、implement ループへ戻す。
