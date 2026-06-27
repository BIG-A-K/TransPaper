---
id: "0001"
title: "Rust実装への移行検討（バイナリ配布の実現）"
status: open
priority: medium
created: 2026-06-27
updated: 2026-06-27
tags: [enhancement, research]
---

## 背景・概要

現在TransPaperはPythonで実装されており、実行には Python 環境・uv・依存パッケージのセットアップが必要。
Rustで再実装すればシングルバイナリとして配布でき、エンドユーザーの導入障壁を大幅に下げられる可能性がある。

Rustエコシステムには candle（HuggingFace公式のRust MLフレームワーク）があり、HuggingFaceモデルやYOLOの推論をRustネイティブで実行できる。

## 仕様・要件

- 現在の4ステージパイプライン（Segmentation → Translation → Collection → Composition）をRustで再実装する
- DocLayout-YOLO による推論を candle 経由で実行する
- DeepL API 呼び出しをRust HTTPクライアントで置き換える
- PDF読み込み・書き込み（現在 PyMuPDF）に相当するRustライブラリを選定する
- シングルバイナリとしてクロスプラットフォーム（macOS / Linux）配布を実現する

## 考慮・調査事項

- candle で DocLayout-YOLO モデル（ONNX or PyTorch形式）を正しく推論できるか検証が必要
- PyMuPDF相当のPDF操作（redaction、TextWriter、画像挿入）がRustライブラリで実現可能か（候補: lopdf, pdf-rs, mupdf-rs）
- モデルファイルのバンドル方法（バイナリ同梱 vs 初回起動時DL）
- Python版との機能パリティをどこまで求めるか
- 移行期間中のPython版との並行メンテナンスコスト
- ユーザーが「castle」と言及しているが candle の可能性あり。正確なライブラリ名を確認する

## 完了条件

- [ ] candle で DocLayout-YOLO 推論が動作することを PoC で確認
- [ ] RustでのPDF操作（読み込み・redaction・テキスト配置・画像挿入）の実現可能性を検証
- [ ] PoC結果を踏まえた移行判断（Go / No-Go）を決定
- [ ] Go判断の場合、段階的移行計画を策定
