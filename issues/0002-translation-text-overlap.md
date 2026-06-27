---
id: "0002"
title: "翻訳結果PDFで重複セグメントの訳文が同じ位置に重なって表示される"
status: open
priority: high
created: 2026-06-27
updated: 2026-06-27
tags: [bug]
---

## 概要

PDFを翻訳した結果の出力PDFで、同一テキスト領域がDocLayout-YOLOで複数のオーバーラップセグメントとして検出され、それぞれの訳文が同じ位置に配置されて重なって表示される。

## 期待される動作

翻訳後のPDFでは、元のテキストが適切にマスキング（redaction）され、翻訳テキストのみが正しい位置・サイズで読みやすく配置されるべき。
同一領域を覆う重複セグメントは配置前に間引かれ、1箇所に1つの訳文のみ配置されるべき。

## 再現手順

1. `attention_is_all_you_need.pdf` を入力として翻訳を実行する
2. 出力PDF（`translated_attention_is_all_you_need.pdf`）の p7 を確認する
3. 同ページに「学習の際には、3種類の正則化を採用しています：」と「学習中は、以下の3種類の正則化手法を採用する：」が同じ位置に重なって表示される

## 考慮・調査事項

【原因（確定）】
- 根本原因は seg 段階。`common/seg.py:316-321` の YOLO 推論は `iou=0.5` で NMS を依頼しているが、IoU≈0.77 の重複ボックスがすり抜けて両方出力される
- seg/translate/compose のいずれにも現状重複除外ロジックが存在しない（`seg.py:399-419`, `translate.py:128-166`, `compose.py:86-166`）。両セグメントが独立に翻訳・配置されて重なった
- compose の redaction（`compose.py:180-220`）は正常に動作しており、元の英語テキストは除去済み。redaction漏れではない

【採用する修正方針】
- compose 段階で防御する（`common/compose.py`）。前段の漏れを確実に吸収し、即効性・既存変更最小
- 重複判定基準: IoS（intersection / min(area_a, area_b)）≥ 0.6
- 重複時はテキスト長（source_text/translated_text）が長い方を残す

【本issueの対象外（別issue化）】
- seg の YOLO NMS が IoU≈0.77 の重複をすり抜けた根本理由（imgsz リサイズ時の座標歪み・DocLayout-YOLO内部実装等）の調査は別issueとする

## 完了条件

- [x] 文字重複が発生するPDF・条件を特定する（`attention_is_all_you_need.pdf` p7）
- [x] 重複の根本原因を特定する（seg の重複検出を前段が吸収できていない）
- [ ] `common/compose.py` に配置前の重複間引き処理（IoS≥0.6、テキスト長が長い方を残す）を実装する
- [ ] 修正後に該当ケース（`attention_is_all_you_need.pdf` p7）で重複が解消されることを確認する
- [ ] 既存の正常ケース（他ページ・他レイアウト）でデグレがないことを確認する

## メモ

- 再現対象: `attention_is_all_you_need.pdf` p7
- 重複テキスト: 「学習の際には、3種類の正則化を採用しています：」 と 「学習中は、以下の3種類の正則化手法を採用する：」
- 重複セグメント実データ（`/tmp/_attention_is_all_you_need/translated/page_007.json`）:
  - `p007_b016`: bbox=[107.4, 711.1, 333.5, 723.3], text="We employ three types of regularization during training:" (56字), conf=0.563
  - `p007_b017`: bbox=[123.4, 711.7, 309.0, 723.2], text="employ three types of regularization during training:" (53字, 先頭"We"欠落), conf=0.441
  - IoU=0.78, IoS(b017基準)=1.00 → 採用方針(IoS≥0.6)で b016 を残す
