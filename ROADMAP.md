# ROADMAP

aidd-workflow の中長期ロードマップ。

---

## 現在地

| フェーズ | 状態 |
|---------|------|
| v0.0.1　コア実装 | ✅ 完了 |
| v0.0.2　自律駆動型ワークフロー | 着手中 |
| v0.0.3　ワークフローの視覚化（ダッシュボード作成） | 🔲 未着手 |
| v0.0.4　リモート通知・承認 | 🔲 未着手 |
| v0.0.5　デーモンのクラッシュリカバリ（既存ワークフローの復元） | 🔲 未着手 |


---

## v0.0.2: 自律駆動型ワークフロー

`workflow-runner` を **外部プロセスとして常駐させ**、Claude Code Channels 経由でタスクを push し、
HTTP コールバックで完了を受け取ることで、人手を介さずワークフローをエンドツーエンドで実行するフェーズ。

### アーキテクチャ概要

```
┌────────────────────────────────────────────────────────────────┐
│  workflow-runner run <workflow>                                  │
│  （常駐外部プロセス / オーケストレーター）                          │
│                                                                │
│  HTTP コールバックサーバー: 127.0.0.1:8789                       │
│  ├─ POST /complete/:task_id   タスク完了受信 → 次タスク dispatch  │
│  ├─ POST /report/:task_id     中間レポート受信（任意）             │
│  ├─ POST /approve             承認（awaiting_approval → active）  │
│  ├─ POST /resume              再開（paused → active、再 dispatch）│
│  ├─ POST /reject/:task_id     タスク却下（approval フロー用）     │
│  └─ POST /pause/:task_id      エージェント起因の一時中断           │
│                                                                │
│  ループ:                                                         │
│    1. config.yml 読み込み、ワークフロー開始（SQLite に記録）        │
│    2. build_next() で実行可能タスクを決定（DAG 評価）              │
│    3. タスク指示 JSON を Channels webhook (8788) に POST         │
│    4. /complete/:task_id のコールバックを待つ                     │
│    5. complete() → 状態更新 → 2 に戻る                          │
│    6. awaiting_approval → /approve または /reject を待つ         │
│    7. paused → /resume を待つ                                   │
│    8. completed → プロセス終了                                   │
└──────────────┬─────────────────────────────────────────────────┘
               │ POST http://127.0.0.1:8788/
               │ { task_id, task, prompt, callback_url, ... }
               ▼
┌────────────────────────────────────────────────────────────────┐
│  channels/webhook.ts（Channels MCP サーバー）                    │
│  - HTTP → MCP notification → Claude Code セッション              │
└──────────────┬─────────────────────────────────────────────────┘
               │ <channel source="webhook"> イベント
               │ { task_id, task, prompt, callback_url, ... }
               ▼
┌────────────────────────────────────────────────────────────────┐
│  Claude Code セッション（常時待機 / ワーカー）                      │
│  - channel 受信 → タスク内容に従って実行（コード編集・テスト等）     │
│  - 完了後 → curl -X POST {callback_url}/complete/{task_id}      │
└────────────────────────────────────────────────────────────────┘
```

**役割分担**:

| コンポーネント | 責務 |
|----------------|------|
| `workflow-runner run` | 状態管理・DAG 評価・タスク dispatch・承認ゲート制御 |
| `channels/webhook.ts` | HTTP → Channels MCP 変換（既存のまま流用） |
| Claude Code | タスクの実行のみ（コード編集・テスト・ドキュメント更新等） |

---

### Item 1: workflow-runner 常駐オーケストレーターモード

#### 目標

`workflow-runner run <workflow>` コマンドを追加し、
ワークフロー全体を外部プロセスとして自律実行できるようにする。

#### 実装内容

1. **`run` サブコマンドの追加** — `src/cmd/run.rs` として実装
   - `workflow-runner run <workflow> [--cwd <path>]`
   - 内部で `start` → `build_next` → dispatch ループを非同期で実行
2. **HTTP コールバックサーバーの組み込み** — `tokio` + `hyper`（または `axum`）
   - `POST /complete/:task_id` — `cmd_complete()` を呼び出し、次タスクを dispatch
   - `POST /report/:task_id` — `cmd_report()` を呼び出して中間状態を記録
   - `POST /next` — `awaiting_approval` の承認（外部ツールや人手から呼べる）
   - `POST /reject/:task_id` — タスクの却下と再 dispatch
3. **Channels webhook への POST** — `reqwest` クレートで `127.0.0.1:8788` に投げる
   - ペイロード: `{ task_id, task, prompt, callback_url, workflow_id }`
   - `callback_url`: `http://127.0.0.1:8789`（コールバックサーバーのベース URL）
4. ~~既存 CLI コマンドは維持~~ — `start` / `next` / `report` / `complete` は廃止
   自律実行モードへの一本化に伴い削除された。`approve` / `resume` / `reject` （常駐デーモンへの
   HTTP クライアント）に置き換わっている。詳細は [ARCHITECTURE.md](./ARCHITECTURE.md) を参照

#### 使用クレート候補

| クレート | 用途 |
|---------|------|
| `tokio` | 非同期ランタイム |
| `axum` | HTTP コールバックサーバー |
| `reqwest` | Channels webhook への HTTP POST |

#### 完了基準

- `workflow-runner run bug-fix` を実行するとワークフローが自律的にエンドツーエンドで完了する
- ~~既存の CLI（`workflow-runner start` / `complete` 等）が引き続き動作する~~（後日削除、上記参照）
- `cargo test` が全て通過する

---

### Item 2: Channels 統合 & Claude Code ワーカー設定

#### 目標

Claude Code が Channels 経由でタスク指示を受け取り、実行後に HTTP コールバックで報告できるようにする。
SKILL.md は廃止し、Claude Code の動作は MCP サーバーの `instructions` フィールドで定義する。

#### 実装内容

1. **タスク指示 JSON 形式の標準化** — `channels/webhook.ts` の instruction を更新
   ```json
   {
     "task_id": "implement",
     "task": "実装する",
     "prompt": "設計書に従って実装してください。...",
     "callback_url": "http://127.0.0.1:8789",
     "workflow_id": "4fd261ba-...",
     "outputs": ["src/**", "tests/**"],
     "deny": { "files": [".env"] }
   }
   ```
2. **Claude Code ワーカー指示の整備** — `channels/webhook.ts` の `instructions` を拡張
   - channel 受信 → `task_id` / `prompt` を取り出してタスクを実行
   - 実行完了後 → `curl -sX POST {callback_url}/complete/{task_id}` でコールバック
   - `outputs` / `deny` は自身の動作を制約する情報として使用
3. **SKILL.md の廃止** — `.claude/skills/workflow-runner/` を削除
   - ワークフロー起動は外部から `workflow-runner run <workflow>` を呼ぶ方式に移行

#### Channels MCP 設定例

```json
// .claude/mcp.json
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

#### 完了基準

- Claude Code セッションを開いた状態で `workflow-runner run bug-fix` を実行すると、
  人手を介さずワークフローがエンドツーエンドで完了する
- `awaiting_approval` タスクで自動停止し、外部から `/approve`（旧 `/next`）または `/reject` を呼べる
- SKILL.md なしでワークフローが動作する
- README.md / ARCHITECTURE.md が更新されている

---

---

## 依存関係グラフ

```
現在地（v0.0.1）
    │
    ├──▶ v0.0.2: 自律駆動型ワークフロー                                  ✅ 完了
    │         │
    │         ├──▶ Item 1: workflow-runner 常駐オーケストレーターモード   ✅ 完了
    │         │
    │         └──▶ Item 2: Channels 統合 & Claude Code ワーカー設定      ✅ 完了
    │                   （Item 1 と並行して着手可能）
    │
    ├──▶ v0.0.3: デーモンのクラッシュリカバリ（既存ワークフローの復元）
    │
    ├──▶ v0.0.4: ワークフローの視覚化（ダッシュボード作成）
    │
    └──▶ v0.0.5: リモート通知・承認 （v0.0.2 完了後）
```

---

## 参考リンク

- [Claude Code – Channels](https://docs.anthropic.com/ja/docs/claude-code/channels)
- [Model Context Protocol 仕様](https://modelcontextprotocol.io/specification)
- [ARCHITECTURE.md](./ARCHITECTURE.md) — 現在のアーキテクチャ詳細

---

## v0.0.3: デーモンのクラッシュリカバリ（既存ワークフローの復元）

### 目標

`workflow-runner serve` が再起動された際、SQLite 上に残っている `active` / `paused` /
`awaiting_approval` な既存ワークフローを、新しいデーモンプロセスの `running`
（`HashMap<workflow_id, RunningWorkflow>`）へ自動復元できるようにする。

### 背景・動機

1デーモンで複数ワークフローを並行実行できるようにした変更（`serve` / `run` / `stop` への分割、
コールバック URL への `workflow_id` 組み込み）で、SQLite 側は元々複数ワークフローの状態を
`workflow_id` 単位で永続化できる設計だったが、HTTP レイヤー（`running` マップ）はプロセスの
インメモリ状態でしかない。そのため、`serve` プロセスが再起動すると、それ以前に `paused` /
`awaiting_approval` になっていたワークフローは SQLite 上に記録が残っていても
`running` マップには存在しなくなり、`/resume/:workflow_id` や `/approve/:workflow_id` が
「未知の workflow_id」としてサイレントに無視されてしまう（`cmd/run.rs` の
`RunEvent::Approve`/`Resume` ハンドラの `running.get(...)` が `None` を返すケース）。

今回の変更ではこの復元ロジックは意図的にスコープ外とした（既存の「プロセスが死ぬとインメモリ状態が
失われる」という制約を単に多ワークフロー化しただけで、悪化させてはいない）。次のフェーズで
明示的に対応する。

### 実装内容（案）

- `serve` 起動時に、当該 `cwd` の `workflow_runs` テーブルから
  `active` / `paused` / `awaiting_approval` な `workflow_id` を全件取得する新規 `store` 関数
  （例: `list_active_workflow_ids(cwd) -> Result<Vec<String>>`）を追加する
- 各 `workflow_id` について `load_state_by_id()` で `WorkflowState` を復元し、
  `state.workflow`（ワークフロー名）から `config.workflows` を引いて `wf: Workflow` を再構築する
- `dispatched: HashSet<task_id>` は、`state.tasks` のうち `status == InProgress` なタスクの
  ID で初期化する（**再 dispatch はしない** — Claude Code 側は既にそのタスクを認識しているはずで、
  再送すると重複指示になる。あくまで `/complete` 等の後続コールバックを正しく受け取れるように
  `running` マップへ登録するだけ）
- 復元中にエラーが起きても `serve` 全体の起動は失敗させない（該当ワークフローだけ復元をスキップし
  stderr にログを出す）方針を検討する

### 完了基準

- `serve` を一度起動してワークフローを `paused`/`awaiting_approval` にした状態でプロセスを
  kill → 再度 `serve` を起動すると、`/resume`/`/approve` が引き続き機能する
- 復元時に再 dispatch が発生しない（Claude Code へタスクが二重送信されない）ことをテストで保証する
- `cargo test` が全て通過する
- README.md / ARCHITECTURE.md が更新されている

---

## v0.0.4: ワークフローの視覚化（ダッシュボード作成）

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
- ブラウザから承認（`workflow-runner approve` 相当）/ 却下（`workflow-runner reject` 相当）を実行

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

## v0.0.5: リモート通知・承認

### 目標

`awaiting_approval` で停止したワークフローを外部サービス（Slack / GitHub）経由でリモートから承認・却下できるようにする。

### 背景・動機

v0.0.2 で実装したコールバックサーバーの `/approve/:workflow_id` / `/reject/:workflow_id/:task_id`
エンドポイントはローカルネットワーク（127.0.0.1）にしか公開されていない。
承認担当者がローカルマシンの前にいない場合でも、Slack や GitHub のインターフェース上から承認操作を行えるようにする。

### 実装内容

- `awaiting_approval` 時に Slack Webhook または GitHub PR コメントへ通知を送信
- 通知メッセージに `/approve/:workflow_id`（承認）/ `/reject/:workflow_id/:task_id`（却下）の URL を含める
- コールバックサーバーのエンドポイントを外部から呼べるようにするためのトンネル設定例（ngrok 等）を docs に追加

### 完了基準

- `awaiting_approval` 状態に入ったとき、Slack または GitHub に通知が届く
- 通知に含まれる承認 URL を踏むとワークフローが再開する
- トンネル設定例が docs に記載されている
- README.md / ARCHITECTURE.md が更新されている