# 概要
このプロジェクトはPDFを要素{figure,caption,text,mathなど}に分割したのち、それぞれを翻訳し、再統合することでen->jaの翻訳を行うものです。

Python版とRust版の2つの実装があります。

## 使い方

### Rust版（シングルバイナリ）

Linux x86_64環境では、GitHub Releasesからビルド済みバイナリを取得できます。

```sh
curl -L -O https://github.com/BIG-A-K/TransPaper/releases/latest/download/transpaper-x86_64-linux.tar.gz
curl -L -O https://github.com/BIG-A-K/TransPaper/releases/latest/download/transpaper-x86_64-linux.sha256
sha256sum -c transpaper-x86_64-linux.sha256
tar -xzf transpaper-x86_64-linux.tar.gz

DEEPL_API={DEEPL_APIの鍵} ./transpaper-*-x86_64-linux/transpaper --input hoge.pdf --output translated_hoge.pdf --model deepl
```

リリースは`v*`タグをpushすると自動作成され、`transpaper-x86_64-linux.tar.gz`と検証用の`transpaper-x86_64-linux.sha256`が添付されます。

ローカルでビルドする場合は以下を使用します。

```sh
# ビルド
cd rust && cargo build --release

# 翻訳（DeepL）
DEEPL_API={DEEPL_APIの鍵} ./rust/target/release/transpaper --input hoge.pdf --output translated_hoge.pdf --model deepl

# 翻訳なしテスト（idxモデル）
./rust/target/release/transpaper --input hoge.pdf --model idx

# 比較PDF作成
DEEPL_API={DEEPL_APIの鍵} ./rust/target/release/transpaper --input hoge.pdf --compare
```

初回実行時にDocLayout-YOLOモデル（ONNX, 約75MB）がHuggingFace Hubから自動ダウンロードされます。

### Python版

```sh
DEEPL_API={DEEPL_APIの鍵} uv run main.py --input hoge.pdf (--output translated_hoge.pdf) (--model deepl)
```

`-m,--model`オプションで、使いたいモデルの変更ができるようにしてあります。

### 比較PDF作成
元のPDFと翻訳後のPDFを見開きで比較したい場合は、`--compare`または`-c`オプションを使用してください：

```sh
# Rust版
DEEPL_API={DEEPL_APIの鍵} ./rust/target/release/transpaper --input hoge.pdf --compare

# Python版
DEEPL_API={DEEPL_APIの鍵} uv run main.py --input hoge.pdf --compare
```

このオプションを指定すると、「元のページ→翻訳後のページ→元のページ→翻訳後のページ...」という順序で配置された比較用PDFが生成されます。デフォルトのファイル名は`translated_{ファイル名}_compare.pdf`になります。


## 開発者へ
Documentを`docs/`以下に書いてあります。適宜参照してください。
開発者向けのドキュメントは`docs/development.md`です。適宜参照してください。

また、`AGENTS.md`を用意してあります。いい感じに修正して使ってください。

codexなどのAgentが変に暴れると怖いのでDocker環境を用意してあります。
```sh
make build
make up
make in
```
でコンテナを作成し、入れます。
必要に応じて`docker/`以下を参照、あるいは編集してください。

なお、make実行時に勝手に`.env`ファイルが生成されるので、事前に作るなどは**しない**でください
