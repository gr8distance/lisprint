# lisprint 言語リファレンス

## データ型

| 型 | 例 | 説明 |
|---|---|---|
| Integer | `42`, `-10` | 64bit 整数 |
| Float | `3.14`, `-0.5` | 64bit 浮動小数点 |
| String | `"hello"` | 文字列 (`\n`, `\t`, `\\`, `\"` エスケープ) |
| Boolean | `true`, `false` | 真偽値 |
| Nil | `nil` | null値 |
| Keyword | `:name`, `:ok` | キーワード (イミュータブルな識別子) |
| Symbol | `foo`, `my-func` | シンボル (変数名・関数名) |
| List | `(1 2 3)`, `'(a b)` | リスト |
| Vector | `[1 2 3]` | ベクタ |
| Map | `{:a 1 :b 2}` | マップ (連想配列) |

## 変数定義

```lisp
;; イミュータブル変数
(def x 42)
(def name "lisprint")

;; ローカル束縛
(let ((x 10)
      (y 20))
  (+ x y))  ;; => 30

;; 分配束縛 (ベクタ)
(let ([a b c] [1 2 3])
  (+ a b c))  ;; => 6

;; 分配束縛 (マップ)
(let ({:keys [name age]} {:name "Alice" :age 30})
  (println name))  ;; => Alice
```

## 関数

```lisp
;; 名前付き関数
(defun add (a b)
  (+ a b))

;; 無名関数
(def double (fn (x) (* x 2)))

;; 複数アリティ
(defun greet
  ((name) (str "Hello, " name "!"))
  ((first last) (str "Hello, " first " " last "!")))

(greet "Alice")          ;; => "Hello, Alice!"
(greet "Alice" "Smith")  ;; => "Hello, Alice Smith!"

;; 型注釈 (オプショナル)
(defun add (a:i64 b:i64)
  (+ a b))
```

## 制御フロー

```lisp
;; if
(if (> x 0)
  "positive"
  "non-positive")

;; when / unless (マクロ)
(when (> x 0)
  (println "positive"))

(unless (= x 0)
  (println "non-zero"))

;; do (連続実行)
(do
  (println "step 1")
  (println "step 2")
  "done")
```

## ループ

```lisp
;; loop/recur (末尾再帰最適化)
(defun factorial (n)
  (loop [i n acc 1]
    (if (= i 0) acc
      (recur (- i 1) (* acc i)))))

;; 再帰
(defun fib (n)
  (if (<= n 1) n
    (+ (fib (- n 1)) (fib (- n 2)))))
```

## パターンマッチ

```lisp
(match value
  ;; リテラルマッチ
  (42 "the answer")
  ("hello" "greeting")
  (nil "nothing")

  ;; バインディング
  (x (str "got: " x)))

;; リストパターン
(match '(1 2 3)
  ((1 x y) (+ x y)))  ;; => 5

;; ベクタパターン
(match [1 2]
  ([a b] (* a b)))  ;; => 2

;; マップパターン
(match {:name "Alice" :age 30}
  ({:name n} (str "name is " n)))

;; ワイルドカード
(match x
  (_ "anything"))
```

## エラーハンドリング

```lisp
;; throw
(throw "something went wrong")

;; try / catch
(try
  (do-something-risky)
  (catch e
    (println (str "Error: " e))))

;; with (nil短絡)
(with
  (a (may-return-nil))
  (b (also-may-fail a))
  (process a b))
```

## マクロ

```lisp
(defmacro unless (cond body)
  `(if (not ~cond) ~body nil))

;; quasiquote: `
;; unquote: ~
;; splice-unquote: ~@
```

## モジュール

```lisp
;; 名前空間
(ns my-module)
(export add multiply)

;; require
(require "json")                    ;; json/parse, json/encode
(require "http" :as h)              ;; h/get, h/post
(require "math" :only [sqrt pow])   ;; sqrt, pow のみ
(require "str" :all)                ;; 全関数をインポート
```

## カスタム型

```lisp
;; 型定義
(deftype Point [x y])

;; コンストラクタ
(def p (Point 10 20))

;; フィールドアクセス
(.x p)    ;; => 10
(.y p)    ;; => 20
(Point? p) ;; => true

;; トレイト
(deftrait Printable
  (to-str [self]))

(defimpl Printable Point
  (to-str [self]
    (str "(" (.x self) ", " (.y self) ")")))
```

## テスト

```lisp
(deftest "addition works"
  (assert= (+ 1 2) 3)
  (assert-true (> 5 3))
  (assert-nil (first '())))
```

```bash
lisprint test                    # *_test.lisp を自動検出
lisprint test tests/my_test.lisp # 指定ファイル
```

## 組み込み関数一覧

### 算術

| 関数 | 例 | 説明 |
|---|---|---|
| `+` | `(+ 1 2 3)` → `6` | 加算 (可変長) |
| `-` | `(- 10 3)` → `7` | 減算 / 単項マイナス |
| `*` | `(* 2 3 4)` → `24` | 乗算 (可変長) |
| `/` | `(/ 10 3)` → `3` | 除算 |
| `mod` | `(mod 10 3)` → `1` | 剰余 |

### 比較

| 関数 | 例 | 説明 |
|---|---|---|
| `=` | `(= 1 1)` → `true` | 等価 |
| `<` | `(< 1 2)` → `true` | 未満 |
| `>` | `(> 2 1)` → `true` | 超過 |
| `<=` | `(<= 1 1)` → `true` | 以下 |
| `>=` | `(>= 2 1)` → `true` | 以上 |
| `not` | `(not false)` → `true` | 論理否定 |

### リスト

| 関数 | 例 | 説明 |
|---|---|---|
| `list` | `(list 1 2 3)` | リスト作成 |
| `cons` | `(cons 0 '(1 2))` → `(0 1 2)` | 先頭に追加 |
| `first` | `(first '(1 2 3))` → `1` | 先頭要素 |
| `rest` | `(rest '(1 2 3))` → `(2 3)` | 残り |
| `nth` | `(nth '(a b c) 1)` → `b` | N番目 |
| `count` | `(count '(1 2 3))` → `3` | 要素数 |
| `empty?` | `(empty? '())` → `true` | 空判定 |
| `concat` | `(concat '(1) '(2 3))` → `(1 2 3)` | 結合 |

### 型チェック

| 関数 | 説明 |
|---|---|
| `nil?` | nil 判定 |
| `number?` | 数値判定 |
| `string?` | 文字列判定 |
| `list?` | リスト判定 |
| `fn?` | 関数判定 |

### I/O

| 関数 | 説明 |
|---|---|
| `println` | 改行付き出力 |
| `print` | 改行なし出力 |
| `str` | 文字列結合 |

### その他

| 関数 | 説明 |
|---|---|
| `apply` | `(apply + '(1 2 3))` → `6` |
| `identity` | 引数をそのまま返す |

## Prelude (自動ロード)

| 関数 | 説明 |
|---|---|
| `map` | `(map f lst)` — 各要素に f を適用 |
| `filter` | `(filter pred lst)` — 条件を満たす要素 |
| `reduce` | `(reduce f init lst)` — 畳み込み |
| `each` | `(each f lst)` — 副作用用イテレーション |
| `reject` | `(reject pred lst)` — filter の逆 |
| `find` | `(find pred lst)` — 最初にマッチする要素 |
| `flatten` | `(flatten lst)` — ネストを平坦化 |
| `range` | `(range n)` — 0 から n-1 のリスト |
| `comp` | `(comp f g)` — 関数合成 |
| `inc` / `dec` | +1 / -1 |
| `zero?` / `pos?` / `neg?` | 数値判定 |
| `even?` / `odd?` | 偶奇判定 |
| `when` / `unless` | 条件付き実行 (マクロ) |
