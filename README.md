# aidd-workflow

Claude Code でワークフローを **決定論的に** 強制実行するツールキット。
「テストを飛ばして完了とされる」「多段タスクでルールを忘れる」問題を
Rust 製エンジン（`workflow-runner`）+ Hooks で構造的に解決する。

## 仕組み

```
workflow-runner serve          （常駐デーモンを起動。この時点ではワークフローを実行しない）
        ↓
workflow-runner run bug-fix    （稼働中の serve に POST /run。workflow_id を発行）
        ↓（HTTP POST → Channels webhook :8788）
Claude Code セッションがタスクを受信（<channel> イベント）
        ↓（prompt に従って実装・テスト等を実行）
curl -sX POST :8789/complete/{workflow_id}/{task_id}
        ↓（workflow-runner が次のタスクを dispatch）
完了するまでループ（同じ serve 上で複数ワークフローを並行実行可能）
        ↓（approval: true のタスクなら）
awaiting_approval → workflow-runner approve で承認 / workflow-runner reject <task-id> で却下
```

詳細なアーキテクチャは [ARCHITECTURE.md](./ARCHITECTURE.md) を参照。

## セットアップ

### 0. セットアップ（v5 以降）

```bash
# workflow-runner を PATH に配置後
workflow-runner setup
# → .workflow/ と .claude/settings.json を自動生成・更新（シェルスクリプト不要）
```

### 1. インストール（バイナリ配布）

```bash
curl -fsSL https://raw.githubusercontent.com/Cyberdog-AI-Lab/aidd-workflow/main/install.sh | bash
```

特定バージョンを指定する場合：

```bash
VERSION=v0.0.1 bash <(curl -fsSL https://raw.githubusercontent.com/Cyberdog-AI-Lab/aidd-workflow/main/install.sh)
```

### 2. Webhook MCP サーバーを Claude Code に登録

インストール後、`webhook-mcp` を Claude Code の MCP サーバーとして登録する：

```bash
claude mcp add webhook -- "${HOME}/.local/bin/webhook-mcp"
```

または `.claude/mcp.json` に直接記述する方法でも設定できる：

```json
{
  "mcpServers": {
    "webhook": {
      "command": "bun",
      "args": ["run", "channels/webhook.ts"],
      "env": { "PORT": "8788" }
    }
  }
}
```

登録後は `workflow-runner serve` でデーモンを起動し、`workflow-runner run <workflow>` で
ワークフローを自律実行できる。Claude Code セッションがタスクを
`<channel source="webhook" ...>` タグとして受信し、`channels/webhook.ts` の `instructions`
に従って実行・コールバックを行う。

> **ポート変更**: デフォルトは `8788`。変更する場合は `--port <PORT>` を MCP サーバーの引数に追加する。
>
> ```bash
> claude mcp add webhook -- "${HOME}/.local/bin/webhook-mcp" --port 9000
> ```

### 3. ビルド（ソースから）

```bash
# CLI (workflow-runner)
cargo build --manifest-path cli/Cargo.toml

# Webhook MCP サーバー
cd channels && bun install && bun run build
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

### 自律実行モード（推奨）

Claude Code セッションを開いた状態で外部ターミナルから実行する。まずデーモンを起動し、
次にワークフローを起動する（2段階）：

```bash
workflow-runner serve &        # 常駐デーモンを起動（この時点では何も実行しない）
workflow-runner run bug-fix    # バグ修正フローを起動（新しい workflow_id が発行される）
workflow-runner run feature    # 同じデーモン上で機能開発フローも並行して起動できる
```

`run` は稼働中の `serve` デーモンに `POST /run` するだけの薄いクライアントで、呼ぶたびに
新しい `workflow_id` が発行される。`serve` は Channels 経由でタスクを Claude Code に push し、
人手を介さずワークフローをエンドツーエンドで実行する。**`serve` はワークフローが完了しても
自動終了しない** — 明示的に `workflow-runner stop` を呼ぶか、プロセスを終了させるまで稼働し続け、
その間いつでも `run` で新しいワークフローを追加できる。

```bash
./target/debug/workflow-runner stop                                    # デーモンを正常終了させる
```

`approval: true` のタスクでは自動停止する（`awaiting_approval`）。CLI から承認・却下できる：

```bash
./target/debug/workflow-runner approve                                 # 承認 → 次タスクへ
./target/debug/workflow-runner reject <task-id> --reason "<理由>"      # 却下 → タスクを再実行
```

エージェントが `pause` で作業を中断した場合（ユーザーへの確認待ちなど）は `resume` で再開する：

```bash
./target/debug/workflow-runner resume                                  # 中断タスクを再 dispatch
```

`run` / `stop` / `approve` / `resume` / `reject` はいずれも稼働中の `workflow-runner serve`
デーモンのコールバックサーバー（デフォルト `http://127.0.0.1:8789`）に HTTP POST するだけの
薄いクライアント。別ホスト・別ポートのデーモンに向ける場合は `--callback-port <PORT>` または
`--callback-url <URL>` を指定する。

同じデーモン上で複数ワークフローが並行している場合、`approve`/`resume`/`reject` は対象ステータス
（`awaiting_approval`/`paused`）の workflow が一意なら自動選択し、複数あれば
`--workflow-id <id>`（サブコマンドより前に指定するグローバルフラグ）で明示的に指定する：

```bash
workflow-runner --workflow-id 4fd261ba-... approve
```

curl で直接叩くことも可能（同じエンドポイントを使う。`workflow_id` は `run` の応答や
`status` で確認できる）：

```bash
curl -sX POST http://127.0.0.1:8789/run -d '{"workflow":"bug-fix"}'
curl -sX POST http://127.0.0.1:8789/approve/<workflow-id>
curl -sX POST http://127.0.0.1:8789/resume/<workflow-id>
curl -sX POST http://127.0.0.1:8789/reject/<workflow-id>/<task-id> \
  -H 'Content-Type: application/json' \
  -d '{"reason":"設計が不十分です"}'
curl -sX POST http://127.0.0.1:8789/stop
```

その他の CLI コマンド（デーモンの起動を必要としない）：

```bash
./target/debug/workflow-runner list                                    # ワークフロー一覧
./target/debug/workflow-runner status                                  # 現在の状態（JSON）
./target/debug/workflow-runner status --format table                   # 現在の状態（ターミナルテーブル）
./target/debug/workflow-runner validate                                # config.yml 検証（JSON）
./target/debug/workflow-runner validate --format text                  # config.yml 検証（人間可読）
./target/debug/workflow-runner setup                                   # .workflow/ と .claude/settings.json を生成・更新
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
# 承認（approve が承認ゲートを解除して次タスクを dispatch）
./target/debug/workflow-runner approve

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

cli/                             # workflow-runner（Rust）
├── Cargo.toml
└── src/
    ├── main.rs                  CLI エントリポイント
    ├── config/                  YAML パース・型定義・imports 解決
    ├── engine/                  DAG 評価・SQLite 状態管理・gate チェック
    ├── adapters/
    │   └── hooks/               Claude Code フック処理（providers 経由）
    ├── providers/
    │   └── claude_code/         Claude Code hook JSON → 型安全な構造体
    ├── infra/                   settings.json 生成
    └── protocol/                JSON 入出力型・テーブルフォーマッター

channels/                        # Channels MCP サーバー（TypeScript / Bun）
└── webhook.ts                   HTTP POST → Claude Code channel 転送サーバー
                                 （instructions フィールドでワーカー動作を定義）

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
└── skills/
    └── workflow-creator/SKILL.md
```

## 依存

- Rust（`cargo build` でバイナリをビルド）
- Bun（`bun build` で webhook-mcp をビルド）

## ドキュメント

- [ARCHITECTURE.md](./ARCHITECTURE.md) — アーキテクチャ詳細
- [PLAN.md](./PLAN.md) — v5 実装計画（Phase 1–4 完了、Phase 5 計画中）
