# 概要
このプロジェクトはPDFを要素{figure,caption,text,mathなど}に分割したのち、それぞれを翻訳し、再統合することでen->jaの翻訳を行うものです。

## 使い方
```sh
DEEPL_API={DEEPL_APIの鍵} uv run main.py --input hoge.pdf (--output translated_hoge.pdf) (--model deepl)
````
で実行できます。

`-m,--model`オプションで、使いたいモデルの変更ができるようにしてあります。


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
