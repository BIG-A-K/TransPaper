# Plan: translation-text-overlap

`spec.md` の what/why をどう実装するか。コードパスの対応表がこの文書の核心。

## 技術スタック

- Python 3.12 + PyMuPDF (fitz) — `common/compose.py`（既存実装あり）
- Rust edition 2021 + mupdf-rs 0.8 + clap 4 — `rust/src/compose.rs`, `rust/src/main.rs`
- 検証資料: `attention_is_all_you_need.pdf` p7（重複再現ケース）

## アーキテクチャ

Python版・Rust版とも、compose パイプラインの `cover_original`(strip) 適用後・テキスト配置ループ前に、独立した重複間引きステップを挟む。間引きは純粋関数として実装し、副作用は warnings への追記のみ。

```
compose_pdf(original, translated_pages, output)
  for each page:
    strip_page_text(...)                  # 既存
    dropped = compute_dedup_dropped(...)  # 新規: 間引き対象 index を計算
    for (i, segment) in segments:
      if i in dropped: continue           # スキップ
      place_text / place_region_snapshot  # 既存
```

Rust 版は `entry.segments` の所有権を動かさず、間引き対象インデックスの `HashSet<usize>` を返す設計（既存 text_rects 収集・strip は間引き前の全セグメントで動作 = Python版と同じ「間引かれたセグメントは白塗り枠＋訳文なし」）。

## 実装ファイル対応表

spec要件 → 実装ファイルの対応。これが tasks.md 生成の入力になる。

| spec要件 | 実装ファイル | 役割 |
|----------|--------------|------|
| A1 共通仕様（IoS・文字数・対象セグメント・warnings・巨大 bbox 2パス方式） | `common/compose.py`, `rust/src/compose.rs` | 両実装で同一仕様を再現。**巨大 bbox（ページ面積40%超）は異常検出扱いで処理順を後回し**（2パス方式）。`dedup_text_segments` は `page_area` を受け取り巨大 bbox 判定に使用 |
| A2 Python版 compose に間引き実装（※実装済み） | `common/compose.py` | `ComposeOptions` に `dedup_enabled`/`dedup_ios_threshold`、`_dedup_text_segments`/`_is_text_placement_target`/`_rect_intersection_area`、`compose_pdf` から `page_area` を渡して strip 後・配置前に呼出 |
| A3 Rust版 compose に間引き実装 | `rust/src/compose.rs` | `pub const DEDUP_ENABLED: bool = true`, `pub const DEDUP_IOS_THRESHOLD: f32 = 0.6`、新規 `dedup_text_segments`/`is_text_placement_target`/`rect_intersection_area`、`compose_pdf` のシグネチャに `dedup_enabled: bool` を追加。`dedup_text_segments` は `page_area: f32` を受け取り巨大 bbox 判定（ページ面積40%）に使用。strip 後・配置前のループでスキップ |
| A3 Rust版 CLI の `--no-dedup` トグル | `rust/src/main.rs` | `Cli` に `#[arg(long, default_value_t = false)] no_dedup: bool` を追加、`run_pipeline` → `compose::compose_pdf` の呼び出しで `dedup_enabled = compose::DEDUP_ENABLED && !cli.no_dedup` を渡す |
| A4 Python版 docs（※追記済み） | `docs/compose.md` | 「重複セグメントの間引き」セクション |
| A4 Rust版 docs 追記 | `docs/compose.md` | Rust版の const・`--no-dedup` オプションの記載を既存セクションへ追記 |
| 検証（Python） | 手動 E2E | `uv run main.py --input attention_is_all_you_need.pdf` で p7 の重複解消・他ページ デグレなし確認 |
| 検証（Rust） | 手動 E2E + ユニットテスト | `cargo run -- --input attention_is_all_you_need.pdf` で p7 の重複解消確認、`#[cfg(test)]` で `rect_intersection_area` / IoS 計算の境界テスト |

## リスク・調査項目

- **Rust の `chars().count()` と Python の `len(str)` の一致**: どちらも Unicode スカラ値（コードポイント）数。絵文字の ZWJ sequence 等では見た目と乖離するが、論文テキストの重複判定では影響しない → 調査不要
- **mupdf-rs の Rect 演算**: `Rect::x0/y0/x1/y1` は f32。`rect.width * rect.height` で面積算出可能。intersect は自前ヘルパで実装（mupdf-rs に直接 API はない）→ 新規 `rect_intersection_area(a, b) -> f32` を compose.rs に追加
- **ソート安定性**: Rust の `sort_by` は安定ソート（`sort_by` は stable）。Python の `sorted(key=)` も安定。テキスト長降順・登場順昇順で両者一致 → リスクなし
- **Rust `page.bounds()` 失敗時フォールバック**: `compose_pdf` で `page.bounds().unwrap_or(Rect::new(0.0, 0.0, 612.0, 792.0))` とし、取得失敗時は US Letter（612×792pt）を仮定する。この値は `page_area`（→巨大 bbox 判定の閾値 = page_area * 0.4）に影響するが、通常の PDF では bounds 取得に失敗しないため実害なし

## constitution 準拠

- **1. パイプライン分離**: compose モジュール内に間引きを閉じ込め、seg/translate への波及なし。中間 JSON 形式も不変
- **2. フォールバック安全性**: `dedup_enabled=false`（`--no-dedup`）で従来動作に戻せる。閾値は固定だがトグルで無効化可能
- **3. シングルバイナリ配布**: 新規依存なし。既存の mupdf-rs/clap の範囲内。配布バイナリサイズは不変
- **4. レイアウト保存翻訳**: 重複テキストでレイアウトが崩れるのを防ぐ方向。間引かれた枠は白塗り＋訳文なしで原本レイアウトを保持
- **5. 段階的移行**: Python版を正として実装し、同一仕様を Rust 版に波及。機能パリティを担保

## analyze — 整合性自己チェック

- A1 共通仕様 → Python(A2)・Rust(A3) 両方に対応ファイルあり ✓
- A2 Python版 → `common/compose.py`（実装済み）✓
- A3 Rust版 compose → `rust/src/compose.rs` ✓
- A3 Rust版 CLI トグル → `rust/src/main.rs` ✓
- A4 docs → `docs/compose.md`（Python分追記済み + Rust分追記）✓
- 検証 → 手動 E2E（両言語）+ Rust ユニットテスト ✓
- spec に無い勝手な要件: なし（新規ライブラリ・新規ステージ追加なし）✓
- constitution 違反: なし ✓

不整合なし。tasks フェーズへ。
