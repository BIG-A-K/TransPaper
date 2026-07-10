---
id: "0006"
title: 固定の`/tmp/_{stem}`作業ディレクトリを一意で安全にする
status: open
priority: high
created: 2026-07-01
updated: 2026-07-01
tags: [bug, security, tmp, pipeline]
---

## 概要

Python 版と Rust 版の両方で、入力PDFの stem から `/tmp/_{stem}` という固定作業ディレクトリを作成している。既存ディレクトリを再利用するため、同名PDFの並列実行や中断後の再実行で古い `page_*.json` が混入し、誤った再構成結果や本文・訳文の漏えいにつながる可能性がある。

## 期待される動作

各実行は一意で安全な作業ディレクトリを使い、別実行の中間成果物と混ざらないべき。必要に応じてデバッグ用に中間ファイルを残せるが、既定では衝突しない設計にする。

## 再現手順

1. 同じ stem のPDFを複数回実行する
2. `/tmp/_{stem}/translated` に過去実行の `page_*.json` が残り得ることを確認する
3. `collect_translated_pages()` がディレクトリ内の `page_*.json` をまとめて収集することを確認する

## 考慮・調査事項

- Python は `tempfile.TemporaryDirectory` または `mkdtemp` 相当で 0700 の一意な作業ディレクトリを作る
- Rust は `tempfile` crate の導入または標準APIで安全な一時ディレクトリを作る
- デバッグ用途で中間ファイルを残したい場合は `--keep-temp` のような明示オプションを検討する
- `collect_translated_pages()` は今回の `seg_results` に対応するファイルだけを読む設計にできるか確認する
- シンボリックリンクや既存パスへの上書き誘導を避ける

## 完了条件

- [ ] Python 版で `/tmp/_{stem}` の固定再利用が廃止される
- [ ] Rust 版で `/tmp/_{stem}` の固定再利用が廃止される
- [ ] 同じ入力stemの並列実行で中間JSONが混入しない
- [ ] 中断後の再実行で古い `page_*.json` が結果に混ざらない
- [ ] `uv run ruff check .` と `cargo test --locked` が成功する

## メモ

- 指摘箇所: `main.py:50-51`, `common/translate.py:292`, `rust/src/main.rs:102-103`, `rust/src/translate.rs:304-310`
- 監査分類: High / security, code-quality
