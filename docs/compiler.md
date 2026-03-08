# ネイティブコンパイラ

lisprint は Cranelift を使って Lisp コードをネイティブバイナリにコンパイルできる。

## 使い方

```bash
# ファイル指定
lisprint build app.lisp

# プロジェクト (lisp.toml があるディレクトリ)
lisprint build

# 出力名指定
lisprint build app.lisp my-binary

# Docker コンテナ対応
lisprint build app.lisp --container
```

## ビルドパイプライン

```
.lisp ソース
    ↓ parse
AST (Value)
    ↓ compile_exprs (4パス)
    │  1. 文字列リテラル収集 → データセクション
    │  2. 関数宣言 (defun シグネチャ)
    │  3. 各 defun の IR 生成
    │  4. _lsp_main (トップレベル式)
    ↓
Cranelift IR → オブジェクトファイル (.o)
    ↓
C エントリポイント生成 (_entry.c)
    ↓ cc でリンク
実行ファイル
```

## コンパイル対応構文

| 構文 | 対応 |
|---|---|
| 整数リテラル | `42` |
| 浮動小数点リテラル | `3.14` |
| 文字列リテラル | `"hello"` |
| 真偽値 | `true`, `false` |
| nil | `nil` |
| `def` | ローカル変数定義 |
| `defun` | 関数定義 |
| 関数呼び出し | `(f x y)` |
| 再帰 | `(fib (- n 1))` |
| `if` | 条件分岐 (else 省略可) |
| `let` | ローカル束縛 |
| `do` | 連続実行 |
| `+`, `-`, `*`, `/`, `%` | 算術演算 (2項) |
| `=`, `!=`, `<`, `>`, `<=`, `>=` | 比較演算 |
| `not` | 論理否定 |
| `println`, `print` | 出力 |
| `str_concat` | 文字列結合 |
| `to_string` | 文字列変換 |

## 値の表現

コンパイル後の値は `(tag: i64, payload: i64)` のペアで表現される。

| Tag | 値 | payload |
|---|---|---|
| 0 | nil | 0 |
| 1 | bool | 0 (false) or 1 (true) |
| 2 | int | 符号付き 64bit 整数 |
| 3 | float | IEEE-754 double のビット表現 |
| 4 | string | ヌル終端文字列へのポインタ |

## インタプリタとの違い

コンパイラはインタプリタのサブセットをサポートする。以下はコンパイル非対応:

- クロージャ / 高階関数 (`map`, `filter` など)
- マクロ (`defmacro`)
- 標準ライブラリ (`require`)
- リスト操作 (`cons`, `first`, `rest` など)
- パターンマッチ (`match`)
- カスタム型 (`deftype`)
- エラーハンドリング (`try`/`catch`)
- 動的評価 (`eval`)
- 可変長引数の算術 (`(+ 1 2 3)` — 2項のみ)

典型的なユースケースは、計算集約的なロジックをネイティブコンパイルして高速実行すること。

## Docker コンテナ

`--container` フラグで Dockerfile を自動生成:

```bash
lisprint build app.lisp --container
# => Dockerfile.app が生成される

docker build -f Dockerfile.app -t app .
docker run app
```

scratch コンテナの場合、静的リンクが必要:

```bash
CC=musl-gcc lisprint build app.lisp app --container
```
