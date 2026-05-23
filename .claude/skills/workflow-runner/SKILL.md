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

`workflow-runner` バイナリが判断ロジックを持ち、このスキルはアクションを dispatch するだけ。
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

### 2. `start` を実行して最初のアクションを取得する

```bash
./target/debug/workflow-runner start <workflow-name>
```

出力 JSON の `actions` 配列を処理する。

---

## アクションの dispatch

`actions` 配列の各 `ActionItem` を `type` フィールドに従って実行する。

| type | 実行方法 |
|------|---------|
| `agent` | Agent ツールで `prompt` を実行する。`background: true` なら並列実行してよい |
| `skill` | Skill ツールで `skill` を呼び出す |
| `workflow` | `./target/debug/workflow-runner start <workflow>` を再帰的に実行する |
| `manual` | `description` に従って Claude が作業する |

---

## アクション完了後の処理

各アクション実行後、`agent` / `skill` タイプは結果を report する：

```bash
echo '{"session_id":"<id>","task_id":"<task>","action_index":<n>,"action_type":"<type>","exit_code":<code>,"stdout":"<out>","stderr":""}' \
  | ./target/debug/workflow-runner report
```

---

## タスク完了

1タスクの全アクションが終わったら complete を呼ぶ：

```bash
./target/debug/workflow-runner complete <task-id>
```

### レスポンスの解釈

| `allowed` | `next.status` | 対応 |
|-----------|---------------|------|
| `false` | - | `reason` をユーザーに伝えてブロック。gate 未実行なら該当コマンドを実行 |
| `true` | `in_progress` | `next.actions` を dispatch する |
| `true` | `completed` | ワークフロー完了。完了サマリーを表示する |
| `true` | `blocked` | 未解決の requires 依存がある。`status` で確認する |

---

## サブエージェントアクションの実行

### agents ブロック（`sub_agent: true`）

`actions` 配列に `"sub_agent": true` の `ActionItem` が含まれる場合、それらは `agents` ブロックのサブエージェントを表す。`task_id` は `"parent/sub"` 形式になる。

```
actions: [
  { "task_id": "quality-check/run-test", "sub_agent": true, "type": "agent", "prompt": "make test を実行してください" },
  { "task_id": "quality-check/run-lint", "sub_agent": true, "type": "agent", "prompt": "make lint を実行してください" },
  { "task_id": "quality-check/security", "sub_agent": true, "type": "skill", "skill": "security-review" }
]
```

実行方針：
- `sub_agent: true` のアクションは Agent ツールでサブエージェントとして起動する
- `background: true` → 複数サブエージェントを並列起動する
- `background: false` → 順番に実行する

各アクション完了後、`report` を呼んで結果を記録する。`task_id` はサブエージェント ID（`parent/sub`）を使う。

全サブエージェントが完了したら、**親タスクの ID**（`/` を含まない部分）で `complete` を呼ぶ：

```bash
./target/debug/workflow-runner complete quality-check
```

### エージェントアクション内の background（`sub_agent: false`）

同一タスクの `actions` 配列に複数の `ActionItem` が含まれ、`type: agent` かつ `background: true` のものがある場合：
- `background: true` の `agent` / `skill` は Agent ツールで並列起動する
- `background: false` は順番に実行する

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
| gate ブロック | `reason` を伝えて該当コマンドを実行してから再度 `complete` |
| セッション中断 | `workflow-runner next` で再開情報を取得 |
| config.yml の警告 | スキーマ警告が出たら自己修正してから報告 |
