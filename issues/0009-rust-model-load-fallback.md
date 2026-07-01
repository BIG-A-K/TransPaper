---
id: "0009"
title: Rust版でONNXモデルロード失敗時もフォールバックする
status: open
priority: high
created: 2026-07-01
updated: 2026-07-01
tags: [bug, rust, segmentation]
---

## 概要

Rust 版では、ローカルまたは HuggingFace キャッシュ上の ONNX モデルが見つかった場合に `create_session(mp)?` を実行する。モデルファイルが壊れている、ONNX Runtime がロードできない、環境依存でセッション作成に失敗する場合、フォールバックせずパイプライン全体が停止する。

## 期待される動作

外部リソースやモデルロードが失敗しても、constitution の「フォールバック安全性」に従い、警告を出して `fallback:text-full-page` で処理を継続するべき。

## 再現手順

1. `rust/src/seg.rs` の `segment_pdf()` を確認する
2. `ModelSource::Local` または `ModelSource::HuggingFace` の場合に `Some(create_session(mp)?)` でエラー伝播していることを確認する
3. Python 版はモデルロード失敗時にフォールバックすることを確認する

## 考慮・調査事項

- `create_session()` 失敗を捕捉し、`ort_session = None` として処理を継続する
- 警告ログにモデルパスと失敗理由を出す
- モデル未取得時の `ModelSource::Fallback` と同じ動作になるよう統一する
- 壊れたモデルファイルを用いた回帰テストまたは手動検証方法を用意する

## 完了条件

- [ ] Rust 版でONNXセッション作成失敗時にパイプラインが停止しない
- [ ] 失敗理由が警告として確認できる
- [ ] フォールバック時に `doclayout_model` が `fallback:text-full-page` 相当として記録される
- [ ] `cargo test --locked` が成功する
- [ ] 必要なら `docs/seg.md` またはRust関連ドキュメントにフォールバック条件が追記される

## メモ

- 指摘箇所: `rust/src/seg.rs:321-323`, `specs/constitution.md:8`
- 監査分類: High / code-quality
