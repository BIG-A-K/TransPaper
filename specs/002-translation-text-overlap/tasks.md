# Tasks: translation-text-overlap

`spec.md` + `plan.md` から生成された実行チェックリスト。**各タスクはファイルパスを明示し、spec↔コードの契約となる。**

## 凡例

- `[ ]` / `[x]`: 未完了 / 完了
- `[P]`: 並列実行可能（依存の無いタスク）
- 依存順に並べる（前のタスクが後のタスクの前提）

## Phase 1: Python版 compose（実装確認＋検証）

- [x] 1.1 `common/compose.py` に重複間引きを実装: `ComposeOptions` に `dedup_enabled`/`dedup_ios_threshold` を追加、`_dedup_text_segments`/`_is_text_placement_target`/`_rect_intersection_area` を実装、`compose_pdf` で strip 後・配置前に呼出 ※事前実装済み (`common/compose.py`)
- [x] 1.2 `docs/compose.md` に「重複セグメントの間引き」セクション追記（Python版分） ※事前実装済み (`docs/compose.md`)
- [x] 1.3 Python版検証: `attention_is_all_you_need.pdf` で p7 の重複解消 + 他ページのデグレなしを確認（`uv run main.py --input attention_is_all_you_need.pdf`）※現在の seg では p7 の重複は発生せず。他ページ（p11/p12）の過剰間引きは C4 で解消確認

**チェックポイント**: p7 で2つの訳文重なりが解消され、他ページのレイアウトが従来同等であること

## Phase 2: Rust版 compose 実装

- [x] 2.1 `compose.rs` に `const DEDUP_ENABLED: bool = true`, `const DEDUP_IOS_THRESHOLD: f32 = 0.6` を追加、ヘルパー（`rect_intersection_area` / `is_text_placement_target`）と `dedup_text_segments`（間引き対象 index の `HashSet<usize>` を返す・文字数は `chars().count()`）を実装 (`rust/src/compose.rs`)
- [x] 2.2 `compose_pdf` のシグネチャに `dedup_enabled: bool` 引数を追加。strip 後・配置前の segment ループで dropped index をスキップ（間引かれたセグメントは白塗り枠＋訳文なし） (`rust/src/compose.rs`)
- [x] 2.3 `main.rs` の `Cli` に `--no-dedup`（`default_value_t = false`）を追加。`run_pipeline` → `compose::compose_pdf` の呼び出しで `dedup_enabled = !cli.no_dedup` を渡す (`rust/src/main.rs`)
- [P] 2.4 `compose.rs` に `#[cfg(test)]` で `rect_intersection_area` / IoS 計算のユニットテストを追加（空集合・完全包含・部分重複・閾値境界 0.6） (`rust/src/compose.rs`)

**チェックポイント**: `cargo build --release` と `cargo test` が通ること

## Phase 3: Rust版 E2E 検証

- [x] 3.1 Rust版 E2E: `cargo run --release -- --input attention_is_all_you_need.pdf --model idx`（翻訳なし）で p7 の重複解消を確認 (`rust/src/compose.rs`)

**チェックポイント**: p7 で訳文重なりが解消されていること

## Phase 4: docs 完成

- [x] 4.1 `docs/compose.md` の重複間引きセクションに Rust版（const 定義値・`--no-dedup` オプション）を追記 (`docs/compose.md`)

**チェックポイント**: Python・Rust 双方のオプション仕様が1箇所に集約されていること

## converge で追記されたタスク

`converge` フェーズで差分が見つかった場合にここへ追記し、implement ループへ戻す。

### 収束ループ1: 過剰間引き問題（p11/p12 デグレ）

implement で Python E2E を実施した結果、p11・p12 で seg が「ページ全体を覆う巨大 bbox（b001: ページ高さ84%, text_len=3204）」を異常検出し、現行基準「テキスト長が長い方を残す」では巨大 bbox が個別セグメントを全て吸収して残り、レイアウト崩壊（デグレ）を起こすことが判明。「面積小さい方優先」への単純変更は p7 の b016/b017（"We"欠落の不良 b017 を残してしまう）で issue 期待と逆になったため、**案A（2パス方式）** に変更。

- [x] C1 spec.md A1 の「残す方」基準を案Aに変更。Clarifications に追記 (`specs/002-translation-text-overlap/spec.md`)
- [x] C2 Python版 `_dedup_text_segments`: 案A実装。通常セグメント（面積≤ページ面積40%）を長い方優先で処理した後、巨大セグメント（>40%）を後回しで処理して accepted と重複判定。`compose_pdf` から `page_area` を渡す (`common/compose.py`)
- [x] C3 Rust版 `dedup_text_segments`: 案A実装。シグネチャに `page_area: f32` を追加。テストに「巨大 bbox は個別セグメントに吸収される（b001 dropped）」「通常重複は長い方優先（b016 kept）」を追加 (`rust/src/compose.rs`)
- [x] C4 E2E再検証: Python・Rust 両方で p7 の重複解消（b016 kept / b017 dropped）+ p11/p12 の過剰間引き解消（b001 等の巨大 bbox が dropped）を確認
  - Rust版: p7 `kept_id=p007_b016` ✓ / p11 `dropped_id=p011_b001` ✓ / 16/16 unit tests pass
  - Python版: warnings 38件→4件に激減、p11/p12 の巨大 bbox dropped を確認 ✓

**チェックポイント**: p7 で b016 が残り b017 が間引き、p11/p12 で巨大 bbox（b001/b007 相当）が個別セグメントに吸収されて dropped になること

### 収束ループ2: ドキュメント整合性（converge 監査で検出）

converge 監査（Spec照合 + Drift検出）で、実装は A1-A4 全て満たすことを確認。ただし以下4つのドキュメント系 drift を検出したため、spec/docs/plan の整合性を取る。

- [x] C5 `docs/compose.md` の重複間引きセクションに「巨大 bbox（ページ面積40%超）は異常検出として処理順を後回しにする2パス方式（`0.4` 固定値）」を追記（Python・Rust 共通仕様） (`docs/compose.md`)
- [x] C6 `spec.md` 影響範囲の「CLI（main.py, rust/src/main.rs）— 新規オプション追加なし」を Clarifications（--no-dedup 追加）と整合するよう修正 (`specs/002-translation-text-overlap/spec.md`)
- [x] C7 `plan.md` の実装ファイル対応表に「`dedup_text_segments` は `page_area: f32` を受け取り巨大 bbox 判定（ページ面積40%）に使用」を追記 (`specs/002-translation-text-overlap/plan.md`)
- [x] C8 docs または plan に「Rust 版 `page.bounds()` 失敗時のフォールバックは US Letter (612×792)」の注記を追記 (`plan.md` リスクセクション)

