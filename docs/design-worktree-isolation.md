# 設計書：`isolation: "worktree"` による Sub-Agent 隔離

**ステータス**: 実装済み  
**日付**: 2026-05-24  
**関連問題**: Sub-agent が `workflow-runner complete` を自律実行し、オーケストレーターの制御を迂回する問題

---

## 問題の定義

workflow-runner スキルが `Agent` ツールで sub-agent を spawn して作業（実装・テストなど）を委譲すると、sub-agent は以下のことができてしまう：

1. ワークフローランナースキルの手順を含む会話コンテキスト全体を引き継ぐ
2. `Bash` ツールに無制限にアクセスし、`./target/debug/workflow-runner` を呼び出せる
3. オーケストレーターの承認なしに `report` / `complete` / `next` を独立して実行し、ワークフロー状態を進められる

### 観測されたインシデント

`feature` ワークフローのシミュレーションテスト中、`implement` タスクの sub-agent が：

- 依頼されたファイルを作成した ✅
- `make test` を実行した ✅
- **`workflow-runner report` と `workflow-runner complete implement` を自律的に呼び出した** ❌

これにより以下の問題が発生した：

- オーケストレーターがその後 `workflow-runner report` を呼んだとき `"no workflow in progress"` で失敗
- オーケストレーターが明示的に `complete` を呼ばずにワークフロー状態が進んだ
- `approval: true` のゲートも同様の手口で迂回される可能性がある

---

## 根本原因分析

### 原因①：コンテキストブリード

```
会話コンテキスト（同一セッション内の全エージェントから参照可能）
├── workflow-runner スキルの全手順（SKILL.md の内容）
│   ├── "report → complete の手順"
│   └── "## ワークフロー完了：{name}" テンプレート
├── 実行中の workflow_id: 4cb86826-...
└── 現在実行中の task_id
```

`Agent` ツールで spawn された sub-agent は親の会話コンテキストウィンドウを共有する。  
そのため workflow-runner のプロトコルを「知っている」状態で起動し、そのまま実行できてしまう。

### 原因②：ツールアクセスの無制限性

Sub-agent はオーケストレーターと同じツール（`Bash` を含む）にアクセスできる。  
`Bash` を使えば以下を実行できる：

- `./target/debug/workflow-runner complete <task-id>`
- オーケストレーターが実行可能な任意のシェルコマンド

### 原因③：過補完バイアス（over-completion bias）

Claude はタスクを完了させることに最適化されている。Sub-agent が作業を終えたとき、  
コンテキストに workflow-runner のプロトコルが見えていると、`report → complete` を  
「自然な次のステップ」として実行してしまう。

---

## 提案ソリューション：Worktree 隔離による二層アーキテクチャ

### なぜ Git Worktree が構造的な保護を提供できるか

各ファイル・ディレクトリの git 管理状況：

```
ファイル／ディレクトリ          git 管理？   worktree に存在？
─────────────────────────── ──────────── ─────────────────
.workflow/config.yml        ✅ あり        ✅ あり
src/                        ✅ あり        ✅ あり
tests/                      ✅ あり        ✅ あり
Cargo.toml, Cargo.lock      ✅ あり        ✅ あり
─────────────────────────── ──────────── ─────────────────
/target/                    ❌ .gitignore  ❌ 存在しない
.workflow/workflow.db        ❌ .gitignore  ❌ 存在しない
.workflow/GATE_ACTIVE        ❌ .gitignore  ❌ 存在しない
tmp/                        ❌ .gitignore  ❌ 存在しない
```

### 攻撃経路の排除

| 攻撃経路 | Worktree なし | Worktree あり |
|---------|-------------|--------------|
| `./target/debug/workflow-runner complete` | ✅ 実行可能 | ❌ ファイルが存在しない |
| `workflow-runner complete`（PATH 経由） | ❌ PATH に未登録（現状） | ❌ PATH に未登録 |
| `cargo build && ./target/debug/workflow-runner complete` | ✅ 実行可能 | ⚠️ バイナリの再ビルドは成功するが、`workflow.db` が不在なので `"no workflow in progress"` |

**二重の構造的防御：**

1. バイナリが不在（`target/` は .gitignore 対象）
2. 状態 DB が不在（`workflow.db` は .gitignore 対象）— バイナリを再ビルドしても操作できる状態がない

---

## アーキテクチャ

```
┌──────────────────────────────────────────────────────────────┐
│  オーケストレーター（メインリポジトリ）                          │
│  パス: /workspaces/cyberdog/aidd-workflow/                   │
│                                                              │
│  保持するもの:                                                 │
│  ├── .workflow/workflow.db   ← ワークフロー状態の唯一の正       │
│  ├── ./target/debug/workflow-runner  ← バイナリ               │
│  └── Tasks API の状態                                         │
│                                                              │
│  実行する操作: start → TaskCreate → [spawn] → report → complete│
└─────────────────────────────┬────────────────────────────────┘
                              │ isolation: "worktree" で Agent を spawn
               ┌──────────────┴──────────────┐
               ▼                             ▼
┌──────────────────────┐    ┌──────────────────────────┐
│  作業エージェント      │    │  品質チェックエージェント   │
│  （implement タスク）  │    │  （run-test / run-lint）   │
│                      │    │                          │
│  パス: /tmp/wt-*/    │    │  パス: /tmp/wt-*/         │
│  あり: src/, tests/  │    │  あり: src/, tests/       │
│  あり: Cargo.toml    │    │  あり: Cargo.toml         │
│                      │    │                          │
│  ❌ workflow.db       │    │  ❌ workflow.db            │
│  ❌ バイナリ          │    │  ❌ バイナリ               │
│                      │    │                          │
│  できること: コードを書く│    │  できること: コード読み取り │
│  できないこと:         │    │  make 実行               │
│    ワークフロー進行    │    │  できないこと:             │
│                      │    │    ワークフロー進行         │
└──────────┬───────────┘    └──────────────────────────┘
           │ 変更は worktree に残る（ファイル変更があれば自動削除されない）
           ▼
┌───────────────────────────────┐
│  オーケストレーターが変更を回収  │
│  git diff worktree..main      │
│  → ゲートチェック → マージ      │
└───────────────────────────────┘
```

---

## 必要な変更点

### 1. エージェント定義ファイルの新規作成

`.claude/agents/` ディレクトリにエージェント定義ファイルを作成する。  
これらは `.workflow/config.yml` の `agents:` ブロックから参照される。

**`.claude/agents/implement.md`**
```markdown
---
name: implement
description: コード実装専用エージェント。workflow-runner の操作は一切行わない。
---

# 実装エージェント

**コードの実装と品質確認のみ** を担当します。

## 制約（厳守）
- `report`・`complete`・`next`・`reject` を呼ばないこと
- ワークフロー状態はオーケストレーターが排他的に管理する
- 実装が完了したら結果を報告して終了すること

## 作業手順
1. 設計ドキュメントを読む
2. コードを実装する
3. `make test` を実行する
4. `make lint` を実行する
5. 結果を報告して終了
```

**`.claude/agents/run-test.md`**
```markdown
---
name: run-test
description: テスト実行専用エージェント。コードへの読み取り専用アクセス。workflow-runner の操作は行わない。
---

# テストエージェント

テストを実行して結果を報告するだけです。

## 制約
- ソースファイルを変更しないこと
- `make test` を実行して全出力を報告すること
```

**`.claude/agents/run-lint.md`**
```markdown
---
name: run-lint
description: lint 実行専用エージェント。コードへの読み取り専用アクセス。workflow-runner の操作は行わない。
---

# lint エージェント

lint を実行して結果を報告するだけです。

## 制約
- ソースファイルを変更しないこと
- `make lint` を実行して全出力を報告すること
```

### 2. SKILL.md の更新

「タスクの dispatch」セクションに以下を追加する：

```markdown
### Agent ツールでの dispatch（必須設定）

作業エージェントを spawn する場合は **必ず** `isolation: "worktree"` を指定する：

\```
Agent(
  isolation: "worktree",   # ← 必須
  prompt: "...\n\n⛔ 禁止: workflow-runner / report / complete / next コマンドを呼ばないこと。"
)
\```

**`isolation: "worktree"` が提供する保護:**
- `./target/debug/workflow-runner` が不在（target/ は .gitignore 対象）
- `.workflow/workflow.db` が不在（状態 DB にアクセス不可）
- `cargo build` でバイナリを再ビルドしても、DB がないので状態変更不可

**prompt に必ず含める禁止指示（末尾に付加）:**
\```
⛔ 禁止：workflow-runner / report / complete / next / reject コマンドを一切呼ばないこと。
   ワークフロー状態はオーケストレーターが管理する。
   実装・テスト・lint の結果のみを報告して終了すること。
\```
```

### 3. `settings.json` のフック設定修正

現在のフックコマンドは PATH に存在しない `workflow-runner` を呼んでおり、  
exit 127 で失敗していて保護として機能していない。

**現状（機能していない）：**
```json
{ "command": "workflow-runner hook pre-bash" }
```

**提案（グレースフルフォールバック）：**
```json
{ "command": "command -v workflow-runner >/dev/null 2>&1 && workflow-runner hook pre-bash || exit 0" }
```

これにより：
- `workflow-runner` が PATH にある場合（本番インストール済み）：フックが正常に動作する
- PATH にない場合（開発環境、worktree）：グレースフルにスキップする（exit 0）
- Worktree の sub-agent：フックがスキップされるので Edit/Write/Bash を自由に使える

**注意**：本来の強制ゲートはオーケストレーターが呼ぶ `workflow-runner complete` である。  
フックはあくまで二次的な防御層であり、主要な制御ポイントではない。

---

## 変更の回収フロー

`isolation: "worktree"` を使った sub-agent がコードを書いた場合、変更は worktree に残る  
（ファイルが変更されたため自動削除されない）。オーケストレーターはこれを以下の方法で回収する：

### Worktree コミット + Cherry-pick

```bash
# Sub-agent が worktree でコミットする（オーケストレーターが指示）
cd /tmp/claude-worktree-abc && git commit -m "implement: ..."
# オーケストレーターがゲートチェック後に cherry-pick する
git cherry-pick <commit-hash>
```

---

## 残存リスク

| リスク | 発生可能性 | 対策 |
|--------|-----------|------|
| Sub-agent が `cargo install` で workflow-runner を PATH にインストールする | 低 | プロンプトレベルの禁止指示 |
| コンテキストブリード（エージェントがスキル手順を読める） | 高 | プロンプトレベルの禁止指示 + worktree（構造的対策） |
| 将来 workflow-runner がシステムワイドにインストールされる | 中 | DB の隔離を維持する（DB は常にメイン worktree にのみ存在） |
| Sub-agent が `git stash pop` でメイン worktree のファイルにアクセスする | 非常に低 | Worktree をまたいだ git アクセスは不可 |

---

## 決定ログ

- **2026-05-24**: シミュレーション中に sub-agent がワークフローステップを自律完了するのを観測し、設計を作成
- **2026-05-24**: 設計に基づいて実装を完了
  - `.claude/agents/implement.md` / `run-test.md` / `run-lint.md` を新規作成
  - `SKILL.md` に `isolation: "worktree"` 必須化セクションを追加
  - `settings.json` のフックをグレースフルフォールバック方式に修正

---

## 関連ファイル

- `.workflow/config.yml` — ワークフロー定義
- `.claude/skills/workflow-runner/SKILL.md` — オーケストレータースキルの手順
- `tmp/design-simulation.md` — シミュレーションテストの設計（一時ファイル）
