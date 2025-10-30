# seg モジュール仕様

このモジュールは DocLayout‑YOLO を用いて PDF をレイアウト要素（text/image/table/caption/math）に分割します。通常はローカルに配置された学習済み重み（`models/doclayout_yolo_docstructbench_imgsz1024.pt`）を利用し、未配置の場合は Hugging Face から取得します。

## フォールバック動作（オフライン/権限不足時）

ネットワーク不通・権限不足・壊れたシンボリックリンクなどでモデルのロードまたは推論が出来ない場合は、次のフォールバックを自動適用します。

- ページ全体を覆う1つの `text` セグメントを生成
- そのセグメントの `meta.text` にはページ全体の抽出テキストを格納（compose 時の配置に利用）
- `segmenter.model_source` は `fallback:text-full-page` になります

これにより完全なレイアウト分割が行えない環境でも、パイプライン（翻訳→再構成）が停止せずに動作します。テスト（`make test`）や検証用途を想定した設計です。

## 既定のモデル配置

- 既定パス: `models/doclayout_yolo_docstructbench_imgsz1024.pt`
- 破損したシンボリックリンクが存在する場合でもパイプラインが止まらないよう、削除に失敗しても致命エラーにしません。

## 補足

- 高精度なレイアウト分割を行うには DocLayout‑YOLO/Ultralytics と学習済み重みの正しく配置された環境を推奨します。
- フォールバックは便宜的な動作であり、検出品質を保証するものではありません。

