---
id: "0008"
title: DocLayoutラベル分類で`title`と未知テキストを翻訳対象にする
status: open
priority: high
created: 2026-07-01
updated: 2026-07-01
tags: [bug, segmentation, translation]
---

## 概要

DocLayout-YOLO の `title` ラベルと未知ラベルが、Python 版・Rust 版の両方で `math` に分類される。翻訳処理は `text` と `caption` のみを対象にするため、論文タイトルや未知のテキスト系領域が翻訳対象から外れる可能性がある。

## 期待される動作

論文タイトルやテキスト系の未知ラベルは、少なくとも翻訳対象から不自然に除外されないべき。Python 版と Rust 版、および `docs/seg.md` のラベルマッピング契約が一致しているべき。

## 再現手順

1. `common/seg.py` の `DOC_LAYOUT_MATH_CLASSES` に `title` が含まれることを確認する
2. `_map_doclayout_label()` の未知値フォールバックが `math` であることを確認する
3. `rust/src/seg.rs` でも同じ分類になっていることを確認する
4. `translate` が `text` と `caption` のみ翻訳対象にしていることを確認する

## 考慮・調査事項

- `title` は `text` 系に移すのが妥当か確認する
- 未知ラベルの既定値を `text` にするか、明示的な `unknown` を導入するか決める
- Python 版と Rust 版の分類を同時に修正する
- `docs/seg.md` の「上記以外 → text」という説明と実装を一致させる
- ラベルマッピングの単体テストを追加する

## 完了条件

- [ ] Python 版で `title` が翻訳対象の分類になる
- [ ] Rust 版で `title` が翻訳対象の分類になる
- [ ] 未知ラベルのフォールバック方針が実装とドキュメントで一致する
- [ ] ラベルマッピングの回帰テストが追加される
- [ ] `uv run ruff check .` と `cargo test --locked` が成功する

## メモ

- 指摘箇所: `common/seg.py:60`, `common/seg.py:124-128`, `rust/src/seg.rs:23`, `rust/src/seg.rs:69-75`
- 監査分類: High / contract, code-quality
