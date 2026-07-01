---
id: "0004"
title: Ollama経由のローカルLLM翻訳バックエンドを追加する
status: open
priority: medium
created: 2026-07-01
updated: 2026-07-01
tags: [enhancement, translation, ollama, local-llm]
---

## 背景・概要

このリポジトリの翻訳処理は実質的にDeepL前提になっており、DeepL以外の翻訳方法を検討・実装できる余地が小さい。DeepL APIに依存しない選択肢として、Ollama経由でGemma系などのローカルLLMを使える翻訳バックエンドを追加する。

## 仕様・要件

- `--model ollama:<model>`形式でOllama上の任意モデルを指定できるようにする
- 例: `uv run main.py --input paper.pdf --model ollama:gemma4`
- OllamaのHTTP API（既定: `http://localhost:11434`）を利用して翻訳する
- 翻訳対象は既存と同じく`text`および`caption`ブロックとする
- 数式、引用、節番号、インラインコード、専門用語を極力保持するプロンプトを用意する
- 既存の`deepl`および`idx`の挙動を壊さない

## 考慮・調査事項

- `gemma4`というモデル名がOllamaで実際に利用可能か、または`gemma3:4b`等の既存モデル名を使うべきか確認する
- 1セグメントずつ翻訳する実装は堅牢だが遅いため、将来的にバッチ翻訳できるか検討する
- LLMが余計な説明文、Markdown、JSON崩れ、要約を返さないようプロンプトとレスポンス検証を設計する
- Ollamaサーバー未起動、モデル未pull、タイムアウト時のエラーメッセージを分かりやすくする
- Python版で先に実装し、Rust版にも同じ`ollama:`モデル指定仕様を導入するか検討する
- 翻訳バックエンド抽象化（DeepL/Ollama/HuggingFace/OpenAI互換API）をどの粒度で行うか検討する

## 完了条件

- [ ] Python版で`--model ollama:<model>`を指定してOllama経由の翻訳が実行できる
- [ ] `deepl`と`idx`の既存動作が維持される
- [ ] Ollama未起動またはモデル未存在時に原因が分かるエラーを出す
- [ ] `docs/translate.md`にOllama/Gemma系ローカルLLM翻訳の使い方と注意点が追記される
- [ ] 可能なら`attention_is_all_you_need.pdf`で`idx`およびOllama分岐の動作確認を行う

## メモ

- 現在の環境には`ollama`コマンドは存在するが、`gemma4`モデルは未導入
- 現在確認できたOllamaモデルは`nomic-embed-text:latest`のみ
