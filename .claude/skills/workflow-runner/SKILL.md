---
name: workflow-runner
description: >
  プロジェクトの .workflow/config.yml に定義されたワークフローを Tasks API で管理・実行するスキル。
  テストスキップ・ルール忘れ・多段タスクの中断など「Claude が守るべき手順を守らない」問題を
  workflow-runner（Rust バイナリ）+ Hooks + Tasks API の組み合わせで構造的に防ぐ。
  「バグ修正フローで進めて」「workflow bug-fix」「ワークフローを開始して」「機能開発フローで」
  「テストを飛ばさずに進めて」「複数ステップの作業をタスク管理しながら進めたい」
  「前回中断した作業を再開して」など、ワークフローに沿って作業を構造的に進めたいときは
  必ずこのスキルを使うこと。ユーザーが「ワークフロー」「フロー」「手順通りに」と言ったら迷わず使う。
---

# Workflow Runner スキル

`workflow-runner` バイナリが判断ロジックを持ち、このスキルはタスクを dispatch するだけ。
各タスクのゲートは Hooks + `workflow-runner` が自動で検証する。

---

## 前提確認

1. `.workflow/config.yml` が存在するか確認する
   - **存在しない場合**：「`.workflow/config.yml` が見つかりません。`/workflow-create` でワークフローを新規作成してください」と案内して終了

2. `workflow-runner` バイナリを以下の優先順で探す：
   1. `which workflow-runner` で PATH にあれば `workflow-runner` をそのまま使う
   2. なければ `./target/debug/workflow-runner` を確認する
   3. どちらも存在しない場合は `cargo build` を実行してビルドする

   以降のコマンドは見つかったパスで置き換えて実行する（このドキュメントでは `workflow-runner` と表記）。

3. 引数でワークフロー名が指定されていれば `start` へ。なければ以下へ

---

## 引数なしで呼ばれた場合

```bash
workflow-runner next 2>/dev/null || workflow-runner list
```

- `next` が中断中のワークフローを検出した場合 → そのまま再開フローへ（`next` の出力は `WorkflowOutput` 形式。`tasks` 配列を TaskCreate して dispatch する）
- `next` が失敗した場合（進行中ワークフローなし）→ `list` の結果をユーザーに提示してワークフロー選択

> **注意**：承認待ち（`awaiting_approval`）状態のときに `next` を呼ぶと、**自動的に承認されて次のタスクへ進む**。承認が必要かどうかをユーザーに確認してから呼ぶこと（承認フローの詳細は後述）。

---

## ワークフローの開始

### 1. ユーザーに確認する

```bash
workflow-runner list
```

選択されたワークフローの内容を表示して確認を取る。

### 2. `start` を実行して最初のタスクを取得する

```bash
workflow-runner start <workflow-name>
```

出力は以下の JSON 形式（`WorkflowOutput`）：

```json
{
  "workflow_id": "bug-fix-a1b2c3",
  "workflow": "bug-fix",
  "status": "started",
  "tasks": [
    {
      "task_id": "reproduce",
      "task": "バグを再現する",
      "prompt": "バグを手元で再現し...",
      "skills": [],
      "agents": [],
      "outputs": [],
      "deny": null,
      "approval": false
    }
  ]
}
```

`workflow_id` と `tasks` 配列を保持して以降の処理に使う。

### 3. TaskCreate で各タスクを登録する

`start` が返した `tasks` 配列の各タスクに対して **TaskCreate** を呼ぶ：

- `subject`：`task` フィールドの値（`null` の場合は `task_id` を使う）
- `description`：`prompt` があれば prompt の内容、なければ `subject` と同じ値
- `metadata`：`{ "workflow_id": "<workflow_id>", "task_id": "<task_id>" }`

返された TaskCreate の ID（Tasks API の ID）を記録し、実行・完了時に TaskUpdate でステータスを更新する。

---

## タスクの dispatch

`tasks` 配列の各タスクを `task` / `prompt` / `skills` / `agents` に従って実行する。

実行開始前に TaskUpdate でステータスを更新する：

```
TaskUpdate(id=<TaskCreate で得た ID>, status="in_progress")
```

| 条件 | 実行方法 |
|------|---------|
| `prompt` あり（`agents` なし） | Agent ツールで `prompt` を実行する |
| `skills` あり | 各スキルを Skill ツールで呼び出す |
| `prompt` と `skills` 両方あり | `prompt` を Agent で実行後、`skills` を順に呼ぶ |
| `agents` あり | 各エージェントを Agent ツールで並列起動する（後述） |
| すべて空 | `task` に従って Claude が直接作業する（手動タスク） |

---

## タスク完了後の処理

各タスクが終わったら `report` → `complete` の順に呼ぶ。

### report：実行履歴を記録する

`report` はゲートチェック前に実行履歴をステートに書き込むためのコマンド。
`session_id` は将来予約フィールドで現時点では任意の文字列でよい。
`exit_code` / `stdout` はオプション。

```bash
echo '{
  "session_id": "n/a",
  "task_id": "<task_id>",
  "action_index": 0,
  "action_type": "agent",
  "exit_code": 0
}' | workflow-runner report
```

`action_type` の値：`"agent"`（Agent ツール実行）、`"skill"`（Skill 呼び出し）、`"run"`（手動作業）

### complete：ゲートチェックしてステートを進める

```bash
workflow-runner complete <task-id>
```

完了後、対応する TaskUpdate でステータスを `completed` に更新する：

```
TaskUpdate(id=<TaskCreate で得た ID>, status="completed")
```

`complete` が `allowed: true` を返し `next.tasks` に新しいタスクが含まれる場合は、
**それぞれに対して TaskCreate を呼んで登録してから** dispatch する。

### complete レスポンスの解釈

`complete` の出力：
```json
{
  "task_id": "reproduce",
  "allowed": true,
  "reason": null,
  "next": { "workflow_id": "...", "workflow": "...", "status": "in_progress", "tasks": [...] }
}
```

| `allowed` | `next` | `next.status` | 対応 |
|-----------|--------|---------------|------|
| `false` | `null` | — | `reason` をユーザーに伝えてブロック。gate 未実行なら該当作業を実行してから `complete` を再実行 |
| `true` | あり | `in_progress` | `next.tasks` を TaskCreate して dispatch する |
| `true` | あり | `completed` | ワークフロー完了。完了サマリーを表示する |
| `true` | あり | `blocked` | 未解決の依存タスクがある（後述） |
| `true` | あり | `awaiting_approval` | 承認待ち（後述） |

### `blocked` ステータスの対応

```bash
workflow-runner status --format table
```

テーブル表示で未完了の依存タスクを特定する。未完了タスクを完了させてから、ブロックされているタスクの `complete` を再度呼ぶ。

---

## agents ブロックの実行

`tasks` に `"agents": ["run-test", "run-lint"]` が含まれる場合：

1. 各エージェントを Agent ツールで **並列起動** する（`.claude/agents/<name>.md` が定義）
2. 各エージェント完了後、`<親タスク ID>/<エージェント名>` の形式で `complete` を呼ぶ：

```bash
workflow-runner complete <parent-task-id>/<agent-name>
# 例: workflow-runner complete quality-check/run-test
```

3. 全エージェント完了後、**親タスク ID** で `complete` を呼ぶ：

```bash
workflow-runner complete <parent-task-id>
# 例: workflow-runner complete quality-check
```

> gate チェックで未完了エージェントが残っていれば `allowed: false` が返る。残りのエージェントを完了させてから `complete <parent>` を再試行する。

---

## approval タスクの完了後

`complete` レスポンスで `next.status == "awaiting_approval"` の場合：

1. タスクの `task` をユーザーに表示する
2. 「次のタスクに進む前に承認が必要です。承認しますか？」と確認を取る

### 承認された場合

```bash
workflow-runner next
```

`next` を呼ぶと承認が通り、次のタスクが `WorkflowOutput` 形式で返る。
`next.tasks` を TaskCreate で登録してから dispatch する。

### 却下された場合

理由をヒアリングし：

```bash
workflow-runner reject <task-id> --reason "<理由>"
```

レスポンスの `task` フィールドにタスク定義が返るので、それを使って同タスクを再 dispatch する。
（`task` が `null` の場合は `status` でタスク定義を確認する）
再完了後、同じ承認フローに入る（回数制限なし）。

---

## 状態確認

```bash
workflow-runner status             # JSON 形式
workflow-runner status --format table  # テーブル形式（人間向け）
```

---

## 完了サマリー

`complete` の返り値で `next.status == "completed"` になったら表示する：

```
## ワークフロー完了：{workflow.name}
```

---

## エラー対応

| 状況 | 対応 |
|------|------|
| `workflow-runner` が PATH にも `./target/debug/` にも存在しない | `cargo build` を実行してビルドする |
| gate ブロック（`allowed: false`） | `reason` を伝えて該当作業を実行してから再度 `complete` |
| `blocked` ステータス | `status --format table` で依存タスクを確認し、未完了タスクを先に完了させる |
| セッション中断 | `workflow-runner next` で再開情報を取得（承認待ち中なら自動承認されるため注意） |
| config.yml の警告 | スキーマ警告が出たら自己修正してから報告 |
| agents で一部が未完了 | 残りのエージェントを完了させてから `complete <parent>` |
