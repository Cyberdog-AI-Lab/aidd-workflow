# workflow-orchestrator

Claude Code でワークフローを強制実行するツールキット。
「テストを飛ばして完了とされる」「多段タスクでルールを忘れる」問題を Hooks + Tasks API で解決する。

## 仕組み

```
/workflow-orchestrator bug-fix
        ↓
Tasks API でステップをタスク化
        ↓
各ステップを順番に実行
        ↓（gate: test のステップで）
make test 実行
        ↓ PostToolUse hook が自動記録
checklist.md にテスト結果を記録
        ↓
TaskUpdate(completed) → PreToolUse hook がゲートチェック
        ↓（記録なし）
ブロック「先に make test を実行してください」
```

## セットアップ

`.workflow/config.yml` を編集してプロジェクトのコマンドを設定する：

```yaml
commands:
  test: npm test      # プロジェクトのテストコマンド
  lint: npm run lint
  build: npm run build
```

## 使い方

```
/workflow-orchestrator bug-fix    # バグ修正フローを開始
/workflow-orchestrator feature    # 機能開発フローを開始
/workflow-orchestrator            # 中断フローの再開 or ワークフロー選択
```

## ワークフローの追加

```
/workflow-create    # インタラクティブに新しいワークフローを追加
```

または `.workflow/config.yml` を直接編集する。

## ファイル構成

```
.claude/
├── hooks/
│   ├── post-bash-capture-test.sh    # テスト出力を checklist.md に記録
│   ├── pre-taskupdate-gate.sh       # テスト未実行ならタスク完了をブロック
│   └── post-edit-validate-config.sh # config.yml 編集後にスキーマ検証
├── skills/
│   ├── workflow-orchestrator/
│   │   └── SKILL.md
│   └── workflow-create/
│       └── SKILL.md
└── settings.json

.workflow/
├── config.yml             # ワークフロー定義（編集する）
├── workflow.schema.json   # JSON Schema（編集不要）
└── checklist.md           # 作業記録（自動生成）
```

## 依存

- Python 3（標準インストール済み想定）
- スキーマ検証（オプション）：`pip install pyyaml jsonschema`
# aidd-workflow
