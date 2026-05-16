# v5 実装計画

v4 までの実装（SQLite なし・シェルスクリプト Hooks・単一ワークフロー）から、
複数ワークフロー同時進行・ステップ単位のファイル制約・providers 分離・Claude Code Channels 対応への移行計画。

---

## 実装状況

### v1〜v4（完了）

- [x] Rust コア（config / engine / protocol）
- [x] Claude Code アダプター（3種のフック処理）
- [x] CLI（10 コマンド）
- [x] `gate: true` による決定論的なゲート制御
- [x] `requires` による DAG 依存解決
- [x] `{{commands.*}}` テンプレート変数解決
- [x] 並列ステップの状態管理（`parent_id/sub_id` キー）
- [x] `standalone` アダプター（`run_command` / `call_anthropic_api`）
- [x] `exec-step` CLI サブコマンド
- [x] `validate --format text` / `status --format table`
- [x] バイナリ配布（`install.sh` + GitHub Actions）
- [x] FCIS 準拠（Pure core / Shell 分離）
- [x] 51 ユニットテスト（全パス）

### v5

- [x] **Phase 1**: SQLite 導入 + `providers/` 層追加 + `--workflow-id` 対応
- [ ] **Phase 2**: `imports:` / `pre_commands` / `post_commands` / `allow_files` / `deny` / `guards` + `Action::Run` 廃止
- [ ] **Phase 3**: `init`/`update` コマンド + シェルスクリプト Hook 廃止 + `pre-edit`/`pre-bash` hook 追加
- [ ] **Phase 4**: Claude Code Channels 対応（`standalone` アダプター刷新・`reqwest` 廃止）

---

## 変更の全体像

| カテゴリ | v4 (現状) | v5 (目標) |
|---------|-----------|-----------|
| 状態管理 | `.workflow/state.json`（単一ワークフロー） | `.workflow/workflow.db`（SQLite・複数並行） |
| 作業記録 | `.workflow/checklist.md` | SQLite（`action_reports` テーブル） |
| ワークフロー識別 | 暗黙（1つのみ） | `--workflow-id <session_id>` |
| 設定ファイル | 単一 `config.yml` | `imports:` で複数ファイルに分割可 |
| コマンド定義 | `config.yml` 内の `commands:` | 別ファイルに切り出して `imports:` |
| ステップ制約 | なし | `allow_files` / `deny` / `guards` |
| シェルコマンド宣言 | `actions: [{type: run}]` | `pre_commands` / `post_commands` |
| Hook 方式 | シェルスクリプト経由 | `workflow-runner hook <event>` 直接呼び出し |
| Hook 設定 | 手動で `settings.json` を管理 | `workflow-runner init/update` で自動生成 |
| AI 依存の隔離 | `adapters/` に混在 | `providers/` 層に分離 |
| Standalone AI 実行 | Anthropic Messages API 直接呼び出し | Claude Code Channels |

---

## Phase 1: SQLite 導入 + adapters/providers 基盤

**目標**: 動作を変えずにストレージを SQLite に置換し、`providers/` 層を追加する。

### 変更ファイル一覧

| ファイル | 変更内容 |
|---------|---------|
| `Cargo.toml` | `rusqlite = { version = "0.31", features = ["bundled"] }` 追加 |
| `src/engine/store.rs` | `load_state` / `save_state` / `clear_state` の内部を SQLite に書き換え。`load_state_by_id(cwd, session_id)` / `clear_state_by_id` を追加 |
| `src/main.rs` | `--workflow-id Option<String>` グローバルフラグ追加。各 cmd に `workflow_id` を伝搬。`append_checklist` 関数を削除。`cmd_hook` で stdin JSON から `cwd` を自動抽出するコードを追加 |
| `src/adapters/claude_code/hook_handler.rs` | `handle_post_bash` から `checklist.md` 書き込みを削除。`providers::claude_code::hook_parser` の型を使って JSON パースを分離 |
| `src/providers/mod.rs` | **新規** |
| `src/providers/claude_code/mod.rs` | **新規** |
| `src/providers/claude_code/hook_parser.rs` | **新規**。各フックイベントの型安全な構造体を定義 |
| `.gitignore` | `.workflow/workflow.db` を追加。`.workflow/state.json` / `.workflow/checklist.md` のエントリを削除 |

### SQLite スキーマ

```sql
-- .workflow/workflow.db

CREATE TABLE IF NOT EXISTS workflow_runs (
    workflow_id   TEXT PRIMARY KEY,      -- UUIDv4（start 時に生成、session_id の後継）
    cwd           TEXT NOT NULL,         -- プロジェクトルート絶対パス
    workflow      TEXT NOT NULL,         -- ワークフロー slug
    status        TEXT NOT NULL DEFAULT 'active',  -- active | completed
    started_at    TEXT NOT NULL,         -- RFC3339 UTC
    completed_at  TEXT                   -- RFC3339 UTC（完了時のみ）
);

CREATE INDEX IF NOT EXISTS idx_runs_cwd_status
    ON workflow_runs(cwd, status);

CREATE TABLE IF NOT EXISTS step_states (
    workflow_id    TEXT NOT NULL REFERENCES workflow_runs(workflow_id) ON DELETE CASCADE,
    step_id        TEXT NOT NULL,        -- "step-id" または "parent/sub"
    status         TEXT NOT NULL DEFAULT 'pending',
    gate_recorded  INTEGER NOT NULL DEFAULT 0,
    started_at     TEXT,
    completed_at   TEXT,
    PRIMARY KEY (workflow_id, step_id)
);

CREATE TABLE IF NOT EXISTS action_reports (
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

### `--workflow-id` の動作仕様

- `start` の JSON 出力に `workflow_id` フィールドを追加（`session_id` フィールドは `workflow_id` に改名）
- `--workflow-id` 省略時: `cwd` で `status='active'` の行が 1 件なら自動選択
- 複数 active が存在する場合: エラーメッセージで `--workflow-id` の明示を促す

### `providers/claude_code/hook_parser.rs` の設計

```rust
// Claude Code の各フックイベントを型安全な構造体にパース

#[derive(Debug, Deserialize)]
pub struct PostBashEvent {
    pub cwd: Option<String>,
    pub tool_input: BashInput,
    pub tool_response: BashResponse,
}

#[derive(Debug, Deserialize)]
pub struct PreTaskUpdateEvent {
    pub cwd: Option<String>,
    pub tool_input: TaskUpdateInput,
}

#[derive(Debug, Deserialize)]
pub struct PostEditEvent {
    pub cwd: Option<String>,
    pub tool_input: EditInput,  // file_path フィールドを持つ
}

pub type PreEditEvent = PostEditEvent;

#[derive(Debug, Deserialize)]
pub struct PreBashEvent {
    pub cwd: Option<String>,
    pub tool_input: BashInput,
}
```

### 完了基準

- `cargo test` が全て通過する
- `workflow-runner start bug-fix` → `workflow-runner --workflow-id <id> next` が SQLite DB で動作する
- `checklist.md` が生成されない
- `.workflow/state.json` が生成されない

---

## Phase 2: ワークフロー定義の拡張

**目標**: `imports`、`pre_commands`/`post_commands`、`allow_files`、`deny`、`guards` を追加し、`Action::Run` と `checklist_key` を廃止する。

### 変更ファイル一覧

| ファイル | 変更内容 |
|---------|---------|
| `src/config/types.rs` | `Step` に新フィールド追加。`Action::Run` バリアント削除。`checklist_key` フィールド削除。`Config` に `imports: Vec<String>` 追加 |
| `src/config/loader.rs` | `imports:` の再帰的解決（循環参照検出付き）。子ワークフローのステップをインライン展開。`Action::Run` が残っている場合はエラー |
| `src/engine/gate.rs` | `post_commands` を gate 判定の根拠に使うよう更新。`guards` チェックを追加 |
| `src/engine/executor.rs` | `pre_commands` / `post_commands` を `WorkflowOutput` に含める。`checklist_key` 参照を削除 |
| `src/protocol/output.rs` | `ResolvedAction::Manual` から `checklist_key` を削除 |
| `Cargo.toml` | `glob = "0.3"` / `regex = "1"` 追加 |
| `.workflow/config.yml` | サンプルを新スキーマに更新（`type: run` → `post_commands`、`checklist_key` 削除、`imports:` 例を追加） |
| `.workflow/workflow.schema.json` | 新フィールドに合わせて更新 |

### 新 Step 型

```yaml
# 新しい config.yml 記法

# imports でファイル分割
imports:
  - commands/default.yml
  - workflows/bug-fix.yml
  - workflows/feature.yml

# .workflow/commands/default.yml
commands:
  test: make test
  lint: make lint

# .workflow/workflows/bug-fix.yml
workflows:
  bug-fix:
    name: バグ修正フロー
    steps:
      - id: design                        # checklist_key は廃止。id が一意識別子
        name: 設計確認
        allow_files:                      # InProgress 中のみ有効（空なら制限なし）
          - "docs/**"
          - "/.*\.md$/"                   # / で囲まれた場合は正規表現
        post_commands:                    # ステップ完了前にゲートとして実行
          - make docs-check

      - id: implement
        name: 実装
        allow_files:
          - "src/**"
          - "tests/**"
        deny:
          files:
            - "docs/specs/**"            # 実装中は仕様書ディレクトリを変更禁止
          commands:
            - "git push"                 # Bash ツールでのコマンドを制限
        guards:
          - step: design                 # design ステップが完了していること
            required_files:
              - "docs/**/*.md"           # design の allow_files に該当するファイルが存在すること
        pre_commands:                    # ステップ開始時に自動実行
          - cargo check
        post_commands:                   # ステップ完了前にゲートとして実行
          - cargo test
        requires: [design]
        actions:
          - type: agent
            prompt: "設計に従って実装してください"
```

### パターンマッチの仕様

- `/pattern/` 形式（スラッシュ区切り）: Rust 正規表現として評価
- それ以外: `glob` クレートの glob パターンとして評価
- `allow_files` / `deny.files`: ファイルパスに対して評価（プロジェクトルートからの相対パス）
- `deny.commands`: 実行コマンド文字列に対して評価（部分一致）

### 子ワークフローのステップ埋め込み

```yaml
# 親ワークフローの steps 内でインポート
steps:
  - id: prepare
    name: 準備

  # 子ファイルのステップをここにフラット展開
  - import: workflows/code-review-steps.yml

  - id: ship
    name: リリース
    requires: [review-complete]          # 子ステップの id で参照可能
```

### Action::Run の廃止移行

- `config/loader.rs` の `load_config` で `actions: [{type: run}]` を検出した場合はロードエラー
- エラーメッセージ: `"step '<id>': Action::Run は廃止されました。pre_commands / post_commands を使用してください"`
- `gate: true` の置き換え: `post_commands` に移動（全 post_commands が成功 = gate 通過）

### 完了基準

- `imports:` を使った分割設定が動作する
- `pre_commands` / `post_commands` が executor 出力に含まれる
- `Action::Run` を使った設定でわかりやすいエラーが出る
- `cargo test` が全て通過する

---

## Phase 3: Hook 改善と `init`/`update` コマンド

**目標**: シェルスクリプト Hooks を廃止し、`workflow-runner` が stdin から `cwd` を読む。`init`/`update` で settings.json を自動生成。

### 変更ファイル一覧

| ファイル | 変更内容 |
|---------|---------|
| `src/main.rs` | `Commands::Init` / `Commands::Update` サブコマンド追加。`cmd_hook` の `cwd` 解決を stdin JSON 読み取りに一本化（`--cwd` フラグは後方互換のため残す） |
| `src/infra/mod.rs` | **新規** |
| `src/infra/settings_writer.rs` | **新規**。`write_settings_json(cwd)` と `merge_settings_json(cwd)` を実装 |
| `src/adapters/claude_code/hook_handler.rs` | `handle_pre_edit()` 追加（allow/deny ファイル制約）。`handle_pre_bash()` 追加（deny コマンド制約）。全ハンドラが `providers::claude_code::hook_parser` の型でパースするよう変更 |
| `.claude/hooks/post-bash-capture-test.sh` | **削除** |
| `.claude/hooks/pre-taskupdate-gate.sh` | **削除** |
| `.claude/hooks/post-edit-validate-config.sh` | **削除** |
| `.claude/hooks/post-edit-rust-checks.sh` | **維持**（開発補助フックのため） |
| `.claude/settings.json` | `workflow-runner init` で再生成（シェルスクリプト参照を廃止） |

### `init` コマンドの動作

```bash
workflow-runner init
# → .workflow/ ディレクトリを作成
# → .claude/settings.json を生成
# → workflow-runner が PATH に存在するか確認し、なければ警告
```

生成される `.claude/settings.json`:

```json
{
  "hooks": {
    "PostToolUse": [
      {
        "matcher": "Edit",
        "hooks": [
          {"type": "command", "command": "workflow-runner hook post-edit"},
          {"type": "command", "command": ".claude/hooks/post-edit-rust-checks.sh"}
        ]
      },
      {
        "matcher": "Write",
        "hooks": [
          {"type": "command", "command": "workflow-runner hook post-edit"},
          {"type": "command", "command": ".claude/hooks/post-edit-rust-checks.sh"}
        ]
      }
    ],
    "PreToolUse": [
      {
        "matcher": "TaskUpdate",
        "hooks": [{"type": "command", "command": "workflow-runner hook pre-taskupdate"}]
      },
      {
        "matcher": "Edit",
        "hooks": [{"type": "command", "command": "workflow-runner hook pre-edit"}]
      },
      {
        "matcher": "Write",
        "hooks": [{"type": "command", "command": "workflow-runner hook pre-edit"}]
      },
      {
        "matcher": "Bash",
        "hooks": [{"type": "command", "command": "workflow-runner hook pre-bash"}]
      }
    ]
  }
}
```

### `update` コマンドの動作

既存の `.claude/settings.json` を読み込み、`workflow-runner` 関連の hook エントリを差分更新する。
`post-edit-rust-checks.sh` など他のフック設定は保持する。

### `cwd` の stdin 抽出

```rust
// cmd_hook 内でシェルスクリプトと同等の cwd 抽出を Rust で実装
fn extract_cwd_from_stdin(stdin_str: &str, fallback: &Path) -> PathBuf {
    let v: serde_json::Value = serde_json::from_str(stdin_str).unwrap_or_default();
    v["cwd"].as_str()
        .map(PathBuf::from)
        .filter(|p| p.exists())
        .unwrap_or_else(|| fallback.to_path_buf())
}
```

hook コマンドは `--cwd` なしで `"command": "workflow-runner hook pre-edit"` として登録可能になる。

### 新フックイベントの動作

| フック | 判定内容 | 返却値 |
|--------|---------|--------|
| `pre-edit` | InProgress ステップの `allow_files` / `deny.files` をチェック | `{"decision":"block","reason":"..."}` または `{"decision":"ask","reason":"..."}` |
| `pre-bash` | InProgress ステップの `deny.commands` をチェック | `{"decision":"block","reason":"..."}` |
| `pre-taskupdate` | InProgress ステップの gate 未実行チェック（既存） | `{"decision":"block","reason":"..."}` |
| `post-edit` | `config.yml` 編集後のスキーマ検証（既存） | 警告文字列または空 |

### 完了基準

- `workflow-runner init` で `settings.json` が正しく生成される
- `workflow-runner update` が既存 `settings.json` を壊さずにマージできる
- シェルスクリプトなしで `workflow-runner hook pre-edit` が stdin JSON から動作する
- `allow_files` 違反で `{"decision":"block"}` が返る
- `cargo test` が全て通過する

---

## Phase 4: Standalone モード → Claude Code Channels

**目標**: Anthropic API 直接呼び出しを廃止し、Claude Code Channels に移行する。

**前提**: https://code.claude.com/docs/en/channels の仕様を実装開始前に精読して設計を確定する。

### 変更ファイル一覧

| ファイル | 変更内容 |
|---------|---------|
| `src/adapters/standalone/runner.rs` | `call_anthropic_api` 削除。`run_command` は維持 |
| `src/adapters/standalone/channels.rs` | **新規**。`providers::channels` を介して Claude Code Channels API を呼び出す |
| `src/providers/channels/mod.rs` | **新規**。Channels API クライアント実装 |
| `src/main.rs` | `cmd_exec_step` の `Action::Agent` ブランチを channels 実装に差し替え |
| `Cargo.toml` | `reqwest` 削除（Channels が HTTP 以外の IPC の場合）または feature-gated 化 |
| `ARCHITECTURE.md` | standalone → Channels 対応の記述を更新 |
| `README.md` | `ANTHROPIC_API_KEY` 依存の記述を削除 |

### Channels 移行の設計方針

- standalone アダプターが Claude Code のセッション外からワークフローを制御する外部コントローラーになる
- `ANTHROPIC_API_KEY` 不要
- `exec-step <step-id>` コマンドが Channels API 経由でエージェントアクションを実行する
- Channels API がどのプロトコル（HTTP/IPC/stdio）を使うかによって `providers/channels/` の実装が変わる

### 完了基準

- `ANTHROPIC_API_KEY` なしで `exec-step` が動作する
- Channels 経由でエージェントアクションが実行される
- `reqwest` 依存が除去される（または feature-gated になる）
- `cargo test` が全て通過する

---

## 依存関係グラフ

```
Phase 1（SQLite + providers）
    ↓ 必須
Phase 2（ワークフロー定義拡張）
    ↓ 必須
Phase 3（Hook 改善）
    ↓ 独立可（Phase 1 完了後に着手可能）
Phase 4（Claude Code Channels）
```

Phase 4 は Phase 1 完了後に他フェーズと並行して設計着手できるが、Phase 3 の hook 設計が固まってから実装するのが望ましい。

---

## 廃止されるファイル・フィールド

| 廃止対象 | 理由 | 代替 |
|---------|------|------|
| `.workflow/state.json` | SQLite に移行 | `.workflow/workflow.db` |
| `.workflow/checklist.md` | SQLite に移行 | `action_reports` テーブル |
| `Step.checklist_key` | `id` で代替可能 | `Step.id` |
| `Action::Run { gate }` | `pre_commands` / `post_commands` に分離 | `Step.post_commands` |
| `.claude/hooks/post-bash-capture-test.sh` | 直接コマンドに移行 | `workflow-runner hook post-bash`（廃止） |
| `.claude/hooks/pre-taskupdate-gate.sh` | 直接コマンドに移行 | `workflow-runner hook pre-taskupdate` |
| `.claude/hooks/post-edit-validate-config.sh` | 直接コマンドに移行 | `workflow-runner hook post-edit` |
| `call_anthropic_api()` | Claude Code Channels に移行 | `providers::channels` |
