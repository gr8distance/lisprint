# lisprint

> Lisp + Sprint — A fast Lisp that compiles to native binaries.

Rust で実装された Lisp 処理系。開発時はインタプリタ (REPL)、本番は Cranelift でネイティブバイナリにコンパイル。

## Features

- **Arc\<Value\>ベース** — GC不要、参照カウントでメモリ管理
- **REPL** — 対話的な開発体験
- **Cranelift コンパイル** — ネイティブバイナリ生成
- **プロジェクト管理** — `lisprint new` / `init` / `add` / `build` / `run`
- **豊富な標準ライブラリ** — HTTP, JSON, ファイルI/O, 正規表現, 非同期処理
- **マクロ** — `defmacro` + quasiquote/unquote
- **パターンマッチ** — `match` 式
- **カスタム型** — `deftype` / `deftrait` / `defimpl`

## インストール

```bash
git clone <repo-url>
cd lisprint
sudo bin/install
```

`/usr/local/bin/lisprint` にリリースビルドがインストールされる。

## クイックスタート

```bash
# プロジェクト作成
lisprint new my-app
cd my-app

# 実行
lisprint run

# ネイティブバイナリにコンパイル
lisprint build
./my-app
```

## コマンド

```
lisprint new <name>              プロジェクト作成
lisprint init                    現在のディレクトリで初期化
lisprint add <pkg>               依存ライブラリ追加 (lisp.toml)
lisprint run [file]              実行 (省略時: src/main.lisp)
lisprint build [file] [output]   ネイティブバイナリ生成
lisprint test [files...]         テスト実行 (*_test.lisp)
lisprint check [file]            構文チェック
lisprint eval '<expr>'           式を評価
lisprint repl                    対話モード (デフォルト)

lisprint -h, --help              ヘルプ
lisprint -v, --version           バージョン
lisprint build --container       Dockerfile も生成
```

## プロジェクト構成

`lisprint new` で生成される構成:

```
my-app/
├── lisp.toml          # プロジェクト設定
└── src/
    └── main.lisp      # エントリポイント
```

**lisp.toml:**

```toml
[project]
name = "my-app"
version = "0.1.0"

[dependencies]
json = "*"
http = "*"
```

## 言語の例

```lisp
;; 関数定義
(defun greet (name)
  (println (str "Hello, " name "!")))

(greet "world")  ;; => Hello, world!

;; 高階関数
(def squares (map (fn (x) (* x x)) [1 2 3 4 5]))
;; => (1 4 9 16 25)

;; loop/recur
(defun factorial (n)
  (loop [i n acc 1]
    (if (= i 0) acc
      (recur (- i 1) (* acc i)))))

(println (factorial 20))  ;; => 2432902008176640000

;; パターンマッチ
(match status
  (:ok    (println "success"))
  (:error (println "failed"))
  (_      (println "unknown")))

;; HTTP サーバ
(require "http/server")
(server/start 8080
  (fn (req)
    {:status 200
     :body "Hello from lisprint!"
     :content-type "text/plain"}))
```

詳細なドキュメントは [docs/](docs/) を参照。

## アーキテクチャ

```
crates/
├── core/          lisprint-core (パーサ, 評価器, 標準ライブラリ)
├── compiler/      lisprint-compiler (Cranelift ネイティブコンパイラ)
└── cli/           lisprint (CLI + REPL)
lib/
└── prelude.lisp   Lisp で書かれた prelude (暗黙ロード)
```

## テスト

```bash
# Rust テスト (118テスト)
cargo test

# Lisp テスト
lisprint test
```

## Roadmap

- [x] Phase 1: コアランタイム (パーサ, eval, REPL)
- [x] Phase 2: マクロ + 標準ライブラリ (prelude)
- [x] Phase 3: Rust クレート bridge (stdlib 11モジュール)
- [x] Phase 4: Cranelift ネイティブコンパイル
- [ ] Extra: フロー型推論パス

## License

MIT
