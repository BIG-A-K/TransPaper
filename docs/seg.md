# セグメント抽出モジュール（`common/seg.py`）

DocLayout-YOLO でレイアウト解析を行い、PDF の各ページからテキスト・図・表・キャプションなどの領域を検出するモジュールです。検出結果は JSON とオーバーレイ PNG で保存されます。

## 依存関係
- PyMuPDF (`fitz`): PDF のレンダリングとテキスト抽出に使用します。
- ultralytics / torch / torchvision / numpy / Pillow: DocLayout-YOLO 推論に必要です。リポジトリでは `uv add doclayout-yolo ultralytics torch torchvision` 済み。
- huggingface-hub: モデルの自動ダウンロードに使用します。
- DocLayout-YOLO モデルは初回実行時に Hugging Face から自動的にダウンロードされ、`models/doclayout_yolo_docstructbench_imgsz1024.pt` に保存されます。

## DocLayoutYoloSegmenter クラス

セグメント処理を担当するクラスです。初期化時に以下のパラメータを指定できます:

```python
from common.seg import DocLayoutYoloSegmenter

segmenter = DocLayoutYoloSegmenter(
    conf=0.25,        # YOLO 信頼度しきい値
    iou=0.5,          # YOLO IoU しきい値
    device="cpu",     # 推論デバイス（cpu, cuda:0 など）
    image_size=1024   # 推論時の画像サイズ
)
```

主なメソッド:
- `predict(image)`: 画像に対して DocLayout-YOLO 推論を実行
- `_install_doclayout_yolo()`: Hugging Face からモデルを自動ダウンロード
- `_load_local(path)`: ローカルのモデルファイルをロード（複数のローダーを試行）

## PDF画像化ユーティリティ
`pdf2images(pdf_path, outdir, dpi=300)` は PyMuPDF を使って PDF を指定 DPI で PNG 化し、生成ファイルのパス一覧を返します。`segment_pdf` 以外でもページ画像だけが欲しい場合に便利です。

```python
from common.seg import pdf2images

image_paths = pdf2images("paper.pdf", "out/pages", dpi=300)
# -> [Path("out/pages/paper_page_001.png"), ...]
```

主な挙動:
- 存在しないパスを指定すると `FileNotFoundError` を送出。
- 既存の出力ディレクトリが無い場合は自動作成。
- ページ番号は 1 始まりでゼロ埋め（3 桁）になります。

## 処理の流れ
1. PyMuPDF でページを DPI 指定で画像化し、`numpy.ndarray` に変換します。
2. DocLayoutYoloSegmenter を使って DocLayout-YOLO でレイアウト領域を検出します。
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
    - `id`: ブロック ID（例: `p001_b000`）
    - `type`: `text` / `caption` / `table` / `image` / `math`
    - `bbox`: `[x0, y0, x1, y1]`（ポイント単位）
    - `meta`: 追加情報
      - `doclayout_label`: 元の YOLO クラス名
      - `confidence`: YOLO のスコア
      - `text`, `text_preview`, `char_count`, `avg_font_size`, `fonts_top`（テキスト系のみ）
  - `png_overlay`: 生成したオーバーレイ PNG のパス
  - `json`: ページ JSON の保存先
  - `doclayout_model`: 使用したモデルのパス
  - `doclayout_confidence`, `doclayout_iou`, `dpi`: 実行に利用したパラメータ
  - `granularity`: セグメント粒度（常に "block"）

## コマンドライン実行

```bash
uv run common/seg.py attention_is_all_you_need.pdf \
  --outdir out/attention_seg \
  --dpi 150 \
  --conf 0.25 \
  --iou 0.5 \
  --device cpu \
  --imgsz 1024
```

主なオプション:
- `--outdir`: 出力ディレクトリ（既定値: `out`）
- `--dpi`: ページをレンダリングする DPI。値を上げると精度が上がる代わりに処理コストも増えます。
- `--conf`: YOLO の信頼度しきい値（既定値: 0.25）
- `--iou`: YOLO の NMS IoU しきい値（既定値: 0.5）
- `--device`: 推論デバイス（例: `cpu`, `cuda:0`）。未指定時は `cpu`
- `--imgsz`: YOLO に入力する画像サイズ（既定値: 1024）

初回実行時には Hugging Face から自動的にモデルがダウンロードされます。

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
    image_size=1024,
)

first_page = pages[0]
print(first_page["page"], len(first_page["blocks"]))
```

戻り値の各要素は JSON と同じ構造で、翻訳フェーズでは `blocks` を再利用します。

## パイプラインとの関係
`main.py` から最初に呼び出され、翻訳対象のテキスト抽出とレイアウト復元の基礎データを提供します。翻訳処理 (`common/translate.py`) では `type` が `text` のブロックを優先的に処理し、`caption` や `math` も必要に応じて翻訳します。

## 実装メモ
- YOLO の推論結果はピクセル座標で返るため、DPI から算出したスケールで PDF 座標へ戻しています。
- テキストメタデータは PyMuPDF の `rawdict` 出力を解析し、文字数やフォント分布を算出しています。
- オーバーレイ PNG は `ImageDraw` で描画し、ラベルには元クラス名とスコアを表示しています。
- モデルのロードは複数のローダー（DocYOLOv10, DocYOLO, YOLOv10, YOLO）を順に試行し、最初に成功したものを使用します。
- モデルファイルは `models/` ディレクトリに保存され、2回目以降の実行では再ダウンロードされません。