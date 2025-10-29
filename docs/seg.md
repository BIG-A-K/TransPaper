# セグメント抽出モジュール（`common/seg.py`）

DocLayout-YOLO でレイアウト解析を行い、PDF の各ページからテキスト・図・表・キャプションなどの領域を検出するモジュールです。検出結果は JSON とオーバーレイ付き PNG として保存され、後段の翻訳・再構成処理で利用されます。

## 依存関係
- PyMuPDF (`fitz`): PDF のレンダリングとテキスト抽出に使用します。
- ultralytics / torch / torchvision / numpy / Pillow / opencv-python: DocLayout-YOLO 推論に必要です。リポジトリでは `uv add ultralytics torch torchvision` 済み。
- DocLayout-YOLO は既定で `YOLOv10.from_pretrained("juliozhao/DocLayout-YOLO-DocStructBench")` を呼び出します。オフライン環境では `--model-path` や `DOCLAYOUT_YOLO_MODEL` でローカル重み (`*.pt`) を指定できます。

## PDF画像化ユーティリティ
`pdf2images(pdf_path, outdir)` は PyMuPDF を使って PDF を 300DPI 相当で PNG 化し、生成ファイルのパス一覧を返します。`segment_pdf` 以外でもページ画像だけが欲しい場合に再利用できます。

```python
from common.seg import pdf2images

image_paths = pdf2images("paper.pdf", "out/pages")
# -> ["out/pages/paper_page_001.png", ...]
```

主な挙動:
- 存在しないパスを指定すると `FileNotFoundError` を送出。
- 既存の出力ディレクトリが無い場合は自動作成。
- ページ番号は 1 始まりでゼロ埋め（3 桁）になります。

## 処理の流れ
1. PyMuPDF でページを DPI 指定で画像化し、`numpy.ndarray` に変換します。
2. DocLayout-YOLO でレイアウト領域を検出します。
3. 検出したバウンディングボックスを PDF 座標系（pt）に変換し、カテゴリに応じて `SegmentBlock` を作成します。
   - YOLO のラベルを `text` / `caption` / `table` / `image` / `math` にマッピング。
   - `text` と `caption` は該当領域からテキストを再抽出し、文字数やフォント統計を `meta` に格納します。
   - 元の YOLO ラベルと信頼度 (`confidence`) も `meta` として保持します。
4. Pillow でオーバーレイ PNG を描画し、境界線とラベル（信頼度付き）を描き込みます。
5. ページ単位で JSON を保存し、`segment_pdf` の戻り値として `SegmentPage` 辞書を返します。

## 出力
- `page_XXX_seg.png`: YOLO 検出結果を重ねたオーバーレイ画像。
- `page_XXX.json`: セグメント情報。`SegmentPage` の内容をそのまま保存しています。
- JSON 内の主なフィールド
  - `page`: ページ番号（1 始まり）
  - `size`: ページサイズ（ポイント単位）
  - `blocks`: `SegmentBlock` の配列
    - `type`: `text` / `caption` / `table` / `image` / `math`
    - `bbox`: `[x0, y0, x1, y1]`（ポイント単位）
    - `meta`: 追加情報
      - `doclayout_label`: 元の YOLO クラス名
      - `confidence`: YOLO のスコア
      - `text`, `text_preview`, `char_count`, `avg_font_size`, `fonts_top`（テキスト系のみ）
  - `png_overlay`: 生成したオーバーレイ PNG のパス
  - `json`: ページ JSON の保存先
  - `doclayout_model`, `doclayout_confidence`, `doclayout_iou`, `dpi`: 実行に利用したパラメータ

## コマンドライン実行

```bash
uv run common/seg.py attention_is_all_you_need.pdf \
  --outdir out/attention_seg \
  --dpi 150 \
  --conf 0.25 \
  --iou 0.5 \
```

主なオプション:
- `--outdir`: 出力ディレクトリ（既定値: `out`）
- `--dpi`: ページをレンダリングする DPI。値を上げると精度が上がる代わりに処理コストも増えます。
- `--conf`: YOLO の信頼度しきい値
- `--iou`: YOLO の NMS IoU しきい値
- `--device`: 推論デバイス（例: `cpu`, `cuda:0`）
- `--imgsz`: YOLO に入力する画像サイズを明示指定したい場合に使用

※ 既定では Hugging Face から重みを自動取得します

```bash
export DOCLAYOUT_YOLO_MODEL=/path/to/doclayout_yolo_base.pt
```

## Python から利用する例

```python
from common import seg

pages = seg.segment_pdf(
    pdf_path="attention_is_all_you_need.pdf",
    outdir="out/attention_seg",
    dpi=200,
    conf_threshold=0.3,
    iou_threshold=0.6,
    device="cpu",
)

first_page = pages[0]
print(first_page["page"], len(first_page["blocks"]))
```

戻り値の各要素は JSON と同じ構造で、翻訳フェーズでは `blocks` を再利用します。

## パイプラインとの関係
`main.py` から最初に呼び出され、翻訳対象のテキスト抽出とレイアウト復元の基礎データを提供します。翻訳処理 (`common/translate.py`) では `type` が `text` または `caption` のブロックだけが翻訳対象になります。

## 実装メモ
- YOLO の推論結果はピクセル座標で返るため、DPI から算出したスケールで PDF 座標へ戻しています。
- テキストメタデータは PyMuPDF の `rawdict` 出力を解析し、文字数やフォント分布を算出しています。
- オーバーレイ PNG は `ImageDraw` で描画し、ラベルには元クラス名とスコアを表示しています。
