# lisprint

> Lisp + Sprint — A fast Lisp that compiles to native binaries.

Rustで実装されたLisp処理系。開発時はREPLでインタラクティブに、本番はネイティブバイナリとしてデプロイ。

## Features

- **Arc\<Value\>ベース** — GC不要、参照カウントでメモリ管理
- **REPL** — 対話的な開発体験
- **動的型付け** — 型注釈はオプショナル
- **レキシカルスコープ** — クロージャ、高階関数
- **TCO** — 末尾呼び出し最適化
- **Craneliftコンパイル** — ネイティブバイナリ生成 (予定)

## Quick Start

```bash
# REPL
cargo run

# ファイル実行
cargo run -- run example.lisp
```

## Example

```lisp
;; フィボナッチ
(defun fib (n)
  (if (<= n 1) n
    (+ (fib (- n 1)) (fib (- n 2)))))

(println (fib 10))  ;; => 55

;; クロージャ
(defun make-adder (x)
  (fn (y) (+ x y)))

(def add5 (make-adder 5))
(println (add5 3))  ;; => 8

;; loop/recur
(defun factorial (n)
  (loop [i n acc 1]
    (if (= i 0) acc
      (recur (- i 1) (* acc i)))))

(println (factorial 10))  ;; => 3628800
```

## Roadmap

- [x] Phase 1: コアランタイム (パーサ, eval, REPL)
- [ ] Phase 2: マクロ + 標準ライブラリ
- [ ] Phase 3: Rustクレートbridge
- [ ] Phase 4: Craneliftネイティブコンパイル

## License

MIT
