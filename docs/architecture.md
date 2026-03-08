# アーキテクチャ

## クレート構成

```
lisprint/
├── Cargo.toml              # ワークスペース
├── bin/install              # インストールスクリプト
├── lib/prelude.lisp         # Lisp prelude (暗黙ロード)
├── crates/
│   ├── core/                # lisprint-core
│   │   ├── value.rs         # Arc<Value> 型定義
│   │   ├── parser.rs        # S 式パーサ
│   │   ├── eval.rs          # tree-walk 評価器 (94テスト)
│   │   ├── env.rs           # 環境 (スコープチェーン)
│   │   ├── builtins.rs      # 組み込み関数
│   │   ├── prelude.rs       # prelude ローダ
│   │   └── stdlib/          # Rust ネイティブ stdlib (11モジュール)
│   │       ├── mod.rs       # モジュールレジストリ
│   │       ├── math.rs
│   │       ├── str_mod.rs
│   │       ├── fs.rs
│   │       ├── os.rs
│   │       ├── json.rs
│   │       ├── uuid_mod.rs
│   │       ├── time.rs
│   │       ├── re.rs
│   │       ├── http.rs
│   │       ├── http_server.rs
│   │       └── async_mod.rs
│   ├── compiler/            # lisprint-compiler
│   │   ├── compiler.rs      # AST → Cranelift IR (24テスト)
│   │   └── runtime.rs       # bridge 関数 (FFI)
│   └── cli/                 # lisprint (バイナリ)
│       ├── main.rs          # CLI コマンド + REPL
│       └── project.rs       # プロジェクト管理 (lisp.toml, Bridge)
└── docs/
```

### ユーザープロジェクト構成 (Bridge 使用時)

```
my-app/
├── lisp.toml              # プロジェクト設定 + [dependencies]
├── bridge/                # ユーザー Bridge (Rust FFI)
│   └── <crate>.rs         # register(env) で関数を環境に登録
├── src/
│   └── main.lisp          # エントリポイント
└── .lisprint/build/       # 自動生成 (gitignore 推奨)
    ├── Cargo.toml          # dependencies + lisprint-core path dep
    └── src/
        ├── main.rs         # Lisp ファイル埋め込み + bridge 呼び出し
        └── bridges/
            ├── mod.rs      # mod <crate>; register_all()
            └── <crate>.rs  # bridge/ からコピー
```

## 設計方針

### Tiny Core
処理系コアは最小限に保つ。機能は prelude (Lisp) か stdlib (Rust) に追加する。

### 三層モジュール
1. **Core** — 組み込み関数 (builtins.rs): `+`, `-`, `cons`, `first` など最小限
2. **Prelude** — Lisp で実装 (lib/prelude.lisp): `map`, `filter`, `reduce` など。暗黙ロード
3. **Stdlib** — Rust で実装 (stdlib/): `require` で明示ロード。外部クレート利用可

### Arc\<Value\>
GC 不要。参照カウントでメモリ管理。`Arc<Value>` を使い、クローンは参照カウント +1 のみ。

### Value アクセス
`value.as_int()`, `Value::int(42)` のようにメソッド経由でアクセス。将来の NaN boxing 移行に備える。

### 不変変数
`def` / `let` は不変。可変状態が必要な場合は `atom` を使う。

### NativeFn
```rust
type NativeFn = fn(&[Value]) -> LispResult;
```
環境 (Env) にアクセスしない純粋関数。stdlib は全てこのシグネチャ。

### Env (環境)
```rust
struct Env {
    bindings: HashMap<String, Value>,
    parent: Option<Arc<Env>>,
}
```
スコープチェーンで変数解決。`Arc<Env>` で親環境を共有。

### Stdlib レジストリ
```rust
type ModuleRegisterFn = fn(&mut Env);

fn registry() -> HashMap<&'static str, ModuleRegisterFn> { ... }
```
`require` 時に名前からレジストリを引き、`register(&mut env)` で関数群を環境に登録。

### ユーザー Bridge
ユーザーが `lisprint add <crate>` で追加した Rust クレートを Lisp から利用する仕組み。

**フロー:**
1. `lisprint add <crate>` → lisp.toml に依存追加 + `bridge/<crate>.rs` テンプレート生成
2. ユーザーが `bridge/<crate>.rs` の `register(env)` 内で関数を定義
3. `lisprint run` / `build` 時に bridge/ があれば自動で:
   - `.lisprint/build/` に Cargo プロジェクトを生成
   - `Cargo.toml` にユーザーの依存 + `lisprint-core` (path dep) を記述
   - `src/main.rs` に Lisp ソースを埋め込み + bridge 呼び出しコード生成
   - `cargo build --release` で実行バイナリを生成
4. 生成されたバイナリを実行 (`run`) またはコピー (`build`)

**Bridge ファイルの規約:**
```rust
pub fn register(env: &mut Env) {
    env.define("<crate>/func_name", Value::NativeFn(Arc::new(NativeFnData {
        name: "<crate>/func_name".to_string(),
        func: Box::new(|args| { ... }),
    })));
}
```

## テスト

- **Core**: 94 テスト (eval + parser)
- **Compiler**: 24 テスト (コンパイル + 実行)
- **合計**: 118 テスト

```bash
cargo test
```

## 開発の進捗

| Phase | 内容 | 状態 |
|---|---|---|
| 1 | コアランタイム (パーサ, eval, REPL) | 完了 |
| 2 | マクロ + prelude | 完了 |
| 3 | Rust クレート bridge (stdlib 11モジュール) | 完了 |
| 4 | Cranelift ネイティブコンパイル | 完了 |
| 5 | ユーザー Bridge システム | 完了 |
| Extra | フロー型推論パス | 未着手 |
