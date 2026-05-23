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
SKILL.md が actions を dispatch
        ↓
workflow-runner complete <task-id>
        ↓（requires 未達なら）
ブロック「依存タスクが未完了です」
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

`.workflow/config.yml` でプロジェクトの変数を設定する：

```yaml
vars:
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
./target/debug/workflow-runner --workflow-id <id> complete <task-id>  # タスク完了
./target/debug/workflow-runner status                         # 現在の状態（JSON）
./target/debug/workflow-runner status --format table          # 現在の状態（ターミナルテーブル）
./target/debug/workflow-runner validate                       # config.yml 検証（JSON）
./target/debug/workflow-runner validate --format text         # config.yml 検証（人間可読）
./target/debug/workflow-runner init                           # .claude/settings.json を生成
./target/debug/workflow-runner update                         # settings.json の hook 設定を更新
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
  - vars/default.yml
  - workflows/release.yml
```

```yaml
# .workflow/workflows/release.yml
workflows:
  release:
    name: リリースフロー
    tasks:
      # 手動タスク（description に従って Claude が作業）
      - id: design
        name: 設計確認
        description: 実装方針・影響範囲を整理して記録する
        allow_files:             # InProgress 中は docs/ 以下のみ編集可
          - "docs/**"

      - id: implement
        name: 実装
        requires: [design]
        allow_files:
          - "src/**"
          - "tests/**"

      # サブエージェントタスク（agents ブロック）
      - id: quality-check
        name: 品質チェック
        requires: [implement]
        agents:
          - id: lint
            actions:
              - type: agent
                prompt: "{{vars.lint}} を実行して Lint が通ることを確認してください"
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

> **廃止**: `type: run`（`actions` に移行）、`type: workflow`（`imports:` で代替）

## ファイル構成

```
install.sh                       バイナリインストールスクリプト（macOS/Linux）

src/                             # workflow-runner（Rust）
├── main.rs                      CLI エントリポイント
├── config/                      YAML パース・型定義・imports 解決
├── engine/                      DAG 評価・SQLite 状態管理・gate チェック
├── adapters/
│   └── hooks/                   Claude Code フック処理（providers 経由）
├── providers/
│   └── claude_code/             Claude Code hook JSON → 型安全な構造体
├── infra/                       settings.json 生成
└── protocol/                    JSON 入出力型・テーブルフォーマッター

.github/workflows/
└── release.yml                  GitHub Actions リリースパイプライン（4ターゲット）

.workflow/
├── config.yml                   ワークフロー定義（編集する）
├── vars/                        変数定義（imports で読み込む）
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

## ドキュメント

- [ARCHITECTURE.md](./ARCHITECTURE.md) — アーキテクチャ詳細
- [PLAN.md](./PLAN.md) — v5 実装計画（Phase 1–4 完了、Phase 5 計画中）
