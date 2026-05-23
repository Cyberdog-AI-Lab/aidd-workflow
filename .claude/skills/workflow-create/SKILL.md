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

## ステップ 2：コマンド変数の確認（必要に応じて）

タスクの `prompt` で `{{vars.key}}` 形式を使うと、コマンドを変数として参照できる。
ここでは `vars` に登録するコマンドを確認する。

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
  タスク名（task）：> ← 簡潔なタスク名を入力

  --- 実行方法（agents と prompt/skills は排他。それ以外は組み合わせ可）---
  prompt（エージェントへの指示文、任意）：>
    ※ {{vars.key}} 形式でコマンドを参照できる
  skills（呼び出すスキル名、任意・スペース区切り）：>
  agents（並列起動するカスタムエージェント名、任意・スペース区切り）：>
    ※ .claude/agents/<name>.md として定義されるエージェント
    ※ agents を指定する場合は prompt / skills との併用不可

  --- アクセス制御・フロー制御（任意）---
  outputs（編集許可パターン、任意・スペース区切り glob または /regex/）：>
    ※ 例: src/** tests/**  ← 空欄でスキップ
  approval（このタスク完了後に承認を要求する？）[y/n]：>
  deny files（編集を禁止するパターン、任意・スペース区切り）：>
  deny commands（禁止するコマンドパターン、任意・スペース区切り）：>

タスクを追加しますか？ [y/n]：
```

### 各フィールドのバリデーション

| フィールド | ルール |
|-----------|-------|
| `id` | `^[a-z][a-z0-9_-]*$`。重複不可（同一ワークフロー内） |
| `task` | 空白入力不可（手動タスクでは必須） |
| 実行方法 | `prompt` / `skills` / `agents` のうち **少なくとも 1 つを必ず定義する** |
| `prompt` | `{{vars.key}}` 形式でコマンド参照可 |
| `skills` | スキル名を 1 つ以上（スペース区切り or 1 行 1 件） |
| `agents` | エージェント名を 1 つ以上。`prompt` / `skills` との **併用不可** |
| `outputs` | glob または `/regex/` パターン。指定するとエージェントの編集範囲をそのパスに限定 |
| `approval` | `true` にするとタスク完了後にワークフローが一時停止し承認を待つ |
| `deny.files` | 編集を禁止するパスパターン（glob または `/regex/`） |
| `deny.commands` | 実行を禁止するコマンドパターン（部分一致または `/regex/`） |

**`prompt` / `skills` / `agents` のいずれも指定されていない場合**：
「実行方法が未定義です。prompt・skills・agents のいずれかを必ず指定してください。」と伝えて再入力を促す。

**`agents` と `prompt` または `skills` を同時に指定した場合**：
「agents は prompt・skills と併用できません。どちらかを選んでください。」と伝えて再入力を促す。

---

## ステップ 3.5：タスク間の依存関係を設定する

全タスクの定義が終わったら、実行順序（`requires`）を確認する。  
依存関係は「このタスクが開始するには、どのタスクが完了している必要があるか」を表す。

```
定義されたタスク：
  1. {id-1}  {task名}
  2. {id-2}  {task名}
  3. {id-3}  {task名}
  ...

各タスクの依存関係を設定します（空欄でスキップ）：
  {id-2} が開始する前に完了している必要があるタスク：> {id-1}
  {id-3} が開始する前に完了している必要があるタスク：> {id-2}
  ...
```

- 存在しない `id` を指定した場合は「`{id}` は定義されていません。」と伝えて再入力を促す
- 循環依存（A → B → A など）が生じる場合は警告して修正を促す
- 依存関係がない（すべて並列実行可）場合は `requires` を省略する

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
      prompt: |               # prompt を指定した場合
        {prompt内容}
      skills:                 # skills を指定した場合
        - {skill名}
      agents:                 # agents を指定した場合
        - {agent名}
      outputs:                # outputs を指定した場合
        - {パターン}
      requires:               # requires を指定した場合
        - {依存タスクid}
      approval: true          # approval が true の場合のみ出力
      deny:                   # deny を指定した場合
        files:
          - {パターン}
        commands:
          - {パターン}
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

# 複数ファイルへの分割が必要な場合（任意）：
# imports:
#   - workflows/extra.yml

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
| `task` が空（手動タスクの場合） | 「タスク名は必須です。入力してください。」と再入力を促す |
| 実行方法が未定義 | 「prompt・skills・agents のいずれかを必ず指定してください。」と再入力を促す |
| `agents` と `prompt`/`skills` を同時指定 | 「agents は prompt・skills と併用できません。」と再入力を促す |
| `requires` に未定義の `id` を指定 | 「`{id}` は定義されていません。」と再入力を促す |
| スキーマ警告が出た | 自己修正してから完了報告する |
| `.workflow/` ディレクトリが存在しない | `mkdir -p .workflow` を実行してから書き込む |
