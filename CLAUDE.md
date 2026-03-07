# lisprint

Rustで実装された本物のLisp処理系。開発時はインタプリタ(REPL)、本番はCraneliftでネイティブバイナリ。

## アーキテクチャ

```
lisprint/
├── crates/
│   ├── core/          # 言語コア (lisprint-core)
│   │   ├── value.rs   # Arc<Value> 型定義
│   │   ├── parser.rs  # S式パーサ
│   │   ├── eval.rs    # tree-walk evaluator
│   │   ├── env.rs     # 環境 (スコープチェーン)
│   │   └── builtins.rs # 組み込み関数
│   └── cli/           # CLI (lisprint)
│       └── main.rs    # REPL + ファイル実行
└── lib/               # Lispで書かれた標準ライブラリ (予定)
```

## 設計方針

- **Tiny Core**: 処理系はできるだけ小さく保つ
- **Arc<Value>**: GCなし、参照カウントでメモリ管理
- **Valueアクセスはメソッド経由**: `value.as_int()`, `Value::int(42)` — 将来のNaN boxing移行に備える
- **三層モジュール**: コア(tiny) → prelude(暗黙ロード) → stdlib(require)
- **不変変数**: def/letは不変、可変はatom

## 言語仕様

canow.life の lisprint プロジェクトに「言語仕様」ノートとして詳細あり。主なポイント:

- 関数定義は `defun` (defnではない)
- 型注釈: `x:i64` (オプショナル)
- キーワード: `:name` (前が空白/開き括弧)
- パイプ: `|>` (関数パイプ + メソッドチェーン)
- モジュール: `ns` + `export` (デフォルト非公開)
- エラー連鎖: `with` マクロ (nil短絡)
- パターンマッチ: `match`
- テスト: Clojure方式 `deftest`

## コマンド

```bash
cargo run              # REPL起動
cargo run -- repl      # REPL起動 (明示)
cargo run -- run FILE  # ファイル実行
cargo test             # テスト実行
```

## 開発ルール

- コミットはタスク単位で細かく
- コミットメッセージにタスク番号 (例: `1-3: S式パーサ`)
- テストを書いてから進む
- canow.life でタスク管理 (プロジェクト: lisprint)
