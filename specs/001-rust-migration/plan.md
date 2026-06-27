# Plan: rust-migration

`spec.md` の what/why をどう実装するか。コードパスの対応表がこの文書の核心。

## 技術スタック

- Rust edition 2021, stable toolchain
- ort 2.0 (ONNX Runtime) — DocLayout-YOLO 推論
- hf-hub (latest) — HuggingFace モデルダウンロード
- serde 1.x, serde_json 1.x — JSON シリアライズ/デシリアライズ
- reqwest (blocking feature) — DeepL API 呼び出し
- clap 4.x — CLI パーサー
- image (latest) — PNG 読み込み・画像処理
- tracing + tracing-subscriber — ロギング
- indicatif (latest) — 進捗表示
- mupdf 0.8 (all-fonts feature) — PDF読み込み・書き込み・redaction・テキスト配置

## アーキテクチャ

Python版と同じ4ステージパイプラインをRustモジュールとして再実装する。
中間ファイル形式（JSON）はPython版と互換。

```
PDF → seg::segment_pdf()
    → Vec<SegmentPage>  (JSON保存)
    → translate::translate()
    → translated JSON files
    → translate::collect_translated_pages()
    → Vec<TranslatedPage>
    → compose::compose_pdf()
    → translated PDF
```

### プロジェクト構成

```
rust/
├── Cargo.toml
└── src/
    ├── main.rs          # CLI エントリポイント (clap)
    ├── seg.rs           # Segmentation (ort + DocLayout-YOLO ONNX)
    ├── translate.rs     # Translation (DeepL API)
    ├── compose.rs       # PDF Composition
    ├── schema.rs        # データ型定義 (serde Serialize/Deserialize)
    └── model.rs         # モデルダウンロード・解決 (hf-hub)
```

### 段階的実装フェーズ

| フェーズ | 内容 | Go/No-Go |
|----------|------|----------|
| PoC Phase 1 | PNG画像を入力として ort で DocLayout-YOLO ONNX 推論を実行。Python版の出力と目視比較 | **ort で推論不可なら中止** |
| PoC Phase 2 | Rust で PDF → PNG レンダリングを検証（mupdf 採用済み） | 完了 |
| Phase 3 | Segmentation 全体（PDF入力→YOLO推論→JSON出力） | — |
| Phase 4 | Translation（DeepL API バッチ呼び出し） | — |
| Phase 5 | Composition（redaction・テキスト配置・画像挿入） | — |
| Phase 6 | CLI統合・クロスプラットフォームビルド | — |

## 実装ファイル対応表

| spec要件 | 実装ファイル | 役割 |
|----------|--------------|------|
| DocLayout-YOLO の ONNX 推論 | `rust/src/seg.rs` | ort で ONNX モデルをロードし、画像に対してYOLO推論を実行。検出結果を SegmentBlock に変換 |
| DocLayout-YOLO の ONNX 推論 | `rust/src/model.rs` | モデルファイルの解決（ローカル優先→HuggingFace DL→フォールバック） |
| DeepL API によるテキスト翻訳 | `rust/src/translate.rs` | reqwest で DeepL API を呼び出し。バッチ処理（短文まとめ送信）対応 |
| PDF読み込み・再構成 | `rust/src/compose.rs` | PDF読み込み、redaction、翻訳テキスト配置、画像/表/数式再配置 |
| シングルバイナリビルド | `rust/Cargo.toml` | クレート定義、依存関係、ビルドプロファイル |
| シングルバイナリビルド | `rust/src/main.rs` | clap による CLI（--input, --output, --model, --compare） |
| モデルの自動ダウンロード | `rust/src/model.rs` | hf-hub でHuggingFaceから ONNX モデルをダウンロード。キャッシュ管理 |
| 中間ファイル互換 | `rust/src/schema.rs` | SegmentPage, TranslatedPage 等をserde derive で定義。Python版JSON互換 |

## リスク・調査項目

- **ort で DocLayout-YOLO が動くか**: ONNX 形式のモデルを ort でロード・推論できるか。→ PoC Phase 1 で検証済み
- **PDF操作ライブラリの能力**: mupdf 0.8 で redaction・テキスト配置・CJKフォント描画が可能であることを確認済み
- **クロスコンパイル**: ort の Apple Silicon / Linux x86_64 ビルドが stable で通るか。GPU 非依存（CPU推論のみ）を前提とする

## constitution 準拠

- **パイプライン分離**: 4ステージを独立モジュール（seg.rs, translate.rs, compose.rs）として実装。中間JSON経由で疎結合
- **フォールバック安全性**: model.rs でモデル未検出時のフォールバック（全ページ単一textブロック）を実装。翻訳API失敗時も同様
- **シングルバイナリ配布**: Cargo.toml で単一バイナリクレートとして定義。動的リンク依存を最小化
- **レイアウト保存翻訳**: compose.rs でPython版と同等のredaction→テキスト配置→画像再配置フローを再現
- **段階的移行**: PoC Phase 1（ort 検証済み）→ Phase 2（mupdf 検証済み）→ 各ステージ順次実装。PNG直接入力で最速Go/No-Go判断
