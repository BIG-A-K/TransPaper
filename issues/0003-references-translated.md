---
id: "0003"
title: 論文のReferencesセクションが翻訳されてしまう
status: open
priority: medium
created: 2026-06-27
updated: 2026-06-27
tags: [bug]
---

## 概要

論文PDFの翻訳時に、Referencesセクション（参考文献一覧）まで日本語に翻訳されてしまう。参考文献は著者名・論文タイトル・ジャーナル名などの固有名詞で構成されており、翻訳すべきでない。

## 期待される動作

Referencesセクションのテキストは翻訳せず、原文のまま保持する。

## 再現手順

1. 任意の英語論文PDFを入力として翻訳を実行する
2. 出力PDFのReferencesセクションを確認する
3. 参考文献が日本語に翻訳されている

## 考慮・調査事項

- Referencesセクションの検出方法: セグメンテーション結果のブロックタイプで判定可能か、テキスト内容（"References"ヘッダー）で判定すべきか
- References以外にも翻訳不要なセクション（Appendix、Acknowledgments等）があるか検討
- セグメンテーション（seg.py）側で対応するか、翻訳（translate.py）側でフィルタするかの設計判断
- 論文によってReferencesの表記が異なる（"References", "Bibliography", "REFERENCES"等）

## 完了条件

- [ ] Referencesセクションのテキストが翻訳されずに原文のまま出力される
- [ ] 他のセクション（本文、キャプション等）の翻訳に影響しない
