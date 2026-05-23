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
2. `target/debug/workflow-runner` が存在するか確認する
   - **存在しない場合**：Bash で `cargo build` を実行してバイナリをビルドする
3. 引数でワークフロー名が指定されていれば `start` へ。なければ以下へ

---

## 引数なしで呼ばれた場合

```bash
./target/debug/workflow-runner next 2>/dev/null || ./target/debug/workflow-runner list
```

- `next` が state を読んで中断ワークフローを返した場合 → 再開フローへ
- `next` が失敗した場合 → `list` の結果をユーザーに提示してワークフロー選択

---

## ワークフローの開始

### 1. ユーザーに確認する

```bash
./target/debug/workflow-runner list
```

選択されたワークフローの内容を表示して確認を取る。

### 2. `start` を実行して最初のタスクを取得する

```bash
./target/debug/workflow-runner start <workflow-name>
```

出力 JSON の `workflow_id` と `tasks` 配列を保持する。

### 3. TaskCreate で各タスクを登録する

`start` が返した `tasks` 配列の各タスクに対して **TaskCreate** を呼ぶ：

- `subject`：タスクの `task` フィールド（簡潔なタスク名）
- `description`：`prompt` があれば prompt の内容、なければ `task` の値
- `metadata`：`{ "workflow_id": "<workflow_id>", "task_id": "<task_id>" }`

返された TaskCreate の ID を記録し、実行・完了時に TaskUpdate でステータスを更新する。

---

## タスクの dispatch

`tasks` 配列の各 `TaskOutput` を `task` / `prompt` / `skills` / `agents` に従って実行する。

実行開始前に TaskUpdate でステータスを `in_progress` に更新する。

| 条件 | 実行方法 |
|------|---------|
| `prompt` あり（`agents` なし） | Agent ツールで `prompt` を実行する |
| `skills` あり | 各スキルを Skill ツールで呼び出す |
| `prompt` と `skills` 両方あり | `prompt` を Agent で実行後、`skills` を順に呼ぶ |
| `agents` あり | 各エージェントを Agent ツールで並列起動する（後述） |
| すべて空 | `task` に従って Claude が直接作業する（手動タスク） |

---

## タスク完了後の処理

各タスクが終わったら `report` → `complete` を呼ぶ：

```bash
echo '{"session_id":"<id>","task_id":"<task>","action_index":0,"action_type":"agent","exit_code":0,"stdout":""}' \
  | ./target/debug/workflow-runner report

./target/debug/workflow-runner complete <task-id>
```

完了後、対応する TaskUpdate でステータスを `completed` に更新する。

`complete` レスポンスの `next.tasks` に新しいタスクが含まれる場合は、
**それぞれに対して TaskCreate を呼んで登録してから** dispatch する。

### レスポンスの解釈

| `allowed` | `next.status` | 対応 |
|-----------|---------------|------|
| `false` | — | `reason` をユーザーに伝えてブロック。gate 未実行なら該当作業を実行 |
| `true` | `in_progress` | `next.tasks` を TaskCreate して dispatch する |
| `true` | `completed` | ワークフロー完了。完了サマリーを表示する |
| `true` | `blocked` | 未解決の依存がある。`status` で確認する |
| `true` | `awaiting_approval` | 承認待ち（後述） |

---

## agents ブロックの実行

`tasks` に `"agents": ["run-test", "run-lint"]` が含まれる場合：

1. 各エージェントを Agent ツールで **並列起動** する（`.claude/agents/<name>.md` が定義）
2. 各エージェント完了後、サブエージェント ID で `complete` を呼ぶ：

```bash
./target/debug/workflow-runner complete <parent-task-id>/<agent-name>
# 例: ./target/debug/workflow-runner complete quality-check/run-test
```

3. 全エージェント完了後、**親タスク ID** で `complete` を呼ぶ：

```bash
./target/debug/workflow-runner complete <parent-task-id>
# 例: ./target/debug/workflow-runner complete quality-check
```

> gate チェックで未完了エージェントが残っていれば `allowed: false` が返る。

---

## approval タスクの完了後

`complete` レスポンスで `next.status == "awaiting_approval"` の場合：

1. タスクの `task` をユーザーに表示する
2. 「次のタスクに進む前に承認が必要です。承認しますか？」と確認を取る

### 承認された場合

```bash
./target/debug/workflow-runner next
```

承認が解除され、次タスクを dispatch する（TaskCreate で登録してから実行）。

### 却下された場合

理由をヒアリングし：

```bash
./target/debug/workflow-runner reject <task-id> --reason "<理由>"
```

レスポンスの `task` を使って同タスクを再 dispatch する。
再完了後、同じ承認フローに入る（回数制限なし）。

---

## 状態確認

```bash
./target/debug/workflow-runner status
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
| `workflow-runner` が存在しない | `cargo build` を実行してビルドする |
| gate ブロック | `reason` を伝えて該当作業を実行してから再度 `complete` |
| セッション中断 | `workflow-runner next` で再開情報を取得 |
| config.yml の警告 | スキーマ警告が出たら自己修正してから報告 |
| agents で一部が未完了 | 残りのエージェントを完了させてから `complete <parent>` |
