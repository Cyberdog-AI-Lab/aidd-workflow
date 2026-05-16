# aidd-workflow

Claude Code でワークフローを **決定論的に** 強制実行するツールキット。
「テストを飛ばして完了とされる」「多段タスクでルールを忘れる」問題を
Rust 製エンジン（`workflow-runner`）+ Hooks で構造的に解決する。

## 仕組み

```
/workflow-orchestrator bug-fix
        ↓
workflow-runner start bug-fix
        ↓（JSON でアクション群を返す）
SKILL.md が actions / post_commands を dispatch
        ↓（post_commands = ゲートコマンド）
make test 実行
        ↓ workflow-runner report で記録
workflow.db の gate_recorded = true
        ↓
workflow-runner complete test
        ↓（gate_recorded = false なら）
ブロック「post_commands が未実行です」
```

## セットアップ

### 0. セットアップ（v5 以降）

```bash
# workflow-runner を PATH に配置後
workflow-runner init
# → .claude/settings.json を自動生成（シェルスクリプト不要）
```

### 1. インストール（バイナリ配布）

```bash
curl -fsSL https://raw.githubusercontent.com/cyberdog/aidd-workflow/main/install.sh | bash
```

特定バージョンを指定する場合：

```bash
VERSION=v0.2.0 bash <(curl -fsSL https://raw.githubusercontent.com/cyberdog/aidd-workflow/main/install.sh)
```

### 2. ビルド（ソースから）

```bash
cargo build
```

### 3. config.yml を編集

`.workflow/config.yml` でプロジェクトのコマンドを設定する：

```yaml
commands:
  test: npm test
  lint: npm run lint
  build: npm run build
```

## 使い方

### Claude Code スキルから起動

```
/workflow-orchestrator bug-fix    # バグ修正フローを開始
/workflow-orchestrator feature    # 機能開発フローを開始
/workflow-orchestrator            # 中断フローの再開 or ワークフロー選択
```

### CLI から直接操作

```bash
./target/debug/workflow-runner list                           # ワークフロー一覧
./target/debug/workflow-runner start bug-fix                  # 開始（workflow_id を返す）
./target/debug/workflow-runner --workflow-id <id> next        # 次のアクション確認
./target/debug/workflow-runner --workflow-id <id> complete <step-id>  # ステップ完了
./target/debug/workflow-runner status                         # 現在の状態（JSON）
./target/debug/workflow-runner status --format table          # 現在の状態（ターミナルテーブル）
./target/debug/workflow-runner validate                       # config.yml 検証（JSON）
./target/debug/workflow-runner validate --format text         # config.yml 検証（人間可読）
./target/debug/workflow-runner init                           # .claude/settings.json を生成
./target/debug/workflow-runner update                         # settings.json の hook 設定を更新
```

### standalone アダプターで自律実行（AI ツール不要）

`exec-step` を使うと `run` / `agent` アクションを workflow-runner が直接実行する。

```bash
# ANTHROPIC_API_KEY が必要（agent アクションを使う場合）
export ANTHROPIC_API_KEY=sk-ant-...

./target/debug/workflow-runner --adapter standalone start feature
./target/debug/workflow-runner --adapter standalone exec-step implement
./target/debug/workflow-runner --adapter standalone exec-step test
```

## ワークフローの追加

```
/workflow-create    # インタラクティブに新しいワークフローを追加
```

または `.workflow/config.yml` を直接編集する。

### config.yml の記述例（v5 スキーマ）

```yaml
# .workflow/config.yml
imports:
  - commands/default.yml
  - workflows/release.yml
```

```yaml
# .workflow/workflows/release.yml
workflows:
  release:
    name: リリースフロー
    steps:
      # 手動ステップ（description に従って Claude が作業）
      - id: design
        name: 設計確認
        description: 実装方針・影響範囲を整理して記録する
        allow_files:             # InProgress 中は docs/ 以下のみ編集可
          - "docs/**"

      - id: implement
        name: 実装
        requires: [design]
        guards:                  # design の成果物（docs/ ファイル）が存在することを確認
          - step: design
            required_files: ["docs/**/*.md"]
        allow_files:
          - "src/**"
          - "tests/**"
        post_commands:           # ステップ完了前にゲートとして実行
          - "{{commands.test}}"  # 失敗すると complete できない

      # 並列ステップ
      - id: quality-check
        name: 品質チェック
        requires: [implement]
        parallel:
          - id: lint
            post_commands:
              - "{{commands.lint}}"
          - id: security
            actions:
              - type: skill
                skill: security-review

      - id: done
        name: 完了
        requires: [quality-check]
```

### アクション型（v5）

| `type` | 説明 |
|--------|------|
| `agent` | サブエージェント起動。`background: true` で並列実行可 |
| `skill` | スキル呼び出し |
| `pre_commands` | ステップ開始時に自動実行するシェルコマンド（Step フィールド） |
| `post_commands` | ステップ完了前にゲートとして実行するシェルコマンド（Step フィールド） |

> **廃止**: `type: run`（`pre_commands` / `post_commands` に移行）、`type: workflow`（`imports:` で代替）

## ファイル構成

```
install.sh                       バイナリインストールスクリプト（macOS/Linux）

src/                             # workflow-runner（Rust）
├── main.rs                      CLI エントリポイント
├── config/                      YAML パース・型定義・imports 解決
├── engine/                      DAG 評価・SQLite 状態管理・gate/guards チェック
├── adapters/
│   ├── claude_code/             Claude Code フック処理（providers 経由）
│   └── standalone/              run_command / Claude Code Channels
├── providers/
│   └── claude_code/             Claude Code hook JSON → 型安全な構造体
├── infra/                       settings.json 生成（init/update コマンド）
└── protocol/                    JSON 入出力型・テーブルフォーマッター

.github/workflows/
└── release.yml                  GitHub Actions リリースパイプライン（4ターゲット）

.workflow/
├── config.yml                   ワークフロー定義（編集する）
├── commands/                    コマンド定義（imports で読み込む）
├── workflows/                   ワークフロー定義（imports で読み込む）
├── workflow.schema.json         JSON Schema（編集不要）
└── workflow.db                  実行状態 SQLite（自動生成）

.claude/
├── hooks/
│   └── post-edit-rust-checks.sh  .rs 編集後の自動品質チェック
└── skills/
    ├── workflow-orchestrator/SKILL.md
    └── workflow-create/SKILL.md
```

## 依存

- Rust（`cargo build` でバイナリをビルド）
- `ANTHROPIC_API_KEY`（v4 の standalone アダプターで `agent` アクションを使う場合のみ。v5 以降は Claude Code Channels に移行）

## ドキュメント

- [ARCHITECTURE.md](./ARCHITECTURE.md) — アーキテクチャ詳細（v5 目標設計）
- [PLAN.md](./PLAN.md) — v5 実装計画（4フェーズ）
