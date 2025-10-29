# 翻訳モジュール（`common/translate.py`）

`common/translate.py` は翻訳バックエンドを統一した関数インターフェースで提供し、`main.py` から呼び出してセグメント化済みの PDF を翻訳します。現在は DeepL API と Hugging Face/MarianMT モデルに対応しています。


実行すると以下のステップを踏みます。
1. `common.seg.segment_pdf` で PDF をセグメント化し、`out/_<PDFファイル名>/seg/` 以下に JSON とオーバーレイ PNG を保存します。
2. セグメント JSON (`page_xxx.json`) を読み込み、`type` が `text` または `caption` のブロックを翻訳します。`math` ブロックは翻訳せず、座標情報だけをそのまま後段へ渡すことで再構成時に原稿から切り出した数式イメージを再配置します。
3. 翻訳結果をページ単位の JSON (`translate/page_xxx.json`) と、全体をまとめた `translate/document_translation.json` に書き出します。出力形式は `common/schema.py` の `TranslatedPage` / `TranslationSegment` に準拠しています。

## DeepL で翻訳する

`transmod.translate_deepl(text, target_lang, auth_key)` を使用します。`main.py` では例外処理を行い、認証キーが設定されていない場合は `click.ClickException` を発生させます。DeepL Free/Pro のどちらのキーでも利用できます。

```python
from common import translate as transmod

translated = transmod.translate_deepl(
    "サンプルテキスト",
    target_lang="EN",
    auth_key=os.environ["DEEPL_API_KEY"],
)
```

## Hugging Face モデルで翻訳する
<!-- TODO：実装 -->
MarianMT の事前学習済みモデルを利用したローカル翻訳です。初回実行時はモデルをダウンロードする必要があり、ネットワークアクセスと PyTorch の実行環境が必要です。

```python
translated = transmod.translate_huggingface(
    "Translate me",
    model_name="Helsinki-NLP/opus-mt-ja-en",
)
```

## openAI形式のAPIを使用する
<!-- TODO：実装 -->
LLM APIを用いた翻訳も実装を予定しています。
groqAPIは無料で使えるので、早く実装したいなと思っています。

## 出力ディレクトリ構造

```
out/
  _attention_is_all_you_need/
    seg/
      page_001.json
      page_001_seg.png
      ...
    translate/
      page_001.json
      document_translation.json
      ...
```

`document_translation.json` は `common/compose.compose_pdf` が受け取る形式であり、後段の PDF 再組版処理でそのまま利用できます。
