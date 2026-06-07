# ROADMAP

aidd-workflow の中長期ロードマップ。

---

## 現在地

| フェーズ | 状態 |
|---------|------|
| v0.0.1　コア実装 | ✅ 完了 |
| v0.0.2　ワークフローの視覚化（ダッシュボード作成） | 🔲 未着手 |
| v0.0.3　自律駆動型ワークフロー | 🔲 未着手 |

---

## v0.0.2: ワークフローの視覚化（ダッシュボード作成）

### 目標

ローカル Web ダッシュボードを通じて、ワークフローの実行状況・履歴・承認待ちタスク・実行ログを視覚的に把握できるようにする。

### 背景・動機

現在の状況確認は `workflow-runner status` のテーブル出力に限られ、DAG 構造や実行履歴、承認待ちの全体像を俯瞰しにくい。
ブラウザベースのダッシュボードにより以下が実現する：

- DAG の進行状況をリアルタイムに可視化
- 実行履歴・所要時間の傾向を把握
- 承認待ちタスクをまとめて確認し、その場で承認/却下
- アクションレポート（コマンド実行結果・AI 出力）を詳細に確認

### アーキテクチャ概要

```
ブラウザ（React UI: DAG ビュー／タイムライン／承認キュー／ログビューア）
        │
        │  HTTP（REST API + SSE でライブ更新）
        ▼
┌──────────────────────────────────────┐
│  workflow-runner dashboard サーバー     │
│  （axum + tokio、127.0.0.1 のみバインド） │
│                                        │
│  GET  /api/workflows[/:id]             │
│  GET  /api/workflows/:id/timeline      │
│  GET  /api/workflows/:id/logs          │
│  POST /api/workflows/:id/approve       │
│  POST /api/workflows/:id/reject        │
│  GET  /api/events （SSE）               │
└──────────────┬─────────────────────────┘
               │  既存 engine / store 層をそのまま呼び出す（新規永続化層は作らない）
               ▼
        .workflow/workflow.db（SQLite、現在の cwd のみを対象）
```

### ディレクトリ構成

CLI ソースとフロントエンドを明確に分離する：

```
src/dashboard/        # axum ベースの HTTP サーバー + API 層（Rust）
dashboard-ui/         # React + Vite のフロントエンド（独立した npm プロジェクト）
  ├── package.json
  ├── src/
  └── dist/           # ビルド成果物（配布方法は実装時に検討）
```

### 実装方針

1. `workflow-runner dashboard` で HTTP サーバーを起動する（127.0.0.1 のみバインド、認証なし — ローカル開発ツールとして既存 CLI と同等の権限前提）
2. 既存の `engine` / `store` 層をそのまま呼び出し、新規の永続化層は作らない
3. 対象は単一プロジェクト（実行時の cwd 配下の `.workflow/workflow.db`）のみとする
4. フロントエンドは React + Vite で実装し、`dashboard-ui/` として CLI ソース（`src/`）と分離する
5. ビルド成果物の配布方法（`rust-embed` 等でバイナリへ埋め込むか、`dist/` を別途配置するか）は実装時に検討する

### フェーズ分割

#### 基盤 + リアルタイム進行状況

- `workflow-runner dashboard` コマンドでサーバーを起動
- DAG 上で各タスクの状態（pending/in_progress/completed/failed）を可視化
- SSE（Server-Sent Events）でライブ更新

#### 履歴・タイムライン

- 過去の実行（`workflow_runs`）の一覧表示
- 開始〜完了時刻、所要時間の推移をタイムライン/グラフで表示

#### 承認キュー・アクション

- `awaiting_approval` のタスクを一覧表示
- ブラウザから承認（`workflow-runner next` 相当）/ 却下（`workflow-runner reject` 相当）を実行

#### ログ詳細ビュー

- `action_reports`（コマンド実行結果・AI 出力）の詳細表示

### 使用ライブラリ候補

| 種別 | 候補 | 用途 |
|---|---|---|
| Rust | `axum` / `tokio` | HTTP サーバー・非同期ランタイム |
| Rust | `tower-http` | 静的アセット配信 |
| Rust | `rust-embed`（検討） | フロントエンド成果物をバイナリへ埋め込み、単一バイナリ配布を維持 |
| npm | `React` + `Vite` | フロントエンド UI |
| npm | DAG 可視化ライブラリ（検討: `react-flow` 等） | DAG の描画 |

### 完了基準

- `workflow-runner dashboard` でローカル Web サーバーが起動し、ブラウザから DAG・履歴・承認待ち・ログを確認できる
- 承認待ちタスクをブラウザから承認/却下できる
- `cargo test`（Rust 側）が全て通過する
- README.md / ARCHITECTURE.md が更新されている

---

## v0.0.3: 自律駆動型ワークフロー

Claude Code セッション外からワークフローを駆動できるようにし、AI ツール呼び出しの標準化（MCP 化）と外部プロセスによる自律実行を実現するフェーズ。

### Item 1: MCP サーバー化

#### 目標

`workflow-runner` を [Model Context Protocol (MCP)](https://modelcontextprotocol.io/) サーバーとして動作させ、
Claude Code・Cursor・その他 MCP 対応クライアントから **ツール呼び出し** でワークフローを操作できるようにする。

#### 背景・動機

現在の Claude Code 統合は SKILL.md（薄いブリッジ）+ CLI 呼び出しで成立している。
MCP サーバー化により以下が実現する：

- **統合の標準化** — SKILL.md の管理が不要になり、MCP 対応クライアントであればそのまま接続できる
- **双方向通知** — サーバー主導でクライアントに通知を送れる（ステップ完了・承認要求など）
- **ツール設計の明確化** — フック（Hooks）と MCP ツールでの役割分担が明確になる
- **マルチクライアント対応** — Cursor・Copilot・カスタムエージェントが同一プロトコルで接続できる

#### アーキテクチャ概要

```
MCP クライアント（Claude Code / Cursor / 任意エージェント）
        │
        │  MCP プロトコル（stdio / SSE）
        │
┌───────▼─────────────────────────────────────┐
│        workflow-runner MCP サーバー           │
│                                              │
│  MCP ツール（Claude Code ツール呼び出しに対応） │
│   workflow_start(workflow, cwd?)             │
│   workflow_next(workflow_id?)                │
│   workflow_complete(task_id, workflow_id?)   │
│   workflow_reject(task_id, reason?)          │
│   workflow_status(workflow_id?)              │
│   workflow_list(cwd?)                        │
│   workflow_validate(cwd?)                    │
└───────┬─────────────────────────────────────┘
        │
        │  既存 engine 層をそのまま呼び出す
        ▼
  .workflow/workflow.db（SQLite）
```

#### 実装方針

1. **既存 CLI プロトコルは維持** — `workflow-runner <command>` は引き続き動作する
2. **MCP レイヤーを追加** — `src/mcp/` として新モジュールを追加し、`engine` 層を直接呼ぶ
3. **起動モード切り替え** — `workflow-runner serve` で MCP サーバーとして起動
4. **Hook は維持** — `pre-edit` / `pre-bash` / `post-edit` は CLI フックのまま（MCP ツールとしては提供しない）

#### 使用クレート候補

| クレート | 用途 |
|---------|------|
| `rmcp` / `mcp-server-sdk` | Rust 製 MCP サーバー実装 |
| `tokio` | 非同期ランタイム（MCP サーバーに必要） |

#### 設定例（Claude Code 側）

```json
// .claude/mcp.json（プロジェクトレベル設定）
{
  "mcpServers": {
    "workflow-runner": {
      "command": "workflow-runner",
      "args": ["serve"]
    }
  }
}
```

#### 完了基準

- `workflow-runner serve` で MCP サーバーが起動する
- Claude Code の MCP クライアントから `workflow_start` / `workflow_complete` が呼び出せる
- 既存の CLI（`workflow-runner start` 等）が引き続き動作する
- SKILL.md なしで Claude Code からワークフローを操作できる
- `cargo test` が全て通過する
- README.md / ARCHITECTURE.md が更新されている

---

### Item 2: 外部プロセスによる自律ワークフロー制御

#### 目標

`workflow-runner` を外部プロセスから駆動し、AI ツールのセッション外で **完全自律的に** ワークフローをエンドツーエンド実行する。
Claude Code の **Channels（Research Preview）** の活用を検討する。

#### 背景・動機

現行の Claude Code Skill 統合は「人間がセッションを開いて `/workflow-runner` を呼ぶ」モデル。
外部プロセス駆動により以下が開放される：

- CI/CD パイプライン（Git push → 自動バグ修正・自動テスト修正）
- スケジューラー / cron による定期ワークフロー実行
- `awaiting_approval` タスクの Slack / GitHub 通知と外部承認
- 複数プロジェクトの並列ワークフロー監視

#### Claude Code Channels（Research Preview）の役割

Claude Code の Channels は **プロセス間通信チャンネル** を提供する Research Preview 機能。
外部プロセスが Claude Code セッションに命令を送ったり、セッションの出力を受け取る用途を想定する。

```
外部コントローラー
    │
    │  Channels API（stdin/stdout または WebSocket）
    ▼
Claude Code セッション（Channels 経由で受信）
    │
    │  SKILL.md または MCP ツール呼び出し
    ▼
workflow-runner（CLI / MCP）
    │
    ▼
.workflow/workflow.db
```

> **検討事項**: Channels の仕様は Research Preview のため変動する可能性がある。
> Channels が利用できない場合は代替の `claude -p`（非インタラクティブモード）で同等の動作を実現する。

#### 外部コントローラーの設計

```
┌────────────────────────────────────────────────────┐
│            外部コントローラー（Rust バイナリ）         │
│                                                    │
│  1. workflow-runner start <workflow>               │
│       → { workflow_id, tasks: [...] }             │
│                                                    │
│  2. tasks を受け取り、AI ツール（Claude Code 等）   │
│     に実行を依頼する                                │
│     （Channels / claude -p / MCP ツール呼び出し）   │
│                                                    │
│  3. 完了後: workflow-runner complete <task-id>     │
│       → { allowed: true, next: { ... } }          │
│                                                    │
│  4. awaiting_approval → 外部通知（Slack / GitHub） │
│     承認後: workflow-runner next                   │
│                                                    │
│  5. completed まで 2–4 をループ                    │
└────────────────────────────────────────────────────┘
```

#### 実装候補の比較

| アプローチ | メリット | デメリット | 優先度 |
|-----------|---------|-----------|--------|
| A: Claude Code Channels 利用 | セッション管理が Claude Code 側に委譲できる | Research Preview 段階・API 未確定 | 調査優先 |
| B: `claude -p`（非インタラクティブ） | 安定・既存ツールで完結 | セッション状態の引き継ぎが手動 | フォールバック |
| C: Anthropic Messages API 直接呼び出し | AI ツール非依存・完全制御可能 | コンテキスト管理・ツール定義を自前で実装する必要がある | 汎用化時 |

#### フェーズ分割

##### `claude -p` ベースのプロトタイプ

- 外部コントローラーを Rust バイナリ（または shell スクリプト）として実装
- `claude -p "<prompt>"` でタスクを実行し、完了後に `workflow-runner complete` を呼ぶ
- ログ・可観測性（JSON ログ出力、実行トレース）の基盤を作る
- `--dry-run` フラグで実際の AI 呼び出しをスキップして制御フローを検証できるようにする

##### Claude Code Channels 統合

- Channels の仕様が安定したタイミングで Channels ベースに移行
- セッションの起動・停止・出力の受け取りを Channels API 経由で制御
- 並列ステップの並列セッション起動を実現

##### 通知・承認統合

- `awaiting_approval` 時に外部サービスへ通知（Slack Webhook / GitHub PR コメント）
- 外部から `workflow-runner next`（承認）または `workflow-runner reject`（却下）を呼ぶ webhook エンドポイント

#### 完了基準

- Claude Code セッション外からワークフローをエンドツーエンドで実行できる
- `cargo test` が全て通過する
- ドキュメント（ARCHITECTURE.md / README.md）が更新されている

---

## 依存関係グラフ

```
現在地（v0.0.1）
    │
    ├──▶ v0.0.2: ワークフローの視覚化（ダッシュボード作成） （独立して着手可能）
    │
    └──▶ v0.0.3: 自律駆動型ワークフロー
              │
              ├──▶ Item 1: MCP サーバー化        （独立して着手可能）
              │
              └──▶ Item 2: 外部プロセス自律制御   （独立して着手可能）
                        ├──  claude -p ベース      🔲 未着手
                        ├──  Channels 統合         🔲 Channels 仕様確定後
                        └──  通知・承認統合        🔲 完了後
```

v0.0.2（ダッシュボード可視化）は v0.0.3 のいずれの Item とも依存関係がない。
既存の `.workflow/workflow.db` を読むだけで完結するため、優先して着手しても問題ない。

Item 1 と Item 2 に依存関係はない。
ただし **Item 1（MCP サーバー化）が完了すると Item 2 の AI ツール呼び出しが MCP 経由に一本化できる** ため、
先に Item 1 を実装することで Item 2 の外部コントローラーが簡潔になる。

---

## 参考リンク

- [Model Context Protocol 仕様](https://modelcontextprotocol.io/specification)
- [Claude Code – MCP サーバー設定](https://docs.anthropic.com/ja/docs/claude-code/mcp)
- [Claude Code – Research Preview: Channels](https://docs.anthropic.com/ja/docs/claude-code/channels)（要確認）
- [ARCHITECTURE.md](./ARCHITECTURE.md) — 現在のアーキテクチャ詳細
