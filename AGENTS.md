# 開発環境
- m4 mac 16GB
- Python 3.12
  - `uv run`で実行
- uvで管理
  - モジュールを追加する際は`uv add`を使用する
- チャットは日本語
-  生成したコードの使い方や仕様などは`docs/{module名}.md`で作成する
- `README.md`に仕様等が書いてある


## 禁止事項
- uv pipの使用
  - `uv pip list`なら使っても良い。(普通に`pyproject.toml`を見てもらっても良い)
- rm コマンドの使用
  - 代わりにgomi/以下にmvすること
- Makefileの編集
- `docker/`以下の編集
  - 開発用docker環境を変更したい場合は`AGENTS.md`に記載して相談すること
  - 承認済み変更（2026-07-10）:
    - ollamaコンテナをroot以外（`make`実行ユーザーのUID/GID）で起動。ボリュームは `${HOME}/.ollama-transpaper` のbind mount（`container.sh`の`up`で所有権を保つため事前作成）。
    - `make in SERVICE=ollama` でollamaコンテナに入れるよう拡張（未指定時は従来の`agent_container`）。

# コード規則
```
uv run main.py --input {pdf}.pdf (--output {pdf(ja)}.pdf) (--mdoel {モデル名("deepl"など)})
```
でja->enにする。
test用に`./attention_is_all_you_need.pdf`を用意してあるので、それで検証する

`main.py`が`common/`以下のモジュールをimportする形で設計する


# ディレクトリ構造
<!-- ここを適宜更新しておいてください -->
```
 .
├──  AGENT.md
├──  claude.md
├──  common #mainから呼び出すコード群
│   └──  seg.py
├──  docker # 開発用docker環境の定義
│   ├──  compose.yml
│   ├──  container.sh
│   └──  Dockerfile
├──  docs # モジュールの説明など
├──  Makefile # 開発用docker環境等
├──  out # ここにtmpファイルを入れる
├──  pyproject.toml
├── 󰂺 README.md
└──  uv.lock
```
