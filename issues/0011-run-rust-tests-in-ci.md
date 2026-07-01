---
id: "0011"
title: Rustテストを通常CIで実行する
status: open
priority: high
created: 2026-07-01
updated: 2026-07-01
tags: [test, ci, rust]
---

## 概要・目的

Rust 版には `rust/src/compose.rs` 内に unit test が存在し、手元の監査では `cargo test --locked` が 16 passed で成功した。一方、通常のPR/Push用CIでは `uv sync` と `make test` のみが実行され、Rustのテストが走っていない。

## 考慮・調査事項

- `.github/workflows/test.yml` に Rust toolchain セットアップと `cargo test --locked` を追加する
- 必要なら `cargo build --locked` も追加し、コンパイル破損をPR段階で検出する
- `release.yml` の build 前にも test を入れるか検討する
- CI時間とキャッシュ設定を確認する
- `Makefile` はAGENTSで編集禁止のため、必要ならworkflow側で直接実行する

## 完了条件

- [ ] PR/Push用CIで `cargo test --locked` が実行される
- [ ] 既存のRust unit tests 16件がCIで成功する
- [ ] Rustのコンパイル破損がrelease前に検出できる
- [ ] Python側の既存CI挙動を壊さない

## 実装するテストのシナリオ

- `rust/` を working directory として `cargo test --locked` を実行する
- 可能なら `cargo build --locked` も実行する
- キャッシュを使う場合は `Swatinem/rust-cache` 等を検討する
