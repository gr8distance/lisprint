# lisprint

Rustで実装された本物のLisp処理系。開発時はインタプリタ(REPL)、本番はCraneliftでネイティブバイナリ。

## アーキテクチャ

```
lisprint/
├── crates/
│   ├── core/          # 言語コア (lisprint-core)
│   │   ├── value.rs   # Arc<Value> 型定義 (TypeInstance含む)
│   │   ├── parser.rs  # S式パーサ
│   │   ├── eval.rs    # tree-walk evaluator (94テスト)
│   │   ├── env.rs     # 環境 (スコープチェーン)
│   │   ├── builtins.rs # 組み込み関数
│   │   ├── prelude.rs # preludeローダー
│   │   └── stdlib/    # Rustネイティブstdlibモジュール群
│   └── compiler/      # Craneliftコンパイラ (lisprint-compiler)
│       ├── compiler.rs # AST → Cranelift IR → object
│       └── runtime.rs  # ランタイムbridge関数
│   └── cli/           # CLI (lisprint)
│       └── main.rs    # REPL + ファイル実行 + test
└── lib/
    └── prelude.lisp   # Lispで書かれた標準ライブラリ
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
- 複数アリティ: `(defun f ((a) body1) ((a b) body2))`
- 型注釈: `x:i64` (オプショナル)
- キーワード: `:name` (前が空白/開き括弧)
- パイプ: `|>` (関数パイプ + メソッドチェーン)
- モジュール: `ns` + `export` (デフォルト非公開), `require` (:as/:only/:all)
- エラー: `throw` / `try` / `catch`, `with` マクロ (nil短絡)
- パターンマッチ: `match` (リテラル, リスト, ベクタ, マップ, ワイルドカード)
- カスタム型: `deftype` / `deftrait` / `defimpl`, `.field` アクセス
- 分配束縛: `let` でベクタ/マップを分解
- テスト: `deftest` + `assert=` / `assert-true` / `assert-nil`

## コマンド

```bash
cargo run              # REPL起動
cargo run -- repl      # REPL起動 (明示)
cargo run -- run FILE  # ファイル実行
cargo run -- test      # *_test.lisp 自動検出・テスト実行
cargo run -- test FILE # 指定ファイルのテスト実行
cargo test             # Rustテスト実行 (118テスト)
cargo run -- build FILE [output] [--container]  # ネイティブバイナリ生成
```

## 開発ルール

- コミットはタスク単位で細かく
- コミットメッセージにタスク番号 (例: `2-5: エラーハンドリング`)
- テストを書いてから進む
- canow.life でタスク管理 (プロジェクト: lisprint)
