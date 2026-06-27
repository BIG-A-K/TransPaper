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

## 比較PDF作成

`create_comparison_pdf` 関数を使用すると、元のPDFと翻訳後のPDFを交互に配置した比較用PDFを作成できます。

### CLI経由での使用

```bash
uv run main.py --input file.pdf --compare
# または短縮形
uv run main.py -i file.pdf -c
```

`--compare`/`-c` オプションを指定すると、ページ順が「元→翻訳→元→翻訳...」となる比較PDFが生成されます。デフォルトのファイル名は `translated_{ファイル名}_compare.pdf` になります。

### モジュールを直接呼び出す

```python
from common import compose

comparison_path = compose.create_comparison_pdf(
    original_pdf="attention_is_all_you_need.pdf",
    translated_pdf="translated_attention_is_all_you_need.pdf",
    output_pdf="comparison.pdf",
)
print(f"Comparison PDF created: {comparison_path}")
```

この関数は元のPDFと翻訳後のPDFのページ数が一致していることを確認し、不一致の場合はエラーを発生させます。

## 注意点

- ORIGINAL PDF をベースとして複製し、既定ではテキスト層のみ削除した後に訳文を描画します。画像や図表は原文のまま残ります。
- `math` セグメントは原稿PDFから該当領域をクリップした画像として再配置されるため、`cover_original=True` でも数式が欠落しません（翻訳対象には含まれません）。
- 行ボックスからはみ出す長さの訳文は自動的にフォントサイズを縮小します。それでも収まらない場合は一部が切り落とされ、警告が発生します。
- 日本語描画にはCJK対応フォントが必要です。システムにインストール済みの`Noto Sans CJK`系フォントなどを指定してください。
- 比較PDF作成時は、元のPDFと翻訳後のPDFのページ数が完全に一致している必要があります。

## 重複セグメントの間引き

前段（seg/translate）で同一テキスト領域が複数のオーバーラップセグメントとして検出されると、訳文が同じ位置に重なって描画されることがあります。`compose_pdf` は配置前にテキストセグメントの重複を間引き、1箇所に1つの訳文のみ配置します。

- 判定基準: IoS（共通面積 / 小さい方の面積）≥ `dedup_ios_threshold`（既定 0.6）
- 残す方: `translated_text`（未設定なら `source_text`）が長い方。同長の場合は登場順が早い方
- 対象: 実際にテキスト配置されるセグメントのみ（`image`/`table`/`math`・空テキスト・`target_types` 外はそのまま残します）
- 間引いたセグメントは `warnings` に `重複セグメントを間引きました ...` として記録されます
- **巨大 bbox（ページ面積の40%超）の2パス方式**: seg の異常検出でページ全体を覆う巨大 bbox が生成されることがあります（参考文献ページ等で個別エントリを1つの巨大ブロックにまとめてしまう等）。このような巨大 bbox をそのまま「長い方優先」で残すと個別セグメントを全て吸収してレイアウトが崩壊するため、**面積がページ面積の40%を超えるセグメントは『異常検出』とみなし処理順を後回し**にします（閾値 `0.4` は固定値）。先に通常セグメント（≤40%）を長い方優先で確定させたあと、巨大セグメントを accepted と比較して重複（IoS≥閾値）すれば間引きます。これにより通常の重複では長い方を残し、巨大 bbox では個別セグメントを残す、という挙動になります（p11 の参考文献ページ等で効果を発揮）。

`ComposeOptions` で制御できます:

```python
from common import compose

result = compose.compose_pdf(
    original_pdf="attention_is_all_you_need.pdf",
    translated_pages="out/.../document_translation.json",
    output_pdf="out/.../translated.pdf",
    options=compose.ComposeOptions(
        dedup_enabled=True,          # 既定: True
        dedup_ios_threshold=0.6,     # 既定: 0.6。0 以下で無効化
    ),
)
```

前段の重複検出漏れを吸収する最終防衛栏として働くため、既定で有効です。重複判定を厳しくしたい場合は閾値を上げ（例: 0.8）、緩くしたい場合は下げてください。

### Rust版での扱い

Rust版（`rust/src/compose.rs`）も Python版と同じ重複間引きロジックを備えます（機能パリティ）。ただし設定方法が異なります:

- 閾値・有効/無効は **コンパイル時定数**（`pub const`）:
  - `compose::DEDUP_ENABLED: bool = true`
  - `compose::DEDUP_IOS_THRESHOLD: f32 = 0.6`
- CLI からは `--no-dedup` フラグのみで無効化できます（閾値の実行時変更は非サポート）:

```bash
# 既定（間引き有効）で実行
./rust/target/release/transpaper --input paper.pdf --output translated.pdf

# 間引きを無効化（従来動作）
./rust/target/release/transpaper --input paper.pdf --output translated.pdf --no-dedup
```

- 文字列長の比較は Python版と同じ「文字数（コードポイント数）」で統一（Rust は `str::chars().count()`）。日本語でも両実装で同一の判定結果になります。
- 间引きタイミングは Python版と同じく `strip_page_text`（原文 redaction）の後・テキスト配置前。間引かれたセグメントは原文 redaction 済み（白塗り枠）＋訳文なしで配置されます。
- 閾値を変更したい場合は `rust/src/compose.rs` の const を編集してリビルドしてください。
