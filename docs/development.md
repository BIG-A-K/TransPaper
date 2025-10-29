# テスト
git cloneしたらそのまますぐに
```sh
make test
```
を実行すると`attention_is_all_you_need.pdf`をwgetでダウンロードし、それを検証します。
翻訳はせず、そのまま英語で出力するようになっています。

# フォーマットチェック
github actionsでruffのリンターチェックが入ります。
git pushする前に
```sh
make ci
```
を実行しておいてください。
中身では`wrkflw`というrust製のgithub actionを実行してくれるツールを使用しています。
`cargo install wrkflw`でいけます。

# 環境条件
- linux or mac OS
- uv
- make
- wrkflw

# 要件定義
## 機能要求
1. 入力はPDFとする
2. PDFをセマンティックセグメンテーションによって分割する
3. 分割された領域のうち、textとcaptionを翻訳する
   1. optionでhuggingfaceモデルかdeeplのAPIかを選択できるようにする
   2. デフォルトはDeepLの翻訳
4. 再統合する
5. 出力はデフォルトでは見開きで英語｜日本語となるようにする
   1. 引数でjaのみかen|jaの２形式で出すようする

- pythonで開発する
- モジュール化で機能を分割する

## 非機能要求
- メモリ：16GB以下(localhostで使う場合)
- batch処理に対応
- tqdmによる進捗度の表示