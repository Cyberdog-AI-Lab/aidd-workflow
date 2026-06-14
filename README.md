# aidd-workflow

Claude Code でワークフローを **決定論的に** 強制実行するツールキット。
「テストを飛ばして完了とされる」「多段タスクでルールを忘れる」問題を
Rust 製エンジン（`workflow-runner`）+ Hooks で構造的に解決する。

## 仕組み

```
/workflow-runner bug-fix
        ↓
workflow-runner start bug-fix
        ↓（JSON で tasks 配列を返す）
SKILL.md が tasks を dispatch
  ・prompt あり → Agent で実行
  ・skills あり → Skill ツールで呼び出し
  ・agents あり → .claude/agents/<name>.md を並列起動
  ・すべて空    → 手動タスク（description に従って作業）
        ↓
workflow-runner complete <task-id>
        ↓（requires/agents 未達なら）
ブロック「依存タスクが未完了です」
        ↓（approval: true のタスクなら）
承認待ち → next で承認 / reject <task-id> で却下・再実行
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
VERSION=v0.0.1 bash <(curl -fsSL https://raw.githubusercontent.com/cyberdog/aidd-workflow/main/install.sh)
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
/workflow-runner bug-fix    # バグ修正フローを開始
/workflow-runner feature    # 機能開発フローを開始
/workflow-runner            # 中断フローの再開 or ワークフロー選択
```

### CLI から直接操作

```bash
./target/debug/workflow-runner list                                    # ワークフロー一覧
./target/debug/workflow-runner start bug-fix                           # 開始（tasks JSON を返す）
./target/debug/workflow-runner next                                    # 次のタスク確認 / 承認
./target/debug/workflow-runner complete <task-id>                      # タスク完了
./target/debug/workflow-runner complete <parent-id>/<agent-name>       # エージェント完了
./target/debug/workflow-runner reject <task-id> --reason "<理由>"      # タスク却下・再実行
./target/debug/workflow-runner status                                  # 現在の状態（JSON）
./target/debug/workflow-runner status --format table                   # 現在の状態（ターミナルテーブル）
./target/debug/workflow-runner validate                                # config.yml 検証（JSON）
./target/debug/workflow-runner validate --format text                  # config.yml 検証（人間可読）
./target/debug/workflow-runner init                                    # .claude/settings.json を生成
./target/debug/workflow-runner update                                  # settings.json の hook 設定を更新
```

## ワークフローの追加

```
/workflow-creator    # インタラクティブに新しいワークフローを追加
```

または `.workflow/config.yml` を直接編集する。

### config.yml の記述例

```yaml
# .workflow/config.yml
vars:
  test: npm test
  lint: npm run lint
  build: npm run build

workflows:
  release:
    name: リリースフロー
    description: 設計・実装・品質チェック（並列）まで完走するフロー
    tasks:
      # 手動タスク（prompt/skills/agents すべて省略。task が必須）
      - id: design
        task: 設計する
        outputs:           # InProgress 中は docs/ 以下のみ編集可
          - "docs/**"
        approval: true     # 完了後に開発者の承認を得てから次へ進む

      # プロンプトタスク（Agent で実行）
      - id: implement
        task: 実装する
        prompt: |
          設計書に従って実装してください。
          実装後は {{vars.build}} でビルドを確認してください。
        outputs:
          - "src/**"
          - "tests/**"
        requires: [design]

      # エージェントタスク（.claude/agents/ 以下を並列起動）
      - id: quality-check
        task: 品質チェック
        requires: [implement]
        agents:
          - run-test    # → .claude/agents/run-test.md
          - run-lint    # → .claude/agents/run-lint.md

      - id: complete
        task: 完了確認
        prompt: |
          設計・実装・品質チェックが完了したことを確認してリリースサマリーを報告してください。
        requires: [quality-check]
        approval: true
```

### タスク種別

| 条件 | 実行方法 |
|------|---------|
| `prompt` あり（`agents` なし） | Agent ツールで `prompt` を実行する |
| `skills` あり | 各スキルを Skill ツールで呼び出す |
| `prompt` と `skills` 両方あり | `prompt` を Agent で実行後、`skills` を順に呼ぶ |
| `agents` あり | `.claude/agents/<name>.md` を並列起動（`prompt`/`skills` と併用不可） |
| すべて空 | 手動タスク。Claude が `task` の指示名を手がかりに直接作業する（`task` フィールドが必須） |

### approval フロー

`approval: true` を付けたタスクは、`complete` 後に `awaiting_approval` 状態へ遷移する。

```bash
# 承認（next が承認ゲートを解除して次タスクを返す）
./target/debug/workflow-runner next

# 却下（タスクを InProgress に戻して再実行）
./target/debug/workflow-runner reject <task-id> --reason "設計が不十分です"
```

### カスタムエージェント

`agents:` に指定した名前に対応する `.claude/agents/<name>.md` を用意する。
ファイルの内容は Claude Code のサブエージェント定義（Markdown 形式）。

```
.claude/agents/
├── run-test.md    # テスト実行エージェント
└── run-lint.md    # Lint 実行エージェント
```

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
├── agents/                      カスタムエージェント定義（agents: で参照）
│   ├── run-test.md
│   └── run-lint.md
├── hooks/
│   └── post-edit-rust-checks.sh  .rs 編集後の自動品質チェック
└── skills/
    ├── workflow-runner/SKILL.md
    └── workflow-creator/SKILL.md
```

## 依存

- Rust（`cargo build` でバイナリをビルド）

## ドキュメント

- [ARCHITECTURE.md](./ARCHITECTURE.md) — アーキテクチャ詳細
- [PLAN.md](./PLAN.md) — v5 実装計画（Phase 1–4 完了、Phase 5 計画中）
