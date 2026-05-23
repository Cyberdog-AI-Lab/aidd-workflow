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
  名前：>
  説明（任意）：>
  ゲート（任意、利用可能: {vars のキー一覧}）：>

タスクを追加しますか？ [y/n]：
```

### 各フィールドのバリデーション

| フィールド | ルール |
|-----------|-------|
| `id` | `^[a-z][a-z0-9_-]*$`。重複不可（同一ワークフロー内） |
| `gate` | `vars` に定義されたキーのみ。未定義キーを指定したら「`{gate値}` は vars に定義されていません。追加しますか？」と促す |

`description`・`gate` は空白入力でスキップ（YAML に出力しない）。

---

## ステップ 4：プレビューを表示して確認を取る

以下の形式でプレビューを表示する：

```yaml
{slug}:
  name: {name}
  description: {description}  # description が空の場合は省略
  tasks:
    - id: {id}
      name: {name}
      description: {description}  # 空の場合は省略
      gate: {gate}                # 空の場合は省略
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
| gate に未定義キーを指定 | 「追加しますか？」で vars への追加を提案 |
| スキーマ警告が出た | 自己修正してから完了報告する |
| `.workflow/` ディレクトリが存在しない | `mkdir -p .workflow` を実行してから書き込む |
