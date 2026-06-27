# Spec: translation-text-overlap

## 概要

翻訳後PDFで、同一テキスト領域がDocLayout-YOLOで複数のオーバーラップセグメントとして検出され、それぞれの訳文が同じ位置に配置されて重なって表示される問題を解決する。compose（PDF再構成）段階で、テキスト配置前に重複セグメントを間引くことで、seg/translate 段階の検出漏れを吸収する最終防衛栏を設ける。

## 動機

issue #0002 で報告されたバグ。`attention_is_all_you_need.pdf` p7 において、IoU≈0.77 の重複ボックスが seg 段階の NMS(`iou=0.5`) をすり抜け、2つのセグメントが独立に翻訳・配置されて訳文が重なった。seg の NMS 根本原因（imgszリサイズ時の座標歪み等）は別issueとし、本issueでは compose 段階での防御的間引きで即効性のある解決を図る。

元issue: `issues/0002-translation-text-overlap.md`

## 要件差分

### ADDED（新規追加）

- [ ] **A1. compose 配置前の重複セグメント間引き（共通仕様）**: compose がテキストを配置する前に、位置が重複するテキストセグメントを間引き、1箇所に1つの訳文のみ配置する。仕様:
  - **重複判定基準**: IoS（共通面積 / min(area_a, area_b)）≥ 閾値（既定 `0.6`）。閾値はオプションで設定可能、`0` 以下で無効化。
  - **残す方の選択**: 基本は `translated_text`（未設定なら `source_text`）の文字列長が長い方を残す（issue #0002 の本来ケース b016/b017 は同サイズ・長い方 b016 を残す）。同長の場合は登場順が早い方を残す。ただし **巨大 bbox（ページ面積の40%超）は「異常検出」とみなし間引き優先（処理順を後回し）** とし、ページ全体を覆うような巨大 bbox が個別セグメントを吸収するのを防ぐ（p11_b001 問題への対策）。これにより通常の重複では長い方を残し、巨大 bbox では個別セグメントを残す、という2パス挙動になる。
  - **対象セグメント**: 実際にテキスト配置されるセグメントのみ（`image` / `table` / `math`・空テキスト・`target_types` 外は間引き対象外でそのまま残す）。
  - **監査可能性**: 間引いたセグメントは warnings に `重複セグメントを間引きました (page=..., dropped_id=..., kept_id=..., IoS≥...)` の形式で記録する。
- [ ] **A2. Python版への適用（`common/compose.py`）**: A1 を `compose_pdf` に実装する。`ComposeOptions` に `dedup_enabled: bool = True`, `dedup_ios_threshold: float = 0.6` を追加し、`cover_original` 適用後・segment 配置ループ前に間引きを実行する。※実装済み（事後spec化）
- [ ] **A3. Rust版への適用（`rust/src/compose.rs`）**: A1 と同等のロジックを Rust 版 `compose_pdf` に実装する。Python版と機能パリティを保証する。閾値・有効/無効はコンパイル時定数（または後述の clarify/plan で決定）とし、segment 配置ループ前に間引きを適用する。
- [ ] **A4. ドキュメント更新（`docs/compose.md`）**: 重複セグメント間引きの仕様・オプション・既定値・warnings フォーマットを記載する。Python版分は既に追記済み。Rust版の適用状況も追記する。

### MODIFIED（既存変更）

なし

### REMOVED（削除・破壊的）

なし

## 影響範囲

- `common/compose.py` — `ComposeOptions` に2フィールド追加、`_dedup_text_segments` 等の新規関数、`compose_pdf` に間引き呼び出し（※実装済み）
- `rust/src/compose.rs` — `compose_pdf` に間引きロジックを新規実装
- `docs/compose.md` — 重複間引きセクション（※Python版分は追記済み、Rust版分を追記）
- `common/schema.py` / `rust/src/schema.rs` — 変更なし（既存 SegmentBlock を使用）
- seg / translate ステージ — 変更なし（本 issue は compose 単独で防御する）
- CLI（`rust/src/main.rs`）— `--no-dedup` トグルを追加（`main.py` は変更なし、Python版は `ComposeOptions` 経由で制御）

## constitution との整合性

| 原則 | 適合 | 要更新 |
|------|------|--------|
| 1. パイプライン分離 | ✓ | |
| 2. フォールバック安全性 | ✓ | |
| 3. シングルバイナリ配布 | ✓ | |
| 4. レイアウト保存翻訳 | ✓ | |
| 5. 段階的移行 | ✓ | |

- 原則4: 重複テキストでレイアウトが崩れるのを防ぎ、原本レイアウトの忠実な保存を強化する方向。
- 原則5: Python版を正として実装し、機能パリティで Rust 版へ波及させる方針に合致。

違反なし。constitution の更新は不要。

## Clarifications

`clarify` フェーズで質問と回答を1組ずつ追記する。最初から明確なら空のままでよい。

| Q | A（推奨案含む） |
|---|---|
| Rust版の `dedup_enabled` / `dedup_ios_threshold` をどう公開するか？ | コンパイル時定数(const)で固定。`DEDUP_ENABLED: bool = true`, `DEDUP_IOS_THRESHOLD: f32 = 0.6`（Python版デフォルトと同一）。加えて `--no-dedup` のトグルのみ CLI に最小追加し、閾値の実行時変更は提供しない |
| 「テキスト長が長い方」の文字列長の定義は？ | 文字数（コードポイント数）で統一。Python: `len(str)`, Rust: `text.chars().count()`。日本語でも両実装で同一の判定結果になる |
| Rust版での間引きタイミングは？（Python版は strip 後・配置前） | strip 後・配置前に統一（Python版と同じ）。間引かれたセグメントは原文 redaction 済み（白塗り）＋訳文なしで配置される。機能パリティを優先 |
| （収束1）「残す方」の基準は「テキスト長が長い方」でよいか？ | **案A（2パス方式）**: 基本は「長い方優先」（issue #0002 の b016/b017 は同サイズ→長い方 b016 を残す）。ただし巨大 bbox（ページ面積の40%超）は「異常検出」とみなし処理順を後回しにし、通常セグメント（個別）を先に確定させる。これで p11_b001（ページ高さ84%の巨大 bbox）が個別セグメントを吸収するのを防ぎつつ、b016/b017 の本来ケースも issue 期待通りに解決する。面積単純優先（案C）は b016/b017 で「We」欠落の短い方 b017 を残してしまうため不採用 |
