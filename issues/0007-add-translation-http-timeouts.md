---
id: "0007"
title: 翻訳HTTP API呼び出しにtimeoutと有限リトライを追加する
status: open
priority: high
created: 2026-07-01
updated: 2026-07-01
tags: [bug, reliability, translation]
---

## 概要

DeepL API 呼び出しに明示的な timeout がなく、Python 版では例外時に `translate_deepl()` を再帰的に呼ぶフォールバックになっている。API遅延・障害・4xx/5xx 継続時に処理が長時間停止したり、リクエストが増幅して quota を浪費したりする可能性がある。

## 期待される動作

外部HTTP API呼び出しは、connect/read timeout、ステータス検証、有限回リトライ、ユーザーが原因を理解できるエラーメッセージを持つべき。

## 再現手順

1. `common/translate.py` の `requests.post()` を確認する
2. DeepL 経路に `timeout` と `raise_for_status()` がないことを確認する
3. 例外時に `translate_deepl(text, ...)` を再帰的に呼んでいることを確認する
4. `rust/src/translate.rs` の DeepL `Client::new()` が timeout 未設定であることを確認する

## 考慮・調査事項

- Python と Rust の DeepL 経路で timeout 方針を揃える
- 4xx は即時失敗、429/5xx/一時的ネットワーク障害のみ有限リトライする
- リトライには指数 backoff と最大回数を設定する
- Python の再帰フォールバックは非再帰の個別処理に置き換える
- Ollama 経路の timeout との整合性も確認する

## 完了条件

- [ ] Python DeepL 呼び出しに明示的な timeout とステータス検証が入る
- [ ] Rust DeepL 呼び出しに明示的な timeout が入る
- [ ] リトライは有限回で、4xx を無駄に再試行しない
- [ ] Python 版の再帰フォールバックが廃止される
- [ ] APIエラー時に原因が分かるメッセージが返る
- [ ] `uv run ruff check .` と `cargo test --locked` が成功する

## メモ

- 指摘箇所: `common/translate.py:35`, `common/translate.py:38-41`, `rust/src/translate.rs:29-38`
- 監査分類: High / code-quality, reliability
