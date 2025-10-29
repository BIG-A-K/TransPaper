# 再構成モジュール (`common.compose`)

`common.compose` は翻訳済みセグメントを元のPDF上に再配置し、訳文付きPDFを生成します。既定では原文PDFからテキストコンテンツを削除（画像・図は維持）したうえで訳文のみを配置するため、英語の文字が透けて見える現象を防げます。セグメントの座標(`bbox`)とレイアウト情報をそのまま用いるため、`main.py`の翻訳結果(JSON)を入力として扱う想定です。

## 使い方

### パイプラインから実行

翻訳後に`--compose`フラグを付けると再構成PDFが`out/<PDF名>/compose/translated.pdf`として出力されます。

```bash
uv run main.py attention_is_all_you_need.pdf --compose \
  --compose-font-path /usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc
```

主なオプション:

- `--compose-font-path`: 日本語を含むフォント(TTF/TTC)。指定しない場合はBase14フォント(`helv`)が使われるため、多言語では文字化けします。
- `--compose-font-scale`: 元の平均フォントサイズに乗算するスケール係数 (既定=1.0)。
- `--compose-padding`: 原文テキストを除去する際に余白として広げる量 (pt)。
- `--compose-no-cover`: 原文テキストを残したまま訳文を重ねたい場合に指定。

### モジュールを直接呼び出す

```python
from common import compose

result = compose.compose_pdf(
    original_pdf="attention_is_all_you_need.pdf",
    translated_pages="out/attention_is_all_you_need/trans/document_translation.json",
    output_pdf="out/attention_is_all_you_need/compose/translated.pdf",
    options=compose.ComposeOptions(
        font_path="/path/to/NotoSansCJK-Regular.ttc",
        cover_original=True,
    ),
)
print(result.page_count, result.segment_count, result.warning_count)
```

`ComposeOptions` ではフォントや最小フォントサイズ、行間、警告表示件数などを調整できます。`cover_original=True` で原文テキストを削除（既定値）、`False` で原文を保持します。`warning_count` は全警告件数、`warnings` は上位 `max_logged_warnings` 件のみが格納されます。

## 注意点

- ORIGINAL PDF をベースとして複製し、既定ではテキスト層のみ削除した後に訳文を描画します。画像や図表は原文のまま残ります。
- `math` セグメントは原稿PDFから該当領域をクリップした画像として再配置されるため、`cover_original=True` でも数式が欠落しません（翻訳対象には含まれません）。
- 行ボックスからはみ出す長さの訳文は自動的にフォントサイズを縮小します。それでも収まらない場合は一部が切り落とされ、警告が発生します。
- 日本語描画にはCJK対応フォントが必要です。システムにインストール済みの`Noto Sans CJK`系フォントなどを指定してください。
