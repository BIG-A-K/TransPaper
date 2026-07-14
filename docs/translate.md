# 翻訳モジュール（`common/translate.py`）

`common/translate.py` は翻訳バックエンドを統一した関数インターフェースで提供し、`main.py` から呼び出してセグメント化済みの PDF を翻訳します。現在は DeepL API、Ollama経由のローカルLLM（Gemma系など）、Hugging Face/MarianMT モデルに対応しています。


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

## Ollama経由のローカルLLMで翻訳する

Ollamaが提供するローカルLLM（Gemma系など）を使った翻訳です。APIキー不要でオフライン実行できますが、事前にモデルの取得とサーバー起動が必要です。

### 準備

```sh
# 使いたいモデルを取得（例: Gemma系）
ollama pull gemma3:4b

# Ollamaサーバーを起動（別プロセスで常時起動しておく）
ollama serve
```

### 実行

モデル名は `--model ollama:<model>` 形式で指定します。`ollama:` のあとに `ollama list` で表示されるモデル名をそのまま書きます。

```sh
uv run main.py --input hoge.pdf --model ollama:gemma3:4b
uv run main.py --input hoge.pdf --model ollama:gemma3:12b
```

### エンドポイントの変更

Ollamaサーバーが別ホストで動いている場合は `OLLAMA_HOST` 環境変数でベースURLを指定できます（既定: `http://localhost:11434`）。

```sh
OLLAMA_HOST=http://host.docker.internal:11434 uv run main.py --input hoge.pdf --model ollama:gemma3:4b
```

### 高速化の仕組み

Ollama翻訳は以下の3つを組み合わせて高速化しています（順序は常に保持されます）。

1. **バッチプロンプト化** … キャプション・見出しなど短いテキスト（既定300文字以下）を1つのプロンプトに束ね、1リクエストでまとめて翻訳します。リクエスト数が大幅に減り、通信・推論のセットアップオーバーヘッドを償却できます。出力はJSON構造化出力で件数と順序を強制し、崩れた場合は自動で1テキストずつ再翻訳します。長いテキストは従来通り1テキスト1リクエストです。
2. **並列リクエスト** … バッチ化した「ジョブ」群を `ThreadPoolExecutor` で並列実行します。**真の並列推論にするにはサーバー側で `OLLAMA_NUM_PARALLEL` を上げて起動**する必要があります。
3. **推論パラメータの最適化** … `temperature=0` で決定的に翻訳し、入力長から `num_predict`（生成トークン上限）を見積もることで、LLM暴走時の無駄な生成時間を抑えます。

### ワーカー数（並列リクエスト数）の指定

並列リクエスト数は未指定時に自動で決定します。優先順位:

1. 環境変数 `TRANSPAPER_NUM_WORKERS`
2. 環境変数 `OLLAMA_NUM_WORKERS`
3. CPU論理コア数（min 2, max 8 でクランプ）

```sh
# サーバー側: 並列スロット数を指定して起動（GPUメモリが許す限り）
OLLAMA_NUM_PARALLEL=8 ollama serve

# クライアント側の同時リクエスト数を明示的に指定する場合
TRANSPAPER_NUM_WORKERS=8 uv run main.py --input hoge.pdf --model ollama:gemma3:4b
```

- `OLLAMA_NUM_PARALLEL`（サーバー）とクライアントのワーカー数を合わせるのが目安です
- クライアント側をサーバー側より大きくしても意味がないので、GPUメモリが足りない場合は両方とも `2` などに下げてください
- ローカルLLMのボトルネックは生成（デコード）本体なので、並列化の恩恵は最大でも数倍程度です

### バッチプロンプト化のチューニング

短いテキストを束ねる閾値・上限は `common/translate.py` の以下の定数で調整できます（環境変数からの変更は未対応）。

| 定数 | 既定値 | 役割 |
| --- | --- | --- |
| `OLLAMA_BATCH_CHAR_THRESHOLD` | `300` | この文字数以下のテキストをバッチ候補にする |
| `OLLAMA_BATCH_MAX_ITEMS` | `16` | 1バッチに含めるテキスト数の上限 |
| `OLLAMA_BATCH_MAX_CHARS` | `3000` | 1バッチの入力合計文字数の上限 |

バッチが大きいほどリクエスト数は減りますが、1リクエストの生成時間が長くなり、件数・順序が崩れやすくなるトレードオフがあります。

### 注意点

- **遅さ**: ローカルLLM自体がDeepLより大幅に遅いです。モデルサイズが大きいほど顕著です（上記「並列化」で緩和可能）。
- **品質**: LLMは「自然な訳」より「要約・言い換え」に寄ることがあり、数式・引用・節番号が崩れる場合があります。プロンプトで保持を指示していますが要確認です。
- **モデル未導入時**: 指定モデルが未pullの場合、原因が分かるエラー（`ollama pull <model>` を促す）を出します。
- **サーバー未起動時**: 接続エラー時に `ollama serve` を促すメッセージを出します。
- Docker開発環境からホストのOllamaを叩く場合は `OLLAMA_HOST=http://host.docker.internal:11434` を指定してください。

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

## 文中数式プレースホルダー

`common.seg` が検出した文中数式は、翻訳入力では `[[TRANSPAPER_INLINE_MATH_0001]]` のようなプレースホルダーになっています。DeepL・idx・Ollamaのすべての経路で同じ保護済みテキストを渡します。Ollamaにはプレースホルダーを1文字も変更せず、同じ位置に1回だけ返すよう明示しています。

翻訳結果を保存する際は、各プレースホルダーがちょうど1回残っているか検証します。

- 正常: `inline_math_status` を `preserved` にして訳文を保存
- 欠落・重複・改変: `inline_math_status` を `fallback_source` にし、プレースホルダーを含む保護済み原文へフォールバック
- フォールバック時: `translation_warnings` に理由を記録し、composeの警告として参照可能

プレースホルダーの位置を確定できない壊れ方では、誤った位置へ数式を配置するより原文を残すことを優先します。外部TeXレンダラーは使用しません。

`collect_translated_pages()` は `inline_math`、`inline_math_status`、`translation_warnings` を `TranslationSegment` へ伝播します。

Rust版の `rust/src/translate.rs` も同じ検証とフォールバックを行い、Python版と同一の中間JSONを扱えます。
