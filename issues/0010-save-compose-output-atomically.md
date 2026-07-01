---
id: "0010"
title: Python compose失敗時に部分PDFを保存しない
status: open
priority: high
created: 2026-07-01
updated: 2026-07-01
tags: [bug, compose, reliability]
---

## 概要

Python 版の `common/compose.py` は、PDF再構成処理中に例外が発生しても `finally` で `out_doc.save(output_path)` を実行する。途中まで処理された不完全なPDFが保存され、既存の正常な出力を壊す可能性がある。

## 期待される動作

compose が正常完了した場合だけ出力PDFを確定し、失敗時には既存の出力ファイルを破壊しないべき。可能なら一時ファイルに保存してから atomic replace する。

## 再現手順

1. `common/compose.py` の `compose_pdf()` を確認する
2. ページ処理を含む `try` ブロックの `finally` で `out_doc.save(output_path)` が必ず呼ばれることを確認する
3. 処理途中で例外が起きても部分PDF保存が走り得ることを確認する

## 考慮・調査事項

- 正常完了後だけ `save()` する構造に変更する
- 既存出力を守るため、一時ファイルへ保存してから `replace` する
- 例外時に一時ファイルが残る場合の扱いを決める
- Rust 版の保存挙動も同様の問題がないか確認する
- 失敗ケースのテストを追加できるか検討する

## 完了条件

- [ ] Python compose の途中失敗時に `output_pdf` が更新されない
- [ ] 正常完了時のみ出力PDFが確定される
- [ ] 可能なら一時ファイル経由の安全な置換になっている
- [ ] 失敗時の挙動を検証するテストまたは手動確認手順がある
- [ ] `uv run ruff check .` が成功する

## メモ

- 指摘箇所: `common/compose.py:174-176`
- 監査分類: High / code-quality, reliability
