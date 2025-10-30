# seg モジュール仕様

`common/seg.py` は DocLayout-YOLO を用いて PDF をページごとにレイアウト要素に分割するモジュールです。

## 概要

PDFの各ページを画像化し、DocLayout-YOLOでレイアウト要素（text/figure/table/caption/equation など）を検出します。検出されたセグメントは以下の情報を含みます：

- **座標** (PDF座標系での bbox)
- **タイプ** (`text`, `image`, `table`, `caption`, `math`)
- **メタデータ** (DocLayout-YOLOのラベル、信頼度スコア、テキスト要素の場合は抽出テキスト)
- **一意ID** (`p{page}_b{block}` 形式)

## 主要な関数

### `segment_pdf()`

PDFファイル全体をセグメント分割し、ページごとにJSON・PNG（可視化画像）を出力します。

**パラメータ:**
- `pdf_path`: 入力PDFのパス
- `outdir`: 出力ディレクトリ (デフォルト: "out")
- `dpi`: ページ画像化の解像度 (デフォルト: `150`)
- `conf_threshold`: 検出信頼度の閾値 (デフォルト: `0.25`)
- `iou_threshold`: NMS の IOU 閾値 (デフォルト: `0.5`)
- `device`: 推論デバイス (`"cpu"`, `"cuda"` など)
- `image_size`: YOLO の入力画像サイズ (デフォルト: `1024`)

**戻り値:**
- `list[SegmentPage]`: 全ページの検出結果のリスト

**出力ファイル:**
- `page_{N:03d}.json`: セグメント情報を含むJSON
- `page_{N:03d}_seg.png`: 検出結果を描画した可視化画像

## モデルの解決とロード

`DocLayoutYoloSegmenter` クラスは次の優先順位でモデルを探索します：

1. **プロジェクト内の実体ファイル**  
   `models/doclayout_yolo_docstructbench_imgsz1024.pt` が存在する場合はこれを使用  
   
2. **Hugging Face Hub からの自動ダウンロード**  
   ローカルにファイルが無い場合、`juliozhao/DocLayout-YOLO-DocStructBench` から `doclayout_yolo_docstructbench_imgsz1024.pt` を自動取得してキャッシュ

3. **フォールバックモード**  
   上記が失敗した場合（オフライン環境・依存未インストール・権限不足など）は、ページ全体を1つの `text` セグメントとして扱う擬似的な検出結果を返す

### ローダーの試行順序

以下のローダーを順に試行し、最初に成功したものを採用：
- `doclayout_yolo.YOLOv10`
- `doclayout_yolo.YOLO`
- `ultralytics.YOLOv10`
- `ultralytics.YOLO`

## フォールバック動作

モデルのロードまたは推論が失敗した場合、自動的に **フォールバックモード** に切り替わります：

- ページ全体を覆う1つの `text` セグメント（bbox: `[0, 0, width, height]`）を生成
- そのセグメントの `meta.text` にはページ全体の PyMuPDF 抽出テキストを格納
- `model_source` は `"fallback:text-full-page"` に設定
- 2ページ目以降も推論を再試行せず、フォールバックモードを継続

この仕組みにより、レイアウト分割が不可能な環境（例: CI/CD、権限制限された環境、オフライン環境）でも翻訳パイプラインが停止せずに動作します。

## セグメントタイプのマッピング

DocLayout-YOLO が出力するラベルは、次のルールで標準タイプにマッピングされます：

| DocLayout ラベル | TransPaper タイプ |
|-----------------|------------------|
| `caption`, `figure_caption`, `table_caption` など | `caption` |
| `table` | `table` |
| `figure`, `image`, `picture`, `graphic` など | `image` |
| `equation`, `formula`, `math` など | `math` |
| 上記以外 | `text` |

## テキスト抽出

`text` および `caption` タイプのセグメントには、PyMuPDF の `get_textbox()` を用いて該当領域のテキストを抽出し、`meta.text` フィールドに格納します。

## 座標系

- DocLayout-YOLO は **画像座標系**（左上原点、ピクセル単位）で bbox を返す
- `seg.py` はこれを **PDF座標系**（左上原点、ポイント単位）に変換して保存
- 変換時に DPI と zoom 係数を考慮し、ページ範囲外にはみ出た bbox はクランプ（切り詰め）される

## 依存関係

- **必須**: `fitz` (PyMuPDF), `PIL`, `numpy`, `loguru`
- **オプション（レイアウト検出用）**:
  - `doclayout_yolo` (推奨)
  - `ultralytics`
  - `huggingface_hub` (モデル自動ダウンロード用)

これらが未インストールの場合、フォールバックモードで動作します。

## 使用例

```python
from common.seg import segment_pdf

results = segment_pdf(
    pdf_path="paper.pdf",
    outdir="output/seg",
    dpi=150,
    conf_threshold=0.3,
    device="cuda"
)

for page in results:
    print(f"Page {page['page']}: {len(page['blocks'])} blocks detected")
```

## 補足

- 高精度なレイアウト分割を行うには、DocLayout-YOLO と学習済み重みの正しい配置を推奨
- フォールバックは便宜的な動作であり、検出品質を保証するものではありません
- テスト環境（`make test`）や CI では依存をインストールせずフォールバックで動作可能
