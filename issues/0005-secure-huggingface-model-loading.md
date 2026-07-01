---
id: "0005"
title: HuggingFaceモデル自動取得と`.pt`ロードを安全化する
status: open
priority: high
created: 2026-07-01
updated: 2026-07-01
tags: [bug, security, model, huggingface]
---

## 概要

`common/seg.py` が HuggingFace Hub から DocLayout-YOLO の `.pt` モデルを自動取得し、そのまま YOLO ローダへ渡している。`.pt` ロード経路は pickle 系の任意コード実行リスクを含むため、モデル配布元・通信経路・ローカルキャッシュが汚染された場合の影響が大きい。

## 期待される動作

モデル取得・ロードは、信頼済みの形式と検証済みの成果物だけを扱うべき。自動取得する場合も、revision とハッシュを固定し、意図しないモデル差し替えを検出できるべき。

## 再現手順

1. `common/seg.py` の `DocLayoutYoloSegmenter._install_doclayout_yolo()` を確認する
2. `hf_hub_download(..., local_files_only=False)` で `.pt` を取得していることを確認する
3. 取得したパスを `_load_local()` が `loader(str(path))` に渡していることを確認する

## 考慮・調査事項

- `.pt` 自動ロードを廃止し、ONNX または safetensors 等の安全な形式へ寄せられるか確認する
- HuggingFace Hub を使い続ける場合は `revision` と SHA256 を固定する
- Rust 版は ONNX を使っているため、Python 版も同じ配布形式に寄せられるか確認する
- 既存のフォールバック動作を壊さず、モデル取得に失敗した場合は従来どおり `fallback:text-full-page` へ縮退する
- README/docs にモデル取得ポリシーとローカルモデル指定方法を明記する

## 完了条件

- [ ] Python 版で未固定の `.pt` 自動取得・ロードが行われない
- [ ] 自動取得を残す場合は revision と SHA256 検証が入っている
- [ ] モデル取得・検証失敗時にフォールバックで処理を継続できる
- [ ] `docs/seg.md` または関連ドキュメントに安全なモデル配置・取得方法が記載される
- [ ] `uv run ruff check .` が成功する

## メモ

- 指摘箇所: `common/seg.py:244`, `common/seg.py:265-269`
- 監査分類: High / security
