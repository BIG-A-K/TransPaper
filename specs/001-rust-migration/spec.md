# Spec: rust-migration

## 概要

TransPaper の処理パイプラインを Rust で再実装し、シングルバイナリとして配布可能にする。エンドユーザーの導入障壁（Python環境・uv・依存パッケージのセットアップ）を排除する。

## 動機

現在の Python 実装は動作するが、利用には Python 環境の構築が必要で非エンジニアにとってハードルが高い。Rust で再実装しシングルバイナリ化することで、ダウンロード即利用を実現する。

元issue: issues/0001-rust-migration.md

## 要件差分

### ADDED（新規追加）

- [x] DocLayout-YOLO の ort 推論: Rust + ort (ONNX Runtime) で DocLayout-YOLO の ONNX モデルを読み込み、PDF各ページの画像に対してレイアウト要素検出（Segmentation）を実行する。Python版 seg.py と同等の出力（SegmentBlock リスト with bbox, type, meta）を生成する。**candle-onnx がオペレーター未対応のため ort に変更。**
- [ ] DeepL API によるテキスト翻訳: Rust の HTTP クライアントで DeepL API を呼び出し、テキスト要素を日本語に翻訳する。短いテキストをまとめて送信するバッチ処理に対応する。
- [ ] PDF読み込み・再構成（Composition）: Rust で PDF を読み込み、原文テキストの redaction、翻訳テキストの配置、画像/表/数式領域の再配置を行い、翻訳済み PDF を出力する。PDF操作ライブラリはPoCで選定する。
- [ ] シングルバイナリとしてのクロスプラットフォームビルド: macOS（Apple Silicon / x86_64）および Linux（x86_64）向けにシングルバイナリをビルドし配布可能にする。CLI は Python 版と同等のオプション（--input, --output, --model, --compare）を提供する。
- [ ] モデルの自動ダウンロード: DocLayout-YOLO モデルファイルが存在しない場合、初回起動時に HuggingFace Hub から自動ダウンロードする。ローカルキャッシュ優先、見つからなければダウンロード、それも失敗すればフォールバック動作に移行する。

### MODIFIED（既存変更）

なし（Python版はそのまま維持）

### REMOVED（削除・破壊的）

なし

## 影響範囲

- 新規 Rust プロジェクト（リポジトリ内に追加、または別リポジトリ）
- Python版の既存コード（common/, main.py）には変更なし
- CI/CD: Rust ビルド・テストの追加が必要

## constitution との整合性

| 原則 | 適合 | 要更新 |
|------|------|--------|
| パイプライン分離 | ✓ | |
| フォールバック安全性 | ✓ | |
| シングルバイナリ配布 | ✓ | |
| レイアウト保存翻訳 | ✓ | |
| 段階的移行 | ✓ | |

全原則に適合。constitution の更新は不要。

## Clarifications

`clarify` フェーズで質問と回答を1組ずつ追記する。最初から明確なら空のままでよい。

| Q | A（推奨案含む） |
|---|---|
| Rustプロジェクトの配置場所は同一リポジトリか別リポジトリか？ | 同一リポジトリ内に `rust/` ディレクトリを作成して配置する。Python版と共存させ、issue・specs・docsを共有する |
| DocLayout-YOLOのモデル形式は？ | ONNX形式を使用。`wybxc/DocLayout-YOLO-DocStructBench-onnx` から取得。当初safetensors/candle予定だったがcandle-onnxのオペレーター制約によりort+ONNXに変更 |
| PoCのGo/No-Go判断基準は？ | attention_is_all_you_need.pdfを入力としてPython版とRust版の出力を目視比較し、検出されるレイアウト要素の種類・位置が概ね一致すればGo。完全一致は不要 |
| 中間ファイル形式はPython版と互換にするか？ | 同じJSON形式にする。パイプライン分離の原則に従い、Python版・Rust版の混在運用やデバッグ時の比較を可能にする |
