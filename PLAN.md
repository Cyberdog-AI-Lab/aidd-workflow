# aidd-workflow 次世代アーキテクチャ計画

## 1. 背景と目的

### 現行設計の課題

| 課題 | 説明 |
|------|------|
| 非決定論的な実行 | ステップの「何をするか」を Claude が description から解釈するため、同じ config でも挙動が変わる |
| Bash フックの脆弱性 | gate ロジックがシェルスクリプトに散在。Python インライン埋め込み、文字列パースに依存 |
| シングルゲート | `gate` は `test` コマンド一種類のみ想定。任意のコマンドやエージェントをゲートにできない |
| 直列実行のみ | 並列化の仕組みがなく、独立したステップも順番待ちになる |
| Claude Code 固有 | Tasks API・Hooks・Skills はすべて Claude Code 専用。他の AI ツールへの移植経路がない |

### 目標

1. **決定論的制御**: `config.yml` に書かれた内容が機械的に実行される。Claude の解釈に依存しない
2. **任意アクション**: シェルコマンド・サブエージェント・スキル・別ワークフロー を統一的に記述できる
3. **並列実行**: 依存関係のないステップを DAG として評価し、並走させる
4. **拡張性**: 現在は Claude Code をターゲットにするが、アダプター層で他の AI ツールにも対応できる

---

## 2. 新アーキテクチャ概要

```
┌─────────────────────────────────────────────────────────┐
│                    AI ツール層                            │
│   Claude Code Skill       Cursor Extension   Generic     │
│   (SKILL.md が薄い        (将来対応)          API Client  │
│    ラッパーになる)                            (将来対応)  │
└────────────┬────────────────────────────────────────────┘
             │ CLI 呼び出し（JSON 入出力）
┌────────────▼────────────────────────────────────────────┐
│              workflow-runner（Rust バイナリ）             │
│                                                          │
│  ┌─────────────┐  ┌──────────────┐  ┌────────────────┐  │
│  │ Config層     │  │ Engine層     │  │ Adapter層      │  │
│  │ YAML パース  │  │ DAG 評価     │  │ claude-code    │  │
│  │ スキーマ検証 │  │ State 管理   │  │ cursor (将来)  │  │
│  │ 型安全な定義 │  │ Action Disp. │  │ standalone     │  │
│  └─────────────┘  └──────────────┘  └────────────────┘  │
└────────────┬────────────────────────────────────────────┘
             │ ファイル I/O
┌────────────▼────────────────────────────────────────────┐
│                   状態層（ファイル）                       │
│   .workflow/config.yml    .workflow/state.json           │
│   .workflow/checklist.md  .workflow/workflow.schema.json │
└─────────────────────────────────────────────────────────┘
```

**設計原則**: Rust バイナリが「何をすべきか」を決定し、AI ツールが「どう実行するか」を担う。
両者は JSON を介した CLI プロトコルで通信する。

---

## 3. workflow-runner CLI 設計

### コマンド体系

```
workflow-runner [--adapter <name>] <command> [options]

コマンド:
  start <workflow>       ワークフローを開始し、最初のアクション群を JSON で返す
  next                   現在の状態から次に実行すべきアクション群を JSON で返す
  report                 アクションの実行結果を記録する（stdin: JSON）
  complete <step-id>     ステップ完了を試みる（ゲートチェック付き）
  resume                 中断ワークフローの再開情報を JSON で返す
  status                 現在の実行状態を JSON で返す
  validate               config.yml を検証して結果を返す
  list                   利用可能なワークフロー一覧を返す
  hook <event-type>      フックイベントを処理する（stdin: フック JSON、Claude Code 専用）
```

### 入出力プロトコル（JSON）

```jsonc
// workflow-runner start bug-fix の出力例
{
  "session_id": "uuid-v4",
  "workflow": "bug-fix",
  "status": "started",
  "actions": [
    {
      "step_id": "reproduce",
      "action_index": 0,
      "type": "agent",
      "prompt": "バグを再現し、再現手順を checklist.md に記録してください",
      "background": false
    }
  ],
  "state_path": ".workflow/state.json"
}

// workflow-runner report の stdin 例
{
  "session_id": "uuid-v4",
  "step_id": "test",
  "action_index": 0,
  "type": "run",
  "exit_code": 0,
  "stdout": "4 passed, 0 failed",
  "stderr": ""
}

// workflow-runner complete test の出力例（ゲート通過）
{
  "allowed": true,
  "next_actions": [...]
}

// workflow-runner complete test の出力例（ゲート失敗）
{
  "allowed": false,
  "reason": "gate 'test' が未実行です。先に make test を実行してください"
}
```

---

## 4. 拡張 config.yml スキーマ

### アクション型の追加

```yaml
# .workflow/config.yml

commands:
  test: make test
  lint: make lint
  build: make build
  deploy: make deploy

workflows:
  release:
    name: リリースフロー
    description: 設計から本番デプロイまでの完走フロー
    steps:

      # --- 直列ステップ（agent アクション）---
      - id: design
        name: 設計確認
        checklist_key: design
        actions:
          - type: agent
            prompt: "実装方針・影響範囲・インターフェースを整理して checklist.md に記録してください"

      - id: implement
        name: 実装
        requires: [design]
        actions:
          - type: agent
            prompt: "設計に従って実装してください"

      # --- 並列ステップ ---
      - id: quality-check
        name: 品質チェック（並列）
        requires: [implement]
        parallel:
          - id: run-test
            actions:
              - type: run
                command: "{{commands.test}}"
                gate: true            # 実行記録が complete の必須条件
          - id: run-lint
            actions:
              - type: run
                command: "{{commands.lint}}"
          - id: security
            actions:
              - type: skill
                skill: security-review

      # --- 別ワークフローをネスト呼び出し ---
      - id: staging
        name: ステージングデプロイ
        requires: [quality-check]
        actions:
          - type: workflow
            workflow: deploy
            inputs:
              env: staging

      - id: complete
        name: 完了
        requires: [design, quality-check, staging]
        checklist_key: release-summary
```

### アクション型の定義

| type | 動作 | 主要フィールド |
|------|------|--------------|
| `run` | シェルコマンド実行 | `command`, `gate: bool` |
| `agent` | サブエージェント起動 | `prompt`, `background: bool` |
| `skill` | スキル呼び出し | `skill`, `args: []` |
| `workflow` | 別ワークフローをネスト実行 | `workflow`, `inputs: {}` |

### テンプレート変数

| 変数 | 解決先 |
|------|-------|
| `{{commands.test}}` | `commands.test` の値 |
| `{{steps.id.output}}` | 指定ステップの実行出力 |
| `{{inputs.key}}` | ワークフロー呼び出し時の入力値 |

---

## 5. Rust 実装設計

### ディレクトリ構成

```
aidd-workflow/
├── PLAN.md
├── Cargo.toml
├── Cargo.lock
├── src/
│   ├── main.rs                  ← CLI エントリポイント（clap）
│   │
│   ├── config/
│   │   ├── mod.rs
│   │   ├── types.rs             ← Workflow, Step, Action 等の型定義
│   │   ├── loader.rs            ← YAML 読み込み・serde_yaml パース
│   │   └── validator.rs         ← JSON Schema バリデーション（jsonschema クレート）
│   │
│   ├── engine/
│   │   ├── mod.rs
│   │   ├── dag.rs               ← requires を解析し実行可能ステップを算出
│   │   ├── state.rs             ← state.json の読み書き・ステップ状態管理
│   │   ├── executor.rs          ← next_actions の構築・action dispatch ロジック
│   │   └── gate.rs              ← gate 条件の評価（state を参照）
│   │
│   ├── adapters/
│   │   ├── mod.rs
│   │   ├── trait.rs             ← AiToolAdapter トレイト定義
│   │   ├── claude_code/
│   │   │   ├── mod.rs
│   │   │   ├── hook_parser.rs   ← Claude Code フック JSON のパース
│   │   │   └── output.rs        ← gate block 決定・checklist 書き込み
│   │   └── standalone/
│   │       ├── mod.rs
│   │       └── runner.rs        ← AI ツールなしで run アクションを直接実行
│   │
│   └── protocol/
│       ├── mod.rs
│       ├── input.rs             ← CLI 入力・stdin JSON の型定義
│       └── output.rs            ← CLI 出力 JSON の型定義（シリアライズ）
│
├── .workflow/
│   ├── config.yml
│   ├── workflow.schema.json     ← 拡張スキーマ（action 型を追加）
│   ├── state.json               ← Rust が管理（旧 GATE_ACTIVE は廃止）
│   └── checklist.md
│
└── .claude/
    ├── skills/
    │   ├── workflow-orchestrator/
    │   │   └── SKILL.md         ← 薄いラッパー（workflow-runner を呼ぶだけ）
    │   └── workflow-create/
    │       └── SKILL.md
    └── settings.json            ← フックが workflow-runner hook を呼ぶ
```

### 主要クレート

| クレート | 用途 |
|---------|------|
| `clap` | CLI パース |
| `serde` / `serde_json` / `serde_yaml` | シリアライズ |
| `jsonschema` | config.yml のスキーマ検証 |
| `petgraph` | DAG 構築・トポロジカルソート |
| `uuid` | セッション ID 生成 |
| `chrono` | タイムスタンプ |
| `anyhow` | エラーハンドリング |

### AiToolAdapter トレイト

```rust
pub trait AiToolAdapter {
    /// アダプター名（"claude-code", "cursor", "standalone" など）
    fn name(&self) -> &str;

    /// アダプターがサポートする capability
    fn capabilities(&self) -> Capabilities;

    /// フックイベントを解析して ActionReport を返す
    fn parse_hook_event(&self, input: &str) -> anyhow::Result<HookEvent>;

    /// gate ブロック時の出力フォーマット
    fn format_gate_block(&self, reason: &str) -> String;

    /// checklist.md への記録フォーマット
    fn format_checklist_entry(&self, event: &HookEvent) -> Option<String>;
}

pub struct Capabilities {
    pub tasks_api: bool,    // Tasks の作成・更新
    pub sub_agents: bool,   // background agent 起動
    pub skills: bool,       // スキル呼び出し
    pub hooks: bool,        // フック統合
}
```

---

## 6. 実行モデル（DAG + 並列）

### 実行例：quality-check 並列ステップ

```
state.json の状態:
  implement: completed
  quality-check: in_progress
    ├── run-test:  pending
    ├── run-lint:  pending
    └── security:  pending

workflow-runner next の出力:
  actions: [
    { step: "quality-check/run-test",  type: "run",   command: "make test"        },
    { step: "quality-check/run-lint",  type: "run",   command: "make lint"        },
    { step: "quality-check/security",  type: "skill", skill: "security-review"    }
  ]
  // 3つを同時に実行してよい

// run-test 完了を report
workflow-runner report < '{"step_id":"quality-check/run-test","exit_code":0,...}'

// run-lint 完了を report  
workflow-runner report < '{"step_id":"quality-check/run-lint","exit_code":0,...}'

// security 完了を report
workflow-runner report < '{"step_id":"quality-check/security","exit_code":0,...}'

// 全サブステップ完了 → quality-check を complete
workflow-runner complete quality-check
// gate チェック: run-test に gate:true → state に実行記録あり → allowed: true
```

### DAG 評価アルゴリズム

1. `requires` を辺としてグラフを構築（petgraph の `DiGraph`）
2. トポロジカルソートで実行順序を決定
3. 各ステップについて「全 requires が completed か」を評価
4. 満たしているステップをすべて「実行可能」とし、`next_actions` に含める

---

## 7. フック簡素化

Rust がゲートロジックを持つため、フックは単純なシェルスクリプトになる。

### 現行（bash スクリプト、約 50 行）

複雑なインライン Python、GATE_ACTIVE フラグ管理、独自の文字列パース

### 新実装（5 行以下）

```bash
# post-bash.sh
#!/bin/bash
cat | workflow-runner --adapter claude-code hook post-bash

# pre-taskupdate.sh
#!/bin/bash
RESULT=$(cat | workflow-runner --adapter claude-code hook pre-taskupdate)
echo "$RESULT"
# workflow-runner が {"decision":"block",...} を返せばそのまま Claude Code に渡る
```

---

## 8. SKILL.md の簡素化

Rust バイナリが判断ロジックを持つため、スキルは「ツールブリッジ」になる。

```markdown
# Workflow Orchestrator スキル（v2）

## 実行手順

1. `workflow-runner start <workflow>` を Bash で実行（引数なし時は `workflow-runner resume` または `workflow-runner list`）
2. JSON の `actions` 配列を順に処理する：
   - `type: run`      → Bash ツールで `command` を実行
   - `type: agent`    → Agent ツールで `prompt` を実行（`background: true` なら並列）
   - `type: skill`    → Skill ツールで `skill` を呼び出す
   - `type: workflow` → `workflow-runner start <workflow>` を再帰的に実行
3. 各アクション完了後、`workflow-runner report` に結果を stdin で渡す
4. `type: run` かつ `gate: true` のアクションは必ず報告する（フックが自動記録）
5. ステップの全アクション完了後、`workflow-runner complete <step-id>` を実行
6. `allowed: false` が返ったらユーザーに `reason` を伝えてブロック
7. `actions` が空になるまで繰り返す
```

---

## 9. アダプター拡張ロードマップ

| フェーズ | アダプター | 概要 |
|---------|-----------|------|
| Phase 1 | `claude-code` | 現行の Hooks + Tasks API + Skills を Rust で再実装 |
| Phase 2 | `standalone` | AI ツールなし。`run` アクションを直接実行。`agent` は Anthropic API 呼び出し |
| Phase 3 | `cursor` | Cursor の拡張機構に対応（フック形式が異なる） |
| Phase 4 | `generic` | OpenAPI スキーマで任意の AI ツールに対応するアダプター設定ファイル方式 |

---

## 10. フェーズ別実装計画

### Phase 1：Rust コア（Claude Code ターゲット）

- [x] `Cargo.toml` 作成・クレート依存定義
- [x] `config/types.rs`：Workflow, Step, Action の型定義（serde）
- [x] `config/loader.rs`：YAML 読み込み・スキーマ検証
- [x] `engine/state.rs`：state.json の読み書き
- [x] `engine/dag.rs`：depends 解析・実行可能ステップ算出
- [x] `engine/gate.rs`：gate 条件チェック
- [x] `engine/executor.rs`：next_actions 構築
- [x] `adapters/claude_code/`：フック JSON パース・出力フォーマット
- [x] `main.rs`：CLI（start / next / report / complete / hook / validate / list）
- [x] `workflow.schema.json` 拡張：action 型・parallel ブロック追加
- [x] `.claude/hooks/` 簡素化（workflow-runner 呼び出しのみ）
- [x] `.claude/skills/workflow-orchestrator/SKILL.md` v2 改訂

### Phase 2：並列実行

- [x] `engine/dag.rs` 拡張：parallel ブロックのサブグラフ評価
- [x] `protocol/output.rs`：並列アクション群の出力形式
- [x] SKILL.md：`background: true` アクションの並列実行手順追加

### Phase 3：standalone アダプター

- [x] `adapters/standalone/runner.rs`：`run` アクションを直接 `std::process::Command` で実行
- [x] `agent` アクション：Anthropic API 呼び出し（`reqwest` + blocking）
- [x] `exec-step <step-id>` CLI サブコマンド：run/agent アクションを自律実行し report + complete を自動処理

### Phase 4：スキーマ・CLI 安定化

- [ ] `workflow-runner validate` でスキーマエラーを人間可読なメッセージで表示
- [ ] `workflow-runner status --format table` でターミナル表示
- [ ] バイナリ配布（GitHub Releases + install スクリプト）

---

## 11. 移行戦略

現行の bash ベース設計から段階的に移行できる。

```
現行                              移行後
────────────────────────────────────────────────────
post-bash-capture-test.sh     →  workflow-runner hook post-bash
pre-taskupdate-gate.sh        →  workflow-runner hook pre-taskupdate
post-edit-validate-config.sh  →  workflow-runner hook post-edit
GATE_ACTIVE フラグ            →  state.json の gate_recorded フィールド
checklist.md 手動記録         →  workflow-runner report が自動追記
SKILL.md（複雑な手順）        →  SKILL.md（action type dispatch のみ）
```

Phase 1 完了後、bash フックをそのまま残しつつ内部を `workflow-runner` 呼び出しに差し替えることで、Claude Code 側の設定変更なしに移行できる。
