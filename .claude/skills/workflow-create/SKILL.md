---
name: workflow-create
description: >
  .workflow/config.yml に新しいワークフローテンプレートをインタラクティブに追加するスキル。
  「ワークフローを作りたい」「新しいフローを定義して」「hotfix フローを追加して」
  「config.yml を作りたい」「config.yml が存在しない」「ワークフローを登録したい」
  「カスタムフローを追加して」「デプロイフローを作りたい」など、
  ワークフローを新規作成・追加したいときは必ずこのスキルを使うこと。
  /workflow-runner から「config.yml が見つかりません」と案内された直後も使う。
---

# Workflow Create スキル

`.workflow/config.yml` に新しいワークフローをインタラクティブに定義・追記する。

---

## 前提確認

1. `.workflow/config.yml` が存在するか確認する
   - **存在する場合**：ファイルを読み、既存の `vars` キーと `workflows` スラッグを把握する
   - **存在しない場合**：新規作成モードで進める（後述）

2. `.workflow/workflow.schema.json` が存在するか確認する（バリデーション用）

---

## ステップ 1：ワークフロー基本情報をヒアリング

以下の形式でユーザーに質問する：

```
## ワークフロー作成

**ワークフロー名（スラッグ）：** 英小文字・ハイフン区切り（例: hotfix, release, review）
**説明（任意）：**
```

- スラッグが既存の `workflows` キーと重複する場合は「`{slug}` はすでに定義されています。上書きしますか？」と確認する
- スラッグは `^[a-z][a-z0-9-]*$` を満たす形式で入力を促す

---

## ステップ 2：コマンドの確認（必要に応じて）

gate タスクで使うコマンドを定義する。

- **config.yml が存在する場合**：現在の `vars` を表示する
  ```
  現在の変数： test: make test / lint: make lint / build: make build
  ```
- **新規作成の場合**：以下を確認する
  ```
  テストコマンドを教えてください（例: make test / npm test / pytest）：
  ```
  lint・build コマンドも「追加しますか？」で確認する（任意）

- **ヒアリング中に新しいコマンドキーが必要になった場合**：
  ```
  `vars` に新しい変数を追加しますか？
  キー名：> deploy
  コマンド：> make deploy
  ```

---

## ステップ 3：タスクをインタラクティブに定義

最低 2 タスクを推奨する。以下の形式でループする：

```
**タスク {N} を定義してください：**
  id（英小文字・ハイフン区切り）：>
  タスク名（task）：> ← 簡潔なタスク名を入力（必須）
  実行方法を選択してください：
    1. prompt（エージェントが自動実行）
    2. skills（スキルを呼び出す）
    3. agents（カスタムエージェントを並列起動）
    4. prompt + skills（自動実行後にスキル呼び出し）
  ※ いずれかを必ず選択してください（手動タスクは作成不可）

タスクを追加しますか？ [y/n]：
```

### 各フィールドのバリデーション

| フィールド | ルール |
|-----------|-------|
| `id` | `^[a-z][a-z0-9_-]*$`。重複不可（同一ワークフロー内） |
| `task` | 必須。空白入力は不可 |
| 実行方法 | `prompt` / `skills` / `agents` のうち **少なくとも 1 つを必ず定義する** |
| `prompt` | 選択時は内容を入力（`{{vars.key}}` 形式でコマンド参照可） |
| `skills` | 選択時はスキル名を 1 つ以上入力（スペース区切り or 1 行 1 件） |
| `agents` | 選択時はエージェント名を 1 つ以上入力（`.claude/agents/<name>.md` として定義される） |

**`prompt` / `skills` / `agents` のいずれも指定されていない場合は**：
「実行方法が未定義です。prompt・skills・agents のいずれかを必ず指定してください。」と伝えて再入力を促す。

---

## ステップ 4：プレビューを表示して確認を取る

以下の形式でプレビューを表示する：

```yaml
{slug}:
  name: {name}
  description: {description}  # description が空の場合は省略
  tasks:
    - id: {id}
      task: {task名}
      prompt: |               # prompt を選択した場合
        {prompt内容}
      skills:                 # skills を選択した場合
        - {skill名}
      agents:                 # agents を選択した場合
        - {agent名}
    ...
```

「`config.yml` に追記しますか？ [y/n]：」で確認を取る。

---

## ステップ 5：config.yml に書き込む

### config.yml が存在する場合（追記）

`workflows:` セクションの末尾に新しいワークフローを追記する。

- 同名スラッグを上書きする場合は既存ブロックを置き換える
- インデントは 2 スペース統一

### config.yml が存在しない場合（新規作成）

以下の構造でファイルを新規作成する：

```yaml
# yaml-language-server: $schema=./workflow.schema.json
# workflow-runner 設定ファイル

vars:
  test: {テストコマンド}
  # lint: make lint
  # build: make build

workflows:
  {slug}:
    name: {name}
    description: {description}
    tasks:
      ...
```

### 書き込み後の処理

- Edit または Write ツールで書き込むと、`post-edit` フックがスキーマを自動検証する
- **スキーマ警告が出た場合**：ユーザーへの完了報告前に必ず自己修正する

---

## ステップ 6：完了報告

```
✅ ワークフロー `{slug}` を config.yml に追記しました。

実行するには：
  /workflow-runner {slug}
```

---

## エラー対応

| 状況 | 対応 |
|------|------|
| スラッグ重複（上書き拒否） | 別のスラッグで再入力を促す |
| `task` が空 | 「タスク名は必須です。入力してください。」と再入力を促す |
| 実行方法が未定義 | 「prompt・skills・agents のいずれかを必ず指定してください。」と再入力を促す |
| `agents` に未定義キーを指定 | 「追加しますか？」で vars への追加を提案 |
| スキーマ警告が出た | 自己修正してから完了報告する |
| `.workflow/` ディレクトリが存在しない | `mkdir -p .workflow` を実行してから書き込む |
