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
        ↓（gate: true のアクションで）
make test 実行
        ↓ workflow-runner report で記録
state.json の gate_recorded = true
        ↓
workflow-runner complete test
        ↓（gate_recorded = false なら）
ブロック「gate アクションが未実行です」
```

## セットアップ

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
./target/debug/workflow-runner start bug-fix                  # 開始
./target/debug/workflow-runner next                           # 次のアクション確認
./target/debug/workflow-runner complete <step-id>             # ステップ完了（ゲートチェック付き）
./target/debug/workflow-runner status                         # 現在の状態（JSON）
./target/debug/workflow-runner status --format table          # 現在の状態（ターミナルテーブル）
./target/debug/workflow-runner validate                       # config.yml 検証（JSON）
./target/debug/workflow-runner validate --format text         # config.yml 検証（人間可読）
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

### config.yml の記述例（新スキーマ）

```yaml
workflows:
  release:
    name: リリースフロー
    steps:
      # 手動ステップ（description に従って Claude が作業）
      - id: design
        name: 設計確認
        description: 実装方針・影響範囲を整理して記録する
        checklist_key: design

      - id: implement
        name: 実装
        requires: [design]

      # 自動ステップ（actions を宣言的に記述）
      - id: test
        name: テスト実行
        requires: [implement]
        actions:
          - type: run
            command: "{{commands.test}}"
            gate: true        # 実行記録がないと complete できない

      # 並列ステップ
      - id: quality-check
        name: 品質チェック
        requires: [implement]
        parallel:
          - id: lint
            actions:
              - type: run
                command: "{{commands.lint}}"
          - id: security
            actions:
              - type: skill
                skill: security-review

      - id: done
        name: 完了
        requires: [test, quality-check]
```

### アクション型

| `type` | 説明 |
|--------|------|
| `run` | シェルコマンド実行。`gate: true` で実行記録が完了の前提条件になる |
| `agent` | サブエージェント起動。`background: true` で並列実行可 |
| `skill` | スキル呼び出し |
| `workflow` | 別ワークフローをネスト実行 |

## ファイル構成

```
install.sh                       バイナリインストールスクリプト（macOS/Linux）

src/                             # workflow-runner（Rust）
├── main.rs                      CLI エントリポイント
├── config/                      YAML パース・型定義・ValidationError
├── engine/                      DAG 評価・状態管理・ゲートチェック
├── adapters/
│   ├── claude_code/             Claude Code フック処理
│   └── standalone/              run_command / call_anthropic_api
└── protocol/                    JSON 入出力型・テーブルフォーマッター

.github/workflows/
└── release.yml                  GitHub Actions リリースパイプライン（4ターゲット）

.workflow/
├── config.yml                   ワークフロー定義（編集する）
├── workflow.schema.json         JSON Schema（編集不要）
├── state.json                   実行状態（自動生成）
└── checklist.md                 作業記録（自動生成）

.claude/
├── hooks/                       workflow-runner を呼ぶ薄いラッパー
│   ├── post-bash-capture-test.sh
│   ├── pre-taskupdate-gate.sh
│   └── post-edit-validate-config.sh
└── skills/
    ├── workflow-orchestrator/SKILL.md
    └── workflow-create/SKILL.md
```

## 依存

- Rust（`cargo build` でバイナリをビルド）
- `ANTHROPIC_API_KEY`（standalone アダプターで `agent` アクションを使う場合のみ。モデル: `claude-sonnet-4-6`）

## ドキュメント

- [ARCHITECTURE.md](./ARCHITECTURE.md) — アーキテクチャ詳細
- [PLAN.md](./PLAN.md) — 実装計画（フェーズ別）
