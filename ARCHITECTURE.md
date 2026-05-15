# aidd-workflow アーキテクチャ

## 概要

`workflow-runner` は AI ツール上で実行されるワークフローを **決定論的に制御する** Rust 製エンジン。

### 解決する問題

| 問題 | 解決方法 |
|------|---------|
| Claude がテストを飛ばして完了と報告する | `gate: true` アクション + ゲートチェック（Rust）|
| ステップの内容を毎回 Claude が解釈する（非決定論） | `actions` フィールドで実行内容を宣言的に記述 |
| セッションをまたいで作業が中断する | `state.json` でステップ状態を永続化 |
| AI ツール固有の API に依存する | アダプター層で AI ツールの差異を吸収 |
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
│  ┌──────────────┐ ┌───────────────┐ ┌──────────────────┐ │
│  │  config 層   │ │  engine 層    │ │  adapters 層     │ │
│  │  YAML パース │ │  DAG 評価     │ │  claude-code     │ │
│  │  型安全な定義│ │  状態管理     │ │  (将来) cursor   │ │
│  │  バリデーション│ │  ゲートチェック│ │  (将来) standalone│ │
│  └──────────────┘ └───────────────┘ └──────────────────┘ │
└─────────────┬────────────────────────────────────────────┘
              │  ファイル I/O
┌─────────────▼────────────────────────────────────────────┐
│                   状態層（ファイル）                        │
│  .workflow/config.yml      .workflow/state.json           │
│  .workflow/checklist.md    .workflow/workflow.schema.json │
└──────────────────────────────────────────────────────────┘
```

---

## ディレクトリ構成

```
aidd-workflow/
├── src/
│   ├── main.rs                          CLI エントリポイント（clap）
│   ├── config/
│   │   ├── types.rs                     Config / Workflow / Step / SubStep / Action 型定義（Pure）
│   │   └── loader.rs                    YAML ロード + バリデーション（Shell）
│   ├── engine/
│   │   ├── state.rs                     WorkflowState 型定義・純粋メソッド（Pure）
│   │   ├── store.rs                     state.json 読み書き（Shell）
│   │   ├── dag.rs                       requires 依存グラフ評価・サブステップ DAG（Pure）
│   │   ├── gate.rs                      gate 条件チェック（Pure）
│   │   └── executor.rs                  next_actions の構築・parallel フラグ付与（Pure）
│   ├── adapters/
│   │   └── claude_code/
│   │       └── hook_handler.rs          Claude Code フック処理（Shell）
│   └── protocol/
│       ├── input.rs                     report コマンドの stdin 型（Pure）
│       └── output.rs                    JSON 出力型定義（Pure）
├── .workflow/
│   ├── config.yml                       ワークフロー定義（ユーザーが編集）
│   ├── workflow.schema.json             JSON Schema（拡張済み）
│   ├── state.json                       実行状態（自動生成、gitignore）
│   └── checklist.md                     作業記録（自動生成、gitignore）
└── .claude/
    ├── hooks/
    │   ├── post-bash-capture-test.sh    テスト実行を検出して state.json を更新
    │   ├── pre-taskupdate-gate.sh       TaskUpdate 前に gate 未実行をブロック
    │   ├── post-edit-validate-config.sh config.yml 編集後にスキーマ検証
    │   └── post-edit-rust-checks.sh     .rs 編集後に fmt / lint / test を自動実行
    └── skills/workflow-orchestrator/    workflow-runner を呼ぶ薄いブリッジ
```

---

## config.yml スキーマ

### 基本構造

```yaml
commands:                          # コマンドエイリアス（{{commands.test}} で参照可）
  test: make test
  lint: make lint

workflows:
  <slug>:
    name: ワークフロー名
    steps:
      - id: <step-id>
        name: ステップ名
        description: 説明
        requires: [<step-id>, ...]   # 依存ステップ（DAG の辺）
        checklist_key: <key>         # 手動記録を促すキー
        actions: [...]               # または parallel: [...] （両立不可）
```

### アクション型

| `type` | フィールド | 説明 |
|--------|-----------|------|
| `run` | `command`, `gate: bool` | シェルコマンド実行。`gate: true` で実行記録が complete の前提条件になる |
| `agent` | `prompt`, `background: bool` | サブエージェント起動。`background: true` で並列実行可 |
| `skill` | `skill`, `args: []` | スキル呼び出し |
| `workflow` | `workflow`, `inputs: {}` | 別ワークフローをネスト実行 |

### ステップの3形態

```yaml
# 1. 自動ステップ（actions あり）
- id: test
  actions:
    - type: run
      command: "{{commands.test}}"
      gate: true

# 2. 並列ステップ（parallel ブロック）
#    各サブステップは requires で並列ブロック内の依存を表現できる
- id: quality-check
  parallel:
    - id: run-test
      actions: [{ type: run, command: make test, gate: true }]
    - id: run-lint
      actions: [{ type: run, command: make lint }]
      requires: [run-test]          # サブステップ間の依存（省略可）

# 3. 手動ステップ（actions も parallel もなし）
- id: design
  description: 実装方針を整理して記録する
  checklist_key: design
```

---

## CLI プロトコル

### コマンド一覧

```
workflow-runner [--adapter <name>] [--cwd <path>] <command>

start <workflow>   ワークフロー開始 → 最初の actions を JSON で返す（status: "started"）
next               次の actions を JSON で返す
report             アクション実行結果を記録（stdin: JSON）
complete <step-id> ステップ完了（ゲートチェック付き）→ 次の actions を返す
resume             中断ワークフローの再開情報を返す
status             現在の実行状態を JSON で返す（並列サブステップも含む）
validate           config.yml を検証する
list               ワークフロー一覧を返す
hook <event-type>  Claude Code フックイベントを処理（stdin: hook JSON）
```

### スキルとの通信フロー

```
SKILL.md（Claude Code）                workflow-runner
        │                                      │
        │── start bug-fix ────────────────────▶│ state.json 作成
        │◀── { status: "started", actions: [...] } ──│
        │                                      │
        │── [actions を実行] ──────────────────│
        │                                      │
        │── report ───────────────────────────▶│ state.json 更新
        │◀── { ok: true } ────────────────────│ gate_recorded = true（gate アクションの場合）
        │                                      │
        │── complete test ────────────────────▶│ gate チェック
        │◀── { allowed: true, next: {...} } ───│ state.json 更新
        │                                      │
        │── [繰り返し] ───────────────────────│
        │                                      │
        │── complete final-step ─────────────▶│ 全完了 → state.json 削除
        │◀── { allowed: true,                  │
        │      next: { status: "completed" } }─│
```

### JSON 出力例

**start の出力**（`status: "started"` でワークフロー開始を明示）

```json
{
  "session_id": "4fd261ba-...",
  "workflow": "bug-fix",
  "status": "started",
  "actions": [
    {
      "step_id": "reproduce",
      "action_index": 0,
      "step_name": "再現確認",
      "parallel": false,
      "type": "manual",
      "description": "バグを手元で再現し、再現手順を記録する",
      "checklist_key": "reproduce"
    }
  ]
}
```

**並列ブロックの next 出力**（`parallel: true` のアイテムは同時実行可）

```json
{
  "session_id": "4fd261ba-...",
  "workflow": "release",
  "status": "in_progress",
  "actions": [
    {
      "step_id": "quality-check/run-test",
      "action_index": 0,
      "step_name": "テスト実行",
      "parallel": true,
      "type": "run",
      "command": "make test",
      "gate": true
    },
    {
      "step_id": "quality-check/run-lint",
      "action_index": 0,
      "step_name": "Lint",
      "parallel": true,
      "type": "run",
      "command": "make lint",
      "gate": false
    }
  ]
}
```

**complete の出力（gate ブロック時）**

```json
{
  "step_id": "test",
  "allowed": false,
  "reason": "gate チェック失敗: ステップ 'test' の gate アクションが未実行です。先にコマンドを実行してください",
  "next": null
}
```

**complete の出力（通過時）**

```json
{
  "step_id": "test",
  "allowed": true,
  "reason": null,
  "next": {
    "session_id": "4fd261ba-...",
    "workflow": "bug-fix",
    "status": "completed",
    "actions": []
  }
}
```

---

## 状態管理（state.json）

```json
{
  "session_id": "uuid-v4",
  "workflow": "release",
  "started_at": "2026-05-14T10:00:00Z",
  "steps": {
    "design":    { "status": "completed", ... },
    "implement": { "status": "completed", ... },
    "quality-check": {
      "status": "in_progress",
      "gate_recorded": false,
      "action_reports": []
    },
    "quality-check/run-test": {
      "status": "completed",
      "gate_recorded": true,
      "action_reports": [
        { "action_index": 0, "action_type": "run", "exit_code": 0, "stdout": "42 passed" }
      ]
    },
    "quality-check/run-lint": { "status": "pending", ... }
  }
}
```

### 並列ステップのキー命名規則

```
通常ステップ      → "step-id"
並列サブステップ  → "parent-id/sub-id"
並列親ステップ    → "parent-id"（サブステップから sync_parallel_parent で自動導出）
```

並列親ステップの `status` は全サブステップの状態から自動導出される。

| サブステップの状態 | 親ステップの status |
|-----------------|------------------|
| 全て `pending` | `pending` |
| 1つ以上 `in_progress` または `completed` | `in_progress` |
| 全て `completed` | `completed` |

---

## ゲートメカニズム

`gate: true` を持つ `run` アクションは「実行の証明」を要求する。

```
config.yml に gate: true を宣言
        ↓
SKILL.md が run アクションを Bash で実行
        ↓（2経路で記録）
A: SKILL が workflow-runner report を呼ぶ（主経路）
B: post-bash フックが workflow-runner hook post-bash を呼ぶ（補助）
        ↓
state.json の gate_recorded = true
        ↓
workflow-runner complete <step-id>
        ↓
gate.rs が gate_recorded を確認 → allowed: true
        ↓
ステップ Completed に遷移
```

**旧設計との比較**

| 項目 | 旧（bash） | 新（Rust） |
|------|-----------|-----------|
| ゲート有効化 | `touch .workflow/GATE_ACTIVE`（手動） | `gate: true` を config で宣言（静的） |
| 記録チェック | checklist.md の文字列検索 | `state.json.gate_recorded` フラグ |
| ゲート判定 | シェルスクリプト + インライン Python | `engine/gate.rs`（型安全な Rust） |
| フック複雑度 | 〜50行のシェルスクリプト | 5行以下（workflow-runner 呼び出しのみ） |

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

### 並列ブロック内のサブステップ依存

`SubStep` も `requires` フィールドを持ち、並列ブロック内で逐次的な依存を表現できる。

```yaml
parallel:
  - id: build        # requires なし → 即時実行可
  - id: test
    requires: [build] # build 完了後に実行可
```

`executable_items` は並列ブロック内でも `requires` を評価し、満たされたサブステップのみを返す。

---

## アダプター設計

`--adapter` フラグで AI ツール固有の処理を切り替える。

### 現在の実装: `claude-code`

`hook_handler.rs` が Claude Code の 3 種類のワークフローフックイベントを処理する。

| フック | タイミング | 処理内容 |
|--------|-----------|---------|
| `post-bash` | Bash ツール実行後 | テストコマンド検出 → checklist.md 追記 + state.json 更新 |
| `pre-taskupdate` | TaskUpdate 実行前 | in_progress ステップの gate 未実行チェック → ブロック判定 |
| `post-edit` | Edit/Write 実行後 | config.yml 変更検出 → スキーマ検証警告 |

ワークフローフックはエラーで終了しない（exit 0 固定）。ワークフロー外の操作を干渉しないように設計。

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

### 将来の拡張

| アダプター | 概要 |
|-----------|------|
| `standalone` | AI ツールなし。`run` を `std::process::Command` で直接実行、`agent` を Anthropic API で実行 |
| `cursor` | Cursor の拡張機構に対応（フックのイベント形式が異なる） |
| `generic` | 設定ファイルで任意の AI ツールに対応 |

コアエンジン（config / engine / protocol）は AI ツールを知らない。アダプターはフックの入出力変換のみを担う。

---

## 実装済みクレート

| クレート | バージョン | 用途 |
|---------|-----------|------|
| `clap` | 4 | CLI パース（derive マクロ） |
| `serde` / `serde_json` / `serde_yaml` | 1 | 設定・状態の JSON/YAML シリアライズ |
| `anyhow` | 1 | エラーハンドリング |
| `uuid` | 1 | セッション ID 生成（v4） |
| `chrono` | 0.4 | タイムスタンプ（UTC/ローカル） |
| `tempfile` | 3 | テスト用一時ディレクトリ（dev-dependency） |

---

## 実装状況（フェーズ）

### Phase 1 — 完了

- [x] Rust コア（config / engine / protocol）
- [x] Claude Code アダプター（3種のフック処理）
- [x] CLI（9コマンド）
- [x] `gate: true` による決定論的なゲート制御
- [x] `requires` による DAG 依存解決
- [x] `{{commands.*}}` テンプレート変数解決
- [x] 並列ステップの状態管理（`parent_id/sub_id` キー）
- [x] フックの簡素化（bash 50行 → 5行）
- [x] SKILL.md v2（workflow-runner ブリッジ）
- [x] JSON Schema 拡張（actions / parallel / action 型）
- [x] FCIS 準拠（`engine/store.rs` を新設し、Pure Core と Shell を分離）
- [x] 全ソースファイルにユニットテスト追加
- [x] ソースコード中の全コメントを英語に統一

### Phase 2 — 完了

- [x] `SubStep.requires`：並列ブロック内のサブステップ依存（DAG サブグラフ評価）
- [x] `ActionItem.parallel: bool`：並列実行可能なアイテムの明示的なフラグ
- [x] `build_status` で並列サブステップの個別状態を表示
- [x] `FlowStatus::Started`：`start` コマンドで開始を明示する status 値
- [x] SKILL.md v2 更新：`parallel: true` アイテムの並列 dispatch 手順
- [x] `release` ワークフロー：並列 `quality-check` ブロックのサンプル追加
- [x] `.rs` 編集後の自動品質チェックフック（fmt / lint / test）
- [x] 42 ユニットテスト（全パス）

### Phase 3 — 予定

- [ ] `standalone` アダプター（`run` を直接実行、`agent` を Anthropic API 呼び出し）

### Phase 4 — 予定

- [ ] `workflow-runner status --format table` のターミナル表示
- [ ] バイナリ配布（install スクリプト）
