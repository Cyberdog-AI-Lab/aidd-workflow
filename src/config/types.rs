use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Config {
    pub commands: HashMap<String, String>,
    pub workflows: HashMap<String, Workflow>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Workflow {
    pub name: String,
    pub description: Option<String>,
    pub steps: Vec<Step>,
}

/// ワークフローの1ステップ。
/// `actions` か `parallel` のどちらか一方を持つ（両方は不可）。
/// どちらも持たない場合は手動ステップ（Claude が description に従って作業する）。
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Step {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    #[serde(default)]
    pub actions: Vec<Action>,
    pub parallel: Option<Vec<SubStep>>,
    pub checklist_key: Option<String>,
    #[serde(default)]
    pub requires: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct SubStep {
    pub id: String,
    pub name: Option<String>,
    pub description: Option<String>,
    #[serde(default)]
    pub actions: Vec<Action>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Action {
    Run {
        command: String,
        /// true のとき、このアクションの実行記録が complete の必須条件になる
        #[serde(default)]
        gate: bool,
    },
    Agent {
        prompt: String,
        /// true のとき他のアクションと並列実行してよい
        #[serde(default)]
        background: bool,
    },
    Skill {
        skill: String,
        #[serde(default)]
        args: Vec<String>,
    },
    Workflow {
        workflow: String,
        #[serde(default)]
        inputs: HashMap<String, String>,
    },
}
