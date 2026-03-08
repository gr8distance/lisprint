# 標準ライブラリ

`(require "module-name")` で読み込む。

## math

```lisp
(require "math")
```

| 関数 | 例 | 説明 |
|---|---|---|
| `math/abs` | `(math/abs -5)` → `5` | 絶対値 |
| `math/sqrt` | `(math/sqrt 9)` → `3.0` | 平方根 |
| `math/pow` | `(math/pow 2 10)` → `1024.0` | 累乗 |
| `math/sin` | `(math/sin 0)` → `0.0` | 正弦 |
| `math/cos` | `(math/cos 0)` → `1.0` | 余弦 |
| `math/tan` | `(math/tan 0)` → `0.0` | 正接 |
| `math/floor` | `(math/floor 3.7)` → `3.0` | 切り捨て |
| `math/ceil` | `(math/ceil 3.2)` → `4.0` | 切り上げ |
| `math/round` | `(math/round 3.5)` → `4.0` | 四捨五入 |
| `math/min` | `(math/min 1 2 3)` → `1` | 最小値 |
| `math/max` | `(math/max 1 2 3)` → `3` | 最大値 |
| `math/pi` | `(math/pi)` → `3.14159...` | 円周率 |

## str

```lisp
(require "str")
```

| 関数 | 例 | 説明 |
|---|---|---|
| `str/upper` | `(str/upper "hello")` → `"HELLO"` | 大文字化 |
| `str/lower` | `(str/lower "HELLO")` → `"hello"` | 小文字化 |
| `str/trim` | `(str/trim " hi ")` → `"hi"` | 空白除去 |
| `str/split` | `(str/split "a,b,c" ",")` → `("a" "b" "c")` | 分割 |
| `str/join` | `(str/join ", " '("a" "b"))` → `"a, b"` | 結合 |
| `str/contains?` | `(str/contains? "hello" "ell")` → `true` | 含有判定 |
| `str/starts-with?` | `(str/starts-with? "hello" "he")` → `true` | 先頭一致 |
| `str/ends-with?` | `(str/ends-with? "hello" "lo")` → `true` | 末尾一致 |
| `str/replace` | `(str/replace "hello" "l" "r")` → `"herro"` | 置換 |
| `str/len` | `(str/len "hello")` → `5` | 文字数 |
| `str/substr` | `(str/substr "hello" 1 3)` → `"el"` | 部分文字列 |

## fs

```lisp
(require "fs")
```

| 関数 | 例 | 説明 |
|---|---|---|
| `fs/read` | `(fs/read "file.txt")` | ファイル読み込み |
| `fs/write` | `(fs/write "file.txt" "content")` | ファイル書き込み |
| `fs/append` | `(fs/append "file.txt" "more")` | ファイル追記 |
| `fs/exists?` | `(fs/exists? "file.txt")` | 存在確認 |
| `fs/delete` | `(fs/delete "file.txt")` | 削除 |
| `fs/copy` | `(fs/copy "a.txt" "b.txt")` | コピー |
| `fs/rename` | `(fs/rename "old" "new")` | リネーム |

## os

```lisp
(require "os")
```

| 関数 | 説明 |
|---|---|
| `os/exec` | コマンド実行 `(os/exec "ls" "-la")` |
| `os/exit` | プロセス終了 `(os/exit 0)` |
| `os/pid` | PID 取得 |
| `os/arch` | アーキテクチャ |
| `os/name` | OS 名 |
| `env/get` | 環境変数取得 `(env/get "HOME")` |
| `env/set` | 環境変数設定 |
| `env/all` | 全環境変数 (マップ) |
| `dir/list` | ディレクトリ一覧 |
| `dir/create` | ディレクトリ作成 (再帰) |
| `dir/remove` | ディレクトリ削除 (再帰) |
| `dir/cwd` | カレントディレクトリ |
| `path/join` | パス結合 `(path/join "a" "b" "c.txt")` |
| `path/basename` | ファイル名 |
| `path/dirname` | ディレクトリ名 |
| `path/ext` | 拡張子 |
| `path/absolute` | 絶対パスに変換 |

## json

```lisp
(require "json")
```

| 関数 | 例 | 説明 |
|---|---|---|
| `json/parse` | `(json/parse "{\"a\":1}")` → `{:a 1}` | JSON パース |
| `json/encode` | `(json/encode {:a 1})` → `"{\"a\":1}"` | JSON エンコード |

## uuid

```lisp
(require "uuid")
```

| 関数 | 説明 |
|---|---|
| `uuid/v4` | ランダム UUID v4 生成 |

## time

```lisp
(require "time")
```

| 関数 | 説明 |
|---|---|
| `time/now` | Unix タイムスタンプ (秒) |
| `time/millis` | Unix タイムスタンプ (ミリ秒) |
| `time/sleep` | スリープ `(time/sleep 1000)` |

## re (正規表現)

```lisp
(require "re")
```

| 関数 | 例 | 説明 |
|---|---|---|
| `re/match?` | `(re/match? "\\d+" "abc123")` → `true` | マッチ判定 |
| `re/find` | `(re/find "\\d+" "abc123")` → `"123"` | 最初のマッチ |
| `re/find-all` | `(re/find-all "\\d+" "a1b2c3")` → `("1" "2" "3")` | 全マッチ |
| `re/replace` | `(re/replace "\\d" "X" "a1b2")` → `"aXbX"` | 置換 |

## http

```lisp
(require "http")
```

| 関数 | 説明 |
|---|---|
| `http/get` | GET リクエスト |
| `http/post` | POST リクエスト |
| `http/put` | PUT リクエスト |
| `http/delete` | DELETE リクエスト |

レスポンス: `{:status 200 :body "..." :headers {...}}`

```lisp
;; シンプルな GET
(def res (http/get "https://api.example.com/data"))
(println (.status res))
(println (.body res))

;; ヘッダ・ボディ付き POST
(def res (http/post "https://api.example.com/data"
  {:headers {"Content-Type" "application/json"}
   :body (json/encode {:name "Alice"})}))
```

## http/server

```lisp
(require "http/server")
```

| 関数 | 説明 |
|---|---|
| `server/start` | HTTP サーバ起動 (ブロッキング) |

```lisp
(server/start 8080
  (fn (req)
    ;; req = {:method "GET" :url "/" :headers {...} :body ""}
    {:status 200
     :body "Hello!"
     :content-type "text/plain"}))
```

## async

```lisp
(require "async")
```

| 関数 | 説明 |
|---|---|
| `async/spawn` | 新スレッドで関数実行、future 返却 |
| `async/await` | future の完了を待つ |
| `async/channel` | チャネル作成 `{:sender ... :receiver ...}` |
| `async/send` | チャネルに値を送信 |
| `async/recv` | チャネルから値を受信 (ブロッキング) |
| `async/sleep` | スレッドスリープ (ミリ秒) |

```lisp
;; スレッド
(def f (async/spawn (fn () (+ 1 2))))
(println (async/await f))  ;; => 3

;; チャネル
(def ch (async/channel))
(async/spawn (fn () (async/send (.sender ch) 42)))
(println (async/recv (.receiver ch)))  ;; => 42
```
