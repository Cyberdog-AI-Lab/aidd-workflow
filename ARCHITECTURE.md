# workflow-runner アーキテクチャ

## 概要

`workflow-runner` は AI ツール上で実行されるワークフローを **決定論的に制御する** Rust 製エンジン。

### 解決する問題

| 問題 | 解決方法 |
|------|---------|
| Claude が前ステップを飛ばして完了と報告する | `requires` / `guards` のゲートチェック（Rust）|
| ステップの内容を毎回 Claude が解釈する（非決定論） | `actions` フィールドで実行内容を宣言的に記述 |
| セッションをまたいで作業が中断する | SQLite でステップ状態を永続化 |
| AI ツール固有の API に依存する | `providers/` 層で差異を吸収し `adapters/` は抽象化 |
| 複数ワークフローを同時進行できない | SQLite + `--workflow-id` で並行管理 |
| 設計フェーズを飛ばして実装が始まる | ステップ単位の `guards`（前提ファイル存在チェック） |
| 対象外のファイルを誤って編集する | ステップ単位の `allow_files` / `deny` 制約 |
| 独立したステップが直列待ちになる | `parallel` ブロックで複数サブステップを同時返却 |

### 設計原則

> **Rust バイナリが「何をすべきか」を決定し、AI ツールが「どう実行するか」を担う。**
> 両者は JSON の CLI プロトコルで通信する。

---

## レイヤー構成

```
┌──────────────────────────────────────────────────────────┐
│                     AI ツール層                           │
│   Claude Code Skill          (将来) Cursor / Generic      │
│   SKILL.md が薄いブリッジ    アダプター追加で対応         │
└─────────────┬────────────────────────────────────────────┘
              │  CLI 呼び出し（JSON 入出力）
┌─────────────▼────────────────────────────────────────────┐
│            workflow-runner（Rust バイナリ）                │
│                                                          │
│  ┌──────────┐ ┌───────────┐ ┌──────────┐ ┌───────────┐  │
│  │ config 層 │ │ engine 層 │ │adapters層│ │providers層│  │
│  │ YAML パース│ │ DAG 評価  │ │AI非依存の│ │AI固有JSON │  │
│  │ imports  │ │ 状態管理  │ │抽象インタ│ │パース実装 │  │
│  │ 型定義   │ │ gate/guard│ │フェース  │ │           │  │
│  └──────────┘ └───────────┘ └──────────┘ └───────────┘  │
└─────────────┬────────────────────────────────────────────┘
              │  ファイル I/O
┌─────────────▼────────────────────────────────────────────┐
│                   状態層（SQLite）                         │
│  .workflow/config.yml       .workflow/workflow.db         │
│  .workflow/commands/        .workflow/workflow.schema.json│
│  .workflow/workflows/                                     │
└──────────────────────────────────────────────────────────┘
```

**依存方向**（一方向のみ）:
```
main.rs
  └── engine/*     → config/* のみ
  └── adapters/*   → providers/* + engine/* + config/*
  └── providers/*  → config/* のみ（AI 固有知識はここで封じ込め）
  └── infra/*      → engine/* + config/*（SQLite, settings.json 書き込み）
```

---

## ディレクトリ構成（v5 現在）

```
aidd-workflow/
├── install.sh                           バイナリインストールスクリプト（macOS/Linux）
├── src/
│   ├── main.rs                          CLI エントリポイント（clap）
│   ├── config/
│   │   ├── types.rs                     Config / Workflow / Step / SubStep / Action 型定義（Pure）
│   │   └── loader.rs                    YAML ロード・imports 解決・バリデーション（Shell）
│   ├── engine/
│   │   ├── state.rs                     WorkflowState 型定義・純粋メソッド（Pure）
│   │   ├── store.rs                     SQLite 読み書き・WorkflowStore trait（Shell）
│   │   ├── dag.rs                       requires 依存グラフ評価・サブステップ DAG（Pure）
│   │   ├── gate.rs                      gate 条件 + guards チェック（Pure）
│   │   └── executor.rs                  next_actions の構築・テンプレート解決（Pure）
│   ├── adapters/
│   │   └── hooks/
│   │       └── hook_handler.rs          Claude Code フック処理（providers 経由・Shell）
│   ├── providers/
│   │   └── claude_code/
│   │       └── hook_parser.rs           Claude Code hook JSON → 型安全な構造体（Pure）
│   ├── infra/
│   │   └── settings_writer.rs           .claude/settings.json の生成・更新（Shell）
│   └── protocol/
│       ├── input.rs                     report コマンドの stdin 型（Pure）
│       └── output.rs                    JSON 出力型・テーブルフォーマッター（Pure）
├── .workflow/
│   ├── config.yml                       ワークフロー定義（ユーザーが編集）
│   ├── commands/                        コマンド定義（imports で読み込む）
│   │   └── default.yml
│   ├── workflows/                       ワークフロー定義（imports で読み込む）
│   │   ├── bug-fix.yml
│   │   └── feature.yml
│   ├── workflow.db                      実行状態 SQLite（自動生成、gitignore）
│   └── workflow.schema.json             JSON Schema（IDE サポート用。ランタイム検証は validate() が担う）
└── .claude/
    ├── hooks/
    │   └── post-edit-rust-checks.sh     .rs 編集後に fmt / lint / test を自動実行
    └── skills/workflow-orchestrator/    workflow-runner を呼ぶ薄いブリッジ
```

---

## config.yml スキーマ

### imports

```yaml
# .workflow/config.yml（メインファイル）
imports:
  - commands/default.yml     # コマンド定義ファイル
  - workflows/bug-fix.yml    # ワークフロー定義ファイル
  - workflows/feature.yml

# インライン定義と imports はマージされる（インラインが優先）
commands:
  extra: make extra
```

```yaml
# .workflow/commands/default.yml
commands:
  test: make test
  lint: make lint
  build: make build
```

- `imports` のパスはメイン config.yml と同じディレクトリからの相対パス
- 再帰的なインポートを許容する（循環参照は検出してエラー）
- `commands` は同一キーがある場合、インライン定義が優先される
- `workflows` は同一 slug がある場合エラー

### ステップの基本構造（v5）

```yaml
workflows:
  <slug>:
    name: ワークフロー名
    description: 説明（任意）
    steps:
      - id: <step-id>             # 一意なステップ ID（checklist_key の代替）
        name: ステップ名
        description: 説明

        # --- 実行制御 ---
        actions:                  # Agent / Skill のみ（type: run は廃止）
          - type: agent
            prompt: "..."
          - type: skill
            skill: security-review

        # --- 依存と前提条件 ---
        requires: [<step-id>, ...] # 依存ステップ（DAG の辺）
        guards:                    # 前ステップの成果物チェック
          - step: design           # このステップが完了していること
            required_files:        # かつ以下のファイルが存在すること
              - "docs/specs/*.md"

        # --- アクセス制御（InProgress 中のみ有効）---
        allow_files:              # 編集を許可するファイルパターン（空なら制限なし）
          - "src/**"
          - "/tests\/.*\.rs$/"   # / で囲まれた場合は正規表現
        deny:
          files:                  # 編集を禁止するファイルパターン
            - "/\.env/"
          commands:               # 実行を禁止するコマンドパターン
            - "git push"
```

### アクション型（v5）

| `type` | フィールド | 説明 |
|--------|-----------|------|
| `agent` | `prompt`, `background: bool` | サブエージェント起動。`background: true` で並列実行可 |
| `skill` | `skill`, `args: []` | スキル呼び出し |

> **廃止**: `type: run` は `actions` に移行。`type: workflow` は `imports:` での子ワークフロー埋め込みに移行。

### ステップの形態

```yaml
# 1. 自動ステップ（agent/skill アクション）
- id: test
  actions:
    - type: agent
      prompt: "{{commands.test}} を実行してテストがすべてパスすることを確認してください"

# 2. 並列ステップ（parallel ブロック）
- id: quality-check
  parallel:
    - id: run-test
      actions:
        - type: agent
          prompt: "make test を実行してください"
    - id: run-lint
      actions:
        - type: agent
          prompt: "make lint を実行してください"
      requires: [run-test]

# 3. 手動ステップ（actions も parallel もなし）
- id: design
  description: 実装方針を整理して記録する
  allow_files:
    - "docs/**"
```

---

## CLI プロトコル

### コマンド一覧

```
workflow-runner [--workflow-id <id>] [--cwd <path>] <command>

start <workflow>          ワークフロー開始 → 最初の actions を JSON で返す
                          出力の workflow_id を以降の --workflow-id に使用する
next                      次の actions を JSON で返す
report                    アクション実行結果を記録（stdin: JSON）
complete <step-id>        ステップ完了（ゲートチェック付き）→ 次の actions を返す
resume                    中断ワークフローの再開情報を返す
status [--format json|table]   現在の実行状態を返す
validate [--format json|text]  config.yml を検証する
list                      ワークフロー一覧を返す
hook <event-type>         Claude Code フックイベントを処理（stdin: hook JSON / cwd 自動抽出）
init                      .claude/settings.json を生成・初期化する
update                    .claude/settings.json の workflow-runner hook 設定を更新する
dump-schema               config.yml の JSON Schema を stdout に出力する
```

### `--workflow-id` の動作

```bash
# 開始時に workflow_id を受け取る
workflow-runner start bug-fix
# → { "workflow_id": "4fd261ba-...", "status": "started", "actions": [...] }

# 以降のコマンドに workflow_id を渡す（複数並行時は必須）
workflow-runner --workflow-id 4fd261ba-... complete reproduce

# 1つしか active がない場合は省略可（自動選択）
workflow-runner next
```

### スキルとの通信フロー（v5）

```
SKILL.md（Claude Code）                workflow-runner
        │                                      │
        │── start bug-fix ────────────────────▶│ workflow.db 作成
        │◀── { workflow_id: "...", actions: [...] } │
        │                                      │
        │── [actions(agent/skill) を実行] ──────│
        │                                      │
        │── report ───────────────────────────▶│ workflow.db 更新
        │◀── { ok: true } ────────────────────│
        │                                      │
        │── complete test ────────────────────▶│ gate / guards チェック
        │◀── { allowed: true, next: {...} } ───│ workflow.db 更新
```

### Hook イベントの stdin JSON（v5）

`workflow-runner hook <event>` は `--cwd` フラグなしで stdin JSON から `cwd` を自動抽出する。

```bash
# settings.json に登録するコマンド（シェルスクリプト不要）
"command": "workflow-runner hook pre-edit"
"command": "workflow-runner hook pre-taskupdate"
```

---

## 状態管理（SQLite）

### ファイルパス

```
.workflow/workflow.db   # SQLite DB（gitignore で除外）
```

### スキーマ

```sql
CREATE TABLE workflow_runs (
    workflow_id   TEXT PRIMARY KEY,      -- UUIDv4（start 時に生成）
    cwd           TEXT NOT NULL,         -- プロジェクトルート絶対パス
    workflow      TEXT NOT NULL,         -- ワークフロー slug
    status        TEXT NOT NULL DEFAULT 'active',
    started_at    TEXT NOT NULL,
    completed_at  TEXT
);

CREATE TABLE step_states (
    workflow_id    TEXT NOT NULL REFERENCES workflow_runs(workflow_id) ON DELETE CASCADE,
    step_id        TEXT NOT NULL,        -- "step-id" または "parent/sub"
    status         TEXT NOT NULL DEFAULT 'pending',
    started_at     TEXT,
    completed_at   TEXT,
    PRIMARY KEY (workflow_id, step_id)
);

CREATE TABLE action_reports (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    workflow_id    TEXT NOT NULL REFERENCES workflow_runs(workflow_id) ON DELETE CASCADE,
    step_id        TEXT NOT NULL,
    action_index   INTEGER NOT NULL,
    action_type    TEXT NOT NULL,
    exit_code      INTEGER,
    stdout         TEXT,
    recorded_at    TEXT NOT NULL
);
```

### 複数ワークフロー並行の識別

| ケース | 動作 |
|--------|------|
| `--workflow-id` あり | 指定した workflow_id の状態を操作 |
| `--workflow-id` なし・active 1件 | 自動選択 |
| `--workflow-id` なし・active 複数 | エラー（`--workflow-id` の指定を促す） |
| hook イベント | `cwd` で active を絞り込み（全件チェック） |

---

---

## ファイル制約と Guards

### allow_files / deny

InProgress 状態のステップに `allow_files` または `deny.files` が設定されている場合、
`PreToolUse(Edit/Write)` hook が編集を制御する。

```
Claude が Edit/Write を実行しようとする
        ↓
PreToolUse(Edit/Write) hook 発火
        ↓
workflow-runner hook pre-edit（stdin: hook JSON）
        ↓
InProgress ステップの allow_files / deny.files を確認
        ↓
allow_files に合致しない → {"decision":"block","reason":"..."} を返す
deny.files に合致する → {"decision":"block","reason":"..."} を返す
"decision":"ask" → Claude に確認を促す
問題なし → 何も返さない（編集を許可）
```

### guards（前提ファイル存在チェック）

```yaml
guards:
  - step: design           # design ステップが Completed であること
    required_files:        # かつ以下のパターンにマッチするファイルが存在すること
      - "docs/specs/*.md"  # design の allow_files に該当するファイル
```

- `gate.rs` の `check()` が `requires` 確認後に `guards` を評価する
- `required_files` は glob パターンでプロジェクトルートからの相対パスで評価
- 未充足の場合は `allowed: false` と理由を返す

---

## DAG 評価

`requires` フィールドが有向非巡回グラフ（DAG）の辺を形成する。

```
例: release ワークフロー

design ──▶ implement ──▶ quality-check ──▶ complete
                              │
                         ┌────┴──────┐
                     run-test    run-lint   ← 並列サブステップ
```

`dag.executable_items()` はこのグラフを評価して **同時に実行可能なアイテム** を返す。

- 通常ステップ: `requires` が全て `Completed` になったステップを返す
- 並列ブロック: 全サブステップを同時に返す（各サブステップの `requires` も評価する）

---

## アダプター / プロバイダー設計

### 層の責務分担

| 層 | 責務 | 具体例 |
|----|------|--------|
| `providers/claude_code/` | Claude Code 固有の hook JSON を型安全な構造体にパース | `PostBashEvent`, `PreEditEvent` |
| `adapters/hooks/` | パース済みイベントを受け取り、engine 層を呼んで `HookResponse` を返す | `handle_pre_edit()` |
| `infra/settings_writer` | settings.json の生成・更新 | `write_settings_json()` |

`adapters/` は `providers/` を使うが、具体的な JSON 形式を知らない。
`providers/` は `engine/` や `config/` を知らない（純粋な変換のみ）。

### claude-code アダプターのフックイベント処理

| フック | タイミング | 処理内容 |
|--------|-----------|---------|
| `pre-edit` | Edit/Write 実行前 | allow_files / deny.files チェック → block / ask |
| `pre-bash` | Bash 実行前 | deny.commands チェック → block |
| `pre-taskupdate` | TaskUpdate 実行前 | no-op |
| `post-edit` | Edit/Write 実行後 | config.yml 変更検出 → スキーマ検証警告 |

フックはエラーで終了しない（exit 0 固定）。ワークフロー外の操作を干渉しない設計。

### 開発者向けフック

`.claude/hooks/post-edit-rust-checks.sh` は `.rs` ファイル編集後に自動で品質チェックを実行する開発補助フック。ワークフローエンジンとは独立して動作する。

```
.rs ファイルの Edit/Write
        ↓
post-edit-rust-checks.sh
        ↓
make fmt   (cargo fmt --all)
make lint  (cargo clippy -D warnings)
make test  (cargo test)
        ↓
失敗時: exit 1 → Claude にエラーとして通知
```

### 自律駆動型ワークフロー（Phase 5 で再設計予定）

Claude Code セッション外からワークフローを外部プロセスとして制御する仕組み。
`start` → `report` → `complete` の CLI プロトコルを外部コントローラーが駆動する。
詳細は `PLAN.md` の Phase 5 を参照。

---

## クレート一覧

| クレート | バージョン | 用途 |
|---------|-----------|------|
| `clap` | 4 | CLI パース（derive マクロ） |
| `serde` / `serde_json` / `serde_yaml` | 1 | 設定・状態の JSON/YAML シリアライズ |
| `anyhow` | 1 | エラーハンドリング |
| `uuid` | 1 | ワークフロー ID 生成（v4） |
| `chrono` | 0.4 | タイムスタンプ（UTC） |
| `rusqlite` | 0.31 | SQLite 状態管理（bundled feature） |
| `glob` | 0.3 | allow_files / guards のパターンマッチ |
| `regex` | 1 | `/pattern/` 形式の正規表現マッチ |
| `comfy-table` | 7 | `status --format table` のターミナルテーブル描画 |
| `tempfile` | 3 | テスト用一時ディレクトリ（dev-dependency） |

---

## コマンド別 実行フロー詳細

### `start <workflow>`

```
main()
  └─ cmd_start(cwd, workflow_name)
      ├─ config::loader::load_config(cwd)
      │    ├─ load_config_recursive(".workflow/config.yml", visited={})
      │    │    ├─ YAML をパース → Config
      │    │    └─ imports を再帰解決（循環検出あり）
      │    └─ validate(&config)   # requires/guards の整合性チェック
      │
      ├─ config.workflows.get(workflow_name)   # 未定義なら bail!
      │
      ├─ WorkflowState::new(workflow_name, wf)
      │    ├─ workflow_id = Uuid::new_v4()
      │    ├─ started_at = Utc::now()
      │    └─ steps: 全ステップ + 全サブステップ を Pending で初期化
      │
      ├─ engine::store::save_state(cwd, &state)   # SQLite へ upsert
      │
      ├─ executor::build_next(wf, &state, &config)
      │    ├─ dag::is_workflow_complete() → false（初回なので false）
      │    ├─ dag::executable_items(wf, state)
      │    │    └─ requires が空 かつ Pending なステップを返す
      │    └─ ActionItem を構築（テンプレート解決含む）
      │
      └─ JSON 出力: { workflow_id, status: "started", actions: [...] }
```

### `next` / `resume`

```
cmd_next(cwd, workflow_id?)
  ├─ load_config() + resolve_state()   # workflow_id 指定時は load_state_by_id()
  └─ executor::build_next()  →  JSON 出力（status: in_progress / blocked / completed）
```

### `report`（stdin: JSON）

```
cmd_report(cwd, workflow_id?)
  ├─ read_stdin() → serde_json::from_str::<ReportInput>()
  ├─ load_config() + resolve_state()
  │
  ├─ step.status が Pending なら InProgress に遷移・started_at を記録
  ├─ step.action_reports に ActionReport を追加
  │     { action_index, action_type, exit_code, stdout, recorded_at }
  │
  ├─ dag::parent_of(step_id) → Some(parent_id) の場合
  │    └─ state.sync_parallel_parent(parent_id, wf)
  │         ├─ 全サブステップ Completed → 親を Completed に遷移
  │         └─ 1つでも Started → 親を InProgress に遷移
  │
  ├─ save_state()
  └─ JSON 出力: { ok: true, step_id }
```

### `complete <step_id>`

```
cmd_complete(cwd, step_id, workflow_id?)
  ├─ load_config() + resolve_state()
  │
  ├─ engine::gate::check(wf, &state, step_id, cwd)
  │    ├─ step.requires: 依存ステップが全て Completed か確認
  │    └─ step.guards:
  │         ├─ 指定ステップが Completed か確認
  │         └─ required_files パターンが cwd 以下に存在するか確認（再帰 walk）
  │
  ├─ gate NG → CompleteOutput { allowed: false, reason: ... } を返して終了
  │
  ├─ step.status = Completed、completed_at = Utc::now() を記録
  ├─ dag::parent_of(step_id) → Some → sync_parallel_parent()
  ├─ save_state()
  │
  ├─ executor::build_next()  →  next アクションを取得
  │    └─ dag::is_workflow_complete() = true なら
  │         └─ store::clear_state_by_id()  # workflow_runs.status = 'completed'
  │
  └─ CompleteOutput { allowed: true, next: WorkflowOutput }
```

### `hook <event_type>`（stdin: hook JSON）

```
cmd_hook(cwd, event_type)
  ├─ read_stdin()
  ├─ extract_cwd_from_stdin()   # stdin JSON の cwd フィールドを優先使用
  │
  ├─ "post-bash"
  │    └─ handle_post_bash()   → no-op（常に Ok(())）
  │
  ├─ "pre-taskupdate"
  │    └─ no-op（常に None を返す）
  │
  ├─ "post-edit"
  │    ├─ serde_json::from_str::<PostEditEvent>()
  │    ├─ file_path が ".workflow/config.yml" で終わらなければ → None
  │    └─ load_config() → 失敗なら "[SCHEMA WARNING] ..." を返す
  │
  ├─ "pre-edit"
  │    ├─ serde_json::from_str::<PreEditEvent>()
  │    ├─ load_config() + load_state()
  │    ├─ 絶対パス → cwd からの相対パスに変換
  │    └─ InProgress なステップに対して:
  │         ├─ allow_files 非空 かつ非マッチ → {"decision":"ask","reason":"..."}
  │         └─ deny.files にマッチ → {"decision":"block","reason":"..."}
  │
  └─ "pre-bash"
       ├─ serde_json::from_str::<PreBashEvent>()
       ├─ load_config() + load_state()
       └─ InProgress なステップの deny.commands と照合
            └─ 部分一致 or /regex/ マッチ → {"decision":"block","reason":"..."}
```

### `init` / `update`

```
init:
  ├─ fs::create_dir_all(".workflow/")
  ├─ which workflow-runner で PATH チェック（警告のみ）
  └─ infra::settings_writer::write_settings_json(cwd)
       ├─ build_settings(cwd)
       │    ├─ post-edit-rust-checks.sh が存在すれば PostToolUse フックに追加
       │    └─ PreToolUse: TaskUpdate / Edit / Write / Bash
       │       PostToolUse: Edit / Write
       └─ .claude/settings.json を新規書き込み

update:
  └─ infra::settings_writer::merge_settings_json(cwd)
       ├─ 既存 .claude/settings.json を読み取り（なければ空 JSON）
       ├─ build_settings() で最新のフック設定を生成
       ├─ merge_hook_settings(): workflow-runner フックを置換、それ以外は保持
       └─ .claude/settings.json に書き戻し
```

---

## DAG 評価の詳細（`engine/dag.rs`）

```rust
executable_items(wf, state) -> Vec<String>:
  for step in wf.steps:
    if step.status is Completed | Failed  → skip
    if !requires_met(wf, state, step.requires)  → skip

    if step.parallel is Some(subs):
      for sub in subs:
        key = "{step.id}/{sub.id}"
        if sub.status is Pending | InProgress AND sub.requires met:
          push(key)
    else:
      if step.status is Pending | InProgress:
        push(step.id)
```

並列ブロックは親ステップのキー（`"parent_id"`）ではなく、子のキー（`"parent_id/sub_id"`）を返します。親ステップのステータスは `sync_parallel_parent()` が各 report / complete 時に自動的に更新します。

---

## ステップのステータス遷移

```
Pending
  │  ← report（初回、step_state を InProgress に遷移）
  ▼
InProgress
  │  ← complete → gate::check()（requires / guards）通過
  ▼
Completed
```

並列サブステップ（`parent_id/sub_id`）：
- 各サブステップが独立して `Pending → InProgress → Completed` を辿る
- `sync_parallel_parent()` が呼ばれるたびに：
  - 全サブ Completed → 親を `Completed` に遷移
  - 1つ以上が Started → 親を `InProgress` に遷移

---

## 設定ローダーの import 解決

```
load_config(cwd)
  └─ load_config_recursive(".workflow/config.yml", base=cwd, visited={})
      ├─ path.canonicalize() で絶対パス取得
      ├─ visited に追加（重複 = 循環 → bail!）
      ├─ YAML をパース → Config
      ├─ config.imports を取り出して mem::take（空に置き換え）
      ├─ for import_path in imports:
      │    └─ load_config_recursive(base/.workflow/<import_path>, ...)
      │         └─ 子 Config を merge_into() でマージ（既存キーは上書きしない）
      └─ 再帰完了後、トップレベルで validate() を実行

validate(&config):  ← マージ後の Config にのみ実行（個別インポートファイルには実行しない）
  ├─ 各ワークフロー: steps が空でないか
  ├─ 各ステップ: actions と parallel を同時に持っていないか
  ├─ 各ステップ: requires に未定義ステップが含まれていないか
  └─ 各ステップ: guards.step に未定義ステップが含まれていないか
     → エラーを全件収集して ValidationError として返す

【JSON Schema の生成と整合性】
  workflow.schema.json は schemars クレートで Rust 型から自動生成する。IDE サポート専用。
  ランタイムでのスキーマ検証は行わない。理由:
  - serde_yaml + #[serde(deny_unknown_fields)] が構造的バリデーションを担う
  - requires / guards.step の参照整合性は JSON Schema で表現できない（クロスリファレンス）

  スキーマ再生成:
    workflow-runner dump-schema > .workflow/workflow.schema.json

  schema_file_matches_generated テストが cargo test で整合性を常時検証する。
  Rust 型を変更したら dump-schema でスキーマを更新してコミットする。
```

---

## テンプレート解決

`executor::resolve_template(s, config)` が `{{commands.<key>}}` を config.commands の値で置換します。未定義キーはそのまま残します。

```yaml
commands:
  test: make test

actions:
  - type: agent
    prompt: "{{commands.test}} を実行してください"   # → "make test を実行してください" に展開
  - type: agent
    prompt: "{{commands.lint}} を実行してください"   # → 未定義のため "{{commands.lint}} ..." のまま
```

---

## エラーハンドリング方針

| コンテキスト | 方針 |
|------------|------|
| 通常コマンド（start / next / complete 等） | `anyhow::Result` でエラーを伝播。`main()` が `ErrorOutput` JSON を stderr に出力し exit code 1 で終了 |
| フックハンドラ（`cmd_hook`） | エラーでプロセスをクラッシュさせてはならない。JSON パース失敗・設定不在は `None` を返す（ワークフロー外の操作を干渉しない） |
| gate チェック失敗（requires / guards） | `CompleteOutput { allowed: false, reason }` を返して処理を継続（プロセスは終了しない） |