# workflow-runner アーキテクチャ

## 概要

`workflow-runner` は AI ツール上で実行されるワークフローを **決定論的に制御する** Rust 製エンジン。

### 解決する問題

| 問題 | 解決方法 |
|------|---------|
| Claude が前タスクを飛ばして完了と報告する | `requires` のゲートチェック（Rust）|
| タスクの内容を毎回 Claude が解釈する（非決定論） | `prompt` / `skills` / `agents` フィールドで実行内容を宣言的に記述 |
| セッションをまたいで作業が中断する | SQLite でタスク状態を永続化 |
| AI ツール固有の API に依存する | `providers/` 層で差異を吸収し `adapters/` は抽象化 |
| 複数ワークフローを同時進行できない | SQLite + `--workflow-id` で並行管理 |
| 対象外のファイルを誤って編集する | タスク単位の `outputs` / `deny` 制約 |
| 独立したタスクが直列待ちになる | `agents` フィールドで複数カスタムエージェントを並列起動 |
| 中間成果物をレビューなしに進める | `approval: true` で開発者承認ゲートを設置 |

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
│  │ 型定義   │ │ gate チェック│ │フェース  │ │           │  │
│  └──────────┘ └───────────┘ └──────────┘ └───────────┘  │
└─────────────┬────────────────────────────────────────────┘
              │  ファイル I/O
┌─────────────▼────────────────────────────────────────────┐
│                   状態層（SQLite）                         │
│  .workflow/config.yml       .workflow/workflow.db         │
│  .workflow/workflow.schema.json                           │
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

## ディレクトリ構成

```
aidd-workflow/
├── install.sh                           バイナリインストールスクリプト（macOS/Linux）
├── src/
│   ├── main.rs                          CLI エントリポイント（clap）
│   ├── config/
│   │   ├── types.rs                     Config / Workflow / Task 型定義（Pure）
│   │   └── loader.rs                    YAML ロード・imports 解決・バリデーション（Shell）
│   ├── engine/
│   │   ├── state.rs                     WorkflowState 型定義・純粋メソッド（Pure）
│   │   ├── store.rs                     SQLite 読み書き・ステータス管理（Shell）
│   │   ├── dag.rs                       requires 依存グラフ評価（Pure）
│   │   ├── gate.rs                      requires / agents ゲートチェック（Pure）
│   │   └── executor.rs                  next_tasks の構築・テンプレート解決（Pure）
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
│   ├── workflow.db                      実行状態 SQLite（自動生成、gitignore）
│   └── workflow.schema.json             JSON Schema（IDE サポート用。ランタイム検証は validate() が担う）
└── .claude/
    ├── agents/                          カスタムエージェント定義（agents: フィールドで参照）
    │   └── <name>.md
    ├── hooks/
    │   └── post-edit-rust-checks.sh     .rs 編集後に fmt / lint / test を自動実行
    └── skills/workflow-runner/          workflow-runner を呼ぶ薄いブリッジ
```

---

## config.yml スキーマ

### imports

```yaml
# .workflow/config.yml（メインファイル）
imports:
  - vars/default.yml         # 変数定義ファイル
  - workflows/bug-fix.yml    # ワークフロー定義ファイル

# インライン定義と imports はマージされる（インラインが優先）
vars:
  extra: make extra
```

- `imports` のパスは **`.workflow/` ディレクトリからの相対パス**（config.yml が置かれている場所からの相対ではない）
  - 例: `imports: [workflows/bug-fix.yml]` → `.workflow/workflows/bug-fix.yml` を参照
  - `.workflow/` 外へのパストラバーサル（`../` など）はエラー
- 再帰的なインポートを許容する（循環参照は検出してエラー）
- ダイアモンドインポート（A→B→shared, A→C→shared）は **許容する**
  - `visited` は全体のセットではなく DFS スタックとして機能し、サブツリー完了後にエントリを削除することで同一ファイルの再インポートを許可している
- `vars` は同一キーがある場合、インライン定義が優先される
- `workflows` は同一 slug がある場合エラー

### タスクの基本構造

```yaml
workflows:
  <slug>:
    name: ワークフロー名
    description: 説明（任意）
    tasks:
      - id: <task-id>              # 一意なタスク ID（kebab-case）

        # --- 実行モード（排他: prompt/skills と agents は共存不可）---
        task: "タスク名"           # 簡潔なタスク名（手動タスクでは必須、他は任意）
        prompt: "..."              # エージェントに渡すプロンプト（{{vars.key}} 展開あり）
        skills:                    # 呼び出すスキルのリスト
          - security-review
        agents:                    # .claude/agents/<name>.md を参照して並列起動
          - run-test
          - run-lint

        # --- 承認ゲート ---
        approval: true             # true の場合、完了後に開発者承認を待つ

        # --- 依存と前提条件 ---
        requires: [<task-id>, ...]  # 依存タスク（DAG の辺）

        # --- アクセス制御（InProgress 中のみ有効）---
        outputs:                   # 編集可能ファイルパターン（空なら制限なし）
          - "src/**"
          - "/tests\\/.*\\.rs$/"   # / で囲まれた場合は正規表現
        deny:
          files:                   # 編集を禁止するファイルパターン
            - "/\\.env/"
          commands:                # 実行を禁止するコマンドパターン
            - "git push"
```

### タスクの形態

```yaml
# 1. 自動タスク（prompt を指定）
- id: implement
  task: 実装する
  prompt: "設計に従って実装し、{{vars.test}} でテストを確認してください"
  outputs:
    - "src/**"

# 2. スキルタスク（skills を指定）
- id: review
  task: レビューする
  skills:
    - security-review

# 3. prompt + skills の併用
- id: implement-with-review
  task: 実装してレビューする
  prompt: "レビューしてください"
  skills:
    - security-review

# 4. カスタムエージェントタスク（agents を指定）
- id: quality-check
  task: 品質チェック
  agents:
    - run-test    # → .claude/agents/run-test.md
    - run-lint    # → .claude/agents/run-lint.md

# 5. 手動タスク（prompt / skills / agents すべて省略、task 必須）
- id: design
  task: 設計する（task が必須）
  outputs:
    - "docs/**"
  approval: true  # 設計完了後に承認を取ってから次へ
```

### カスタムエージェント（`.claude/agents/<name>.md`）

`agents:` フィールドで指定する名前は `.claude/agents/<name>.md` のエージェント定義を参照する。
エージェントの動作・ツール・システムプロンプトはそのファイルで定義する。
`config.yml` は名前の参照のみを持ち、エージェントの実装知識を持たない。

---

## CLI プロトコル

### コマンド一覧

```
workflow-runner [--workflow-id <id>] [--cwd <path>] <command>

run <workflow>                  自律実行モード（常駐デーモン）
                               Channels webhook 経由でタスクを Claude Code に push し、
                               HTTP コールバック（:8789）で完了を受け取りループする
start <workflow>               ワークフロー開始 → 最初の tasks を JSON で返す
                               出力の workflow_id を以降の --workflow-id に使用する
next                           次の tasks を返す
                               awaiting_approval 状態では承認として機能し、次へ進む
report                         タスク実行結果を記録（stdin: JSON）
complete <task-id>             タスク完了（ゲートチェック付き）→ 次の tasks を返す
reject <task-id> [--reason]    承認待ちタスクを却下してやり直す
resume                         中断ワークフローの再開情報を返す
status [--format json|table]   現在の実行状態を返す
validate [--format json|text]  config.yml を検証する
list                           ワークフロー一覧を返す
hook <event-type>              Claude Code フックイベントを処理（stdin: hook JSON）
setup                          .claude/settings.json の workflow-runner hook 設定を更新する
dump-schema                    config.yml の JSON Schema を stdout に出力する
```

### WorkflowOutput の `status` 値

| 値 | 意味 |
|----|------|
| `started` | `start` コマンド成功直後の初回応答 |
| `in_progress` | 実行可能なタスクが 1 件以上ある |
| `blocked` | ワークフローは完了していないが、依存関係により現在実行可能なタスクが 0 件 |
| `awaiting_approval` | `approval: true` タスクの完了後、開発者承認待ち |
| `completed` | 全タスクが Completed に遷移してワークフロー完了 |

### `--workflow-id` の動作

```bash
# 開始時に workflow_id を受け取る
workflow-runner start bug-fix
# → { "workflow_id": "4fd261ba-...", "status": "started", "tasks": [...] }

# 以降のコマンドに workflow_id を渡す（複数並行時は必須）
workflow-runner --workflow-id 4fd261ba-... complete reproduce

# 1つしか active / awaiting_approval がない場合は省略可（自動選択）
workflow-runner next
```

### スキルとの通信フロー

```
SKILL.md（Claude Code）                workflow-runner
        │                                      │
        │── start bug-fix ────────────────────▶│ workflow.db 作成
        │◀── { workflow_id, status:"started", tasks:[...] }
        │                                      │
        │── [tasks を dispatch] ───────────────│
        │                                      │
        │── report ───────────────────────────▶│ workflow.db 更新
        │◀── { ok: true } ────────────────────│
        │                                      │
        │── complete implement ───────────────▶│ gate チェック
        │◀── { allowed: true,                 │ workflow.db 更新
        │      next: { status: "awaiting_approval" } }
        │                                      │
        │  [ユーザーに承認確認]                 │
        │── next ─────────────────────────────▶│ 承認 → active に戻す
        │◀── { status:"in_progress", tasks:[...] }
```

### `run` コマンドの通信フロー（自律実行モード）

```
workflow-runner run <workflow>          channels/webhook.ts       Claude Code セッション
（オーケストレーター :8789）               （MCP サーバー :8788）    （ワーカー、常時待機）
        │                                       │                        │
        │── POST :8788 {task_id, prompt, …} ──▶│                        │
        │                                       │── <channel> event ────▶│
        │                                       │                        │ タスク実行
        │◀── POST :8789/complete/{task_id} ─────────────────────────────│
        │ complete() → build_next()             │                        │
        │── POST :8788 {next_task, …} ─────────▶│                       │
        │   …（ループ）                          │                        │
        │                                       │                        │
        │  ※ approval: true タスク完了時         │                        │
        │    → status = awaiting_approval       │                        │
        │    → POST :8789/next で承認           │                        │
        │    → POST :8789/reject/:id で却下     │                        │
```

### Hook イベントの stdin JSON

`workflow-runner hook <event>` は `--cwd` フラグなしで stdin JSON から `cwd` を自動抽出する。

```bash
# settings.json に登録するコマンド（シェルスクリプト不要）
"command": "workflow-runner hook pre-edit"
"command": "workflow-runner hook pre-bash"
"command": "workflow-runner hook post-edit"
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
    --   'active'            : 通常実行中
    --   'awaiting_approval' : approval: true タスク完了後、承認待ち
    --   'completed'         : ワークフロー完了
    started_at    TEXT NOT NULL,
    completed_at  TEXT
);

CREATE INDEX IF NOT EXISTS idx_runs_cwd_status
    ON workflow_runs(cwd, status);       -- cwd + status の複合インデックス（load_state の高速化）

CREATE TABLE step_states (
    workflow_id    TEXT NOT NULL REFERENCES workflow_runs(workflow_id) ON DELETE CASCADE,
    step_id        TEXT NOT NULL,        -- "task-id" または "parent-id/agent-name"
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
    action_type    TEXT NOT NULL,        -- "agent" | "skill" | "reject" など
    exit_code      INTEGER,
    stdout         TEXT,
    recorded_at    TEXT NOT NULL
);
```

### 複数ワークフロー並行の識別

| ケース | 動作 |
|--------|------|
| `--workflow-id` あり | 指定した workflow_id の状態を操作 |
| `--workflow-id` なし・active/awaiting_approval 1件 | 自動選択 |
| `--workflow-id` なし・複数件 | エラー（`--workflow-id` の指定を促す） |
| hook イベント | `cwd` で絞り込み（active / awaiting_approval を対象） |

---

## ファイル制約

### outputs / deny

InProgress 状態のタスクに `outputs` または `deny.files` が設定されている場合、
`PreToolUse(Edit/Write)` hook が編集を制御する。

```
Claude が Edit/Write を実行しようとする
        ↓
PreToolUse(Edit/Write) hook 発火
        ↓
workflow-runner hook pre-edit（stdin: hook JSON）
        ↓
InProgress タスクの outputs / deny.files を確認
        ↓
【評価順序: deny.files → outputs の順で評価する】
1. deny.files に合致する → {"decision":"block","reason":"..."} を返す  ← 最優先
2. outputs に合致しない → {"decision":"ask","reason":"..."} を返す
3. 問題なし → 何も返さない（編集を許可）
```

> **注意**: `deny.files` は `outputs` より先に評価される。ファイルが `outputs` の範囲外であっても、`deny.files` に合致する場合は `ask` ではなく `block` になる。

---

## DAG 評価

`requires` フィールドが有向非巡回グラフ（DAG）の辺を形成する。

```
例: release ワークフロー

design ──▶ implement ──▶ quality-check ──▶ complete
                              │
                         ┌────┴──────┐
                     run-test    run-lint   ← .claude/agents/ のカスタムエージェント
```

`dag.executable_items()` はこのグラフを評価して **現在実行可能なタスク** を返す。

- 通常タスク（prompt/skills/手動）: `requires` が全て `Completed` かつ `Pending or InProgress` を返す
- agent タスク: `requires` が全て `Completed` かつ **`Pending` のみ** を返す
  - InProgress の agent タスクは再 dispatch しない（エージェントは既に起動済み）

---

## approval フロー

`approval: true` を持つタスクが `complete` されると、ワークフローは `awaiting_approval` 状態に遷移する。

```
complete <task>（approval: true）
        ↓
gate チェック通過 → task を Completed に
        ↓
workflow_runs.status = 'awaiting_approval'
        ↓
{ next: { status: "awaiting_approval", tasks: [] } }
        ↓
SKILL.md がユーザーに確認

    承認 → next         → status = 'active' → 次タスクを返す
    却下 → reject <id>  → status = 'active' → タスクを InProgress に戻して再 dispatch
```

---

## agents タスクの完了フロー

```
build_next が { task_id: "quality-check", agents: ["run-test", "run-lint"] } を返す
        ↓
SKILL.md が run-test / run-lint を並列起動
        ↓
run-test 完了 → complete quality-check/run-test → gate パス（サブは常に通過）
run-lint 完了 → complete quality-check/run-lint → gate パス
        ↓
complete quality-check
        ↓
gate::check:
  - requires [implement] → Completed? ✅
  - agents run-test → Completed? ✅
  - agents run-lint → Completed? ✅
→ 通過 → quality-check を Completed に → 次タスクへ
```

**注意**: `sync_agents_parent()` は親タスクを `Pending → InProgress` に遷移させるが、
**自動 Completed** にはしない。完了は必ず `complete <parent>` の明示的な呼び出しが必要。

---

## タスクのステータス遷移

```
Pending
  │  ← report（初回かつ Pending の場合のみ InProgress に遷移。既に InProgress なら変更しない）
  ▼
InProgress
  │  ← complete → gate::check()（requires + agents）通過
  ▼
Completed
```

> **`Failed` について**: `StepStatus` enum に `Failed` variant が定義されており、`dag.executable_items()` では `Completed` と同様にスキップされる。ただし現バージョンでは `Failed` に遷移するコードパスは存在せず、将来の拡張のために予約されている。

agent サブタスク（`parent-id/agent-name`）：
- `complete parent/agent-name` → Completed に遷移（gate チェックなし）
- `sync_agents_parent()` により、いずれかのエージェントが非 Pending になると親が InProgress に遷移
- 全エージェント Completed でも、親の Completed 遷移には `complete <parent>` が必要

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

validate(&config):
  ├─ 各ワークフロー: tasks が空でないか
  ├─ 各ワークフロー: タスク ID の重複がないか（重複 ID を全件列挙してエラー）
  ├─ 各タスク: id が ^[a-z][a-z0-9_-]*$ にマッチするか（/ や大文字を禁止）
  ├─ 各タスク: (prompt/skills) と agents を同時に持っていないか
  ├─ 各タスク: 手動タスク（prompt/skills/agents すべて空）なら task があるか
  ├─ 各タスク: requires に未定義タスクが含まれていないか
  → エラーを全件収集して ValidationError として返す

【JSON Schema の生成と整合性】
  workflow.schema.json は schemars クレートで Rust 型から自動生成する。IDE サポート専用。
  ランタイムでのスキーマ検証は行わない。

  スキーマ再生成:
    workflow-runner dump-schema > .workflow/workflow.schema.json

  schema_file_matches_generated テストが cargo test で整合性を常時検証する。
  Rust 型を変更したら dump-schema でスキーマを更新してコミットする。
```

---

## テンプレート解決

`executor::resolve_template(s, config)` が `{{vars.<key>}}` を config.vars の値で置換する。
未定義キーはそのまま残す。`Task.prompt` のみが対象（`agents` の参照名は展開しない）。

```yaml
vars:
  test: make test

tasks:
  - id: impl
    prompt: "{{vars.test}} を実行してください"   # → "make test を実行してください" に展開
  - id: check
    prompt: "{{vars.lint}} を実行してください"   # → 未定義のため "{{vars.lint}} ..." のまま
```

---

## コマンド別 実行フロー詳細

### `start <workflow>`

```
cmd_start(cwd, workflow_name)
  ├─ config::loader::load_config(cwd)
  │    ├─ YAML をパース → Config
  │    └─ validate(&config)
  │
  ├─ config.workflows.get(workflow_name)
  │
  ├─ WorkflowState::new(workflow_name, wf)
  │    ├─ workflow_id = Uuid::new_v4()
  │    ├─ started_at = Utc::now()
  │    └─ tasks: 全タスク + agents の各エントリを Pending で初期化
  │         例: quality-check, quality-check/run-test, quality-check/run-lint
  │
  ├─ store::save_state(cwd, &state)
  │
  ├─ executor::build_next(wf, &state, &config)
  │    ├─ dag::executable_items(wf, state)
  │    │    └─ requires が空 かつ Pending なタスクを返す
  │    └─ TaskOutput を構築（prompt のテンプレート解決含む）
  │
  └─ JSON 出力: { workflow_id, status: "started", tasks: [...] }
```

### `next`

```
cmd_next(cwd, workflow_id?)
  ├─ load_config() + resolve_state()
  │
  ├─ get_workflow_status() == "awaiting_approval"?
  │    └─ YES → set_workflow_status("active")  # 承認として機能
  │
  └─ executor::build_next() → JSON 出力
```

### `complete <task_id>`

```
cmd_complete(cwd, task_id, workflow_id?)
  ├─ load_config() + resolve_state()
  │
  ├─ gate::check(wf, &state, task_id)
  │    ├─ サブタスク（"parent/agent"）→ 常に通過
  │    ├─ 通常タスク → requires が全て Completed か確認
  │    └─ agents タスク → requires + 全 agents が Completed か確認
  │
  ├─ gate NG → { allowed: false, reason: ... }
  │
  ├─ task.status = Completed
  ├─ parent_of(task_id) → Some → sync_agents_parent()
  │    └─ いずれかのエージェントが非 Pending → 親を InProgress に（Completed にはしない）
  ├─ save_state()
  │
  ├─ 親タスク かつ task.approval == true ?
  │    └─ YES → set_workflow_status("awaiting_approval")
  │           → { allowed: true, next: { status: "awaiting_approval", tasks: [] } }
  │
  ├─ executor::build_next() → next タスクを取得
  │    └─ is_workflow_complete() = true → clear_state_by_id()
  │
  └─ { allowed: true, next: WorkflowOutput }
```

### `reject <task_id> [--reason]`

```
cmd_reject(cwd, task_id, reason?, workflow_id?)
  ├─ load_config() + resolve_state()
  ├─ get_workflow_status() != "awaiting_approval" → エラー
  │
  ├─ task.status = InProgress、completed_at = None
  ├─ reason あり → action_reports に { action_type: "reject", stdout: reason } を追加
  ├─ set_workflow_status("active")
  ├─ save_state()
  │
  └─ { task_id, reason, task: TaskOutput }  # 再 dispatch 用
```

### `hook <event_type>`（stdin: hook JSON）

```
cmd_hook(cwd, event_type)
  ├─ stdin から cwd を自動抽出
  │
  ├─ "pre-edit"
  │    ├─ load_config() + load_state()
  │    ├─ InProgress なタスクの outputs と照合
  │    │    └─ 非マッチ → {"decision":"ask"}
  │    └─ deny.files と照合
  │         └─ マッチ → {"decision":"block"}
  │
  ├─ "pre-bash"
  │    └─ InProgress なタスクの deny.commands と照合
  │         └─ マッチ → {"decision":"block"}
  │
  └─ "post-edit"
       └─ config.yml の編集を検知 → バリデーション → 失敗時に警告出力
```

---

## アダプター / プロバイダー設計

| 層 | 責務 | 具体例 |
|----|------|--------|
| `providers/claude_code/` | Claude Code 固有の hook JSON を型安全な構造体にパース | `PostBashEvent`, `PreEditEvent` |
| `adapters/hooks/` | パース済みイベントを受け取り、engine 層を呼んで応答を返す | `handle_pre_edit()` |
| `infra/settings_writer` | settings.json の生成・更新 | `merge_settings_json()` |

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
| `glob` | 0.3 | outputs のパターンマッチ |
| `regex` | 1 | `/pattern/` 形式の正規表現マッチ |
| `comfy-table` | 7 | `status --format table` のターミナルテーブル描画 |
| `tempfile` | 3 | テスト用一時ディレクトリ（dev-dependency） |

---

## エラーハンドリング方針

| コンテキスト | 方針 |
|------------|------|
| 通常コマンド（start / next / complete 等） | `anyhow::Result` でエラーを伝播。`main()` が `ErrorOutput` JSON を stderr に出力し exit code 1 で終了 |
| フックハンドラ（`cmd_hook`） | エラーでプロセスをクラッシュさせてはならない。JSON パース失敗・設定不在は `None` を返す（ワークフロー外の操作を干渉しない） |
| gate チェック失敗（requires / agents 未充足） | `CompleteOutput { allowed: false, reason }` を返して処理を継続（プロセスは終了しない） |
| reject（awaiting_approval 以外） | エラーとして exit code 1 で終了 |
