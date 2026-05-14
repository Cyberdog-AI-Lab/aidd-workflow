use crate::config::types::{Config, Workflow, Action};
use crate::engine::dag;
use crate::engine::state::WorkflowState;
use crate::protocol::output::{ActionItem, ResolvedAction, WorkflowOutput, FlowStatus};

pub fn build_next(wf: &Workflow, state: &WorkflowState, config: &Config) -> WorkflowOutput {
    let is_complete = dag::is_workflow_complete(wf, state);
    if is_complete {
        return WorkflowOutput {
            session_id: state.session_id.clone(),
            workflow: state.workflow.clone(),
            status: FlowStatus::Completed,
            actions: vec![],
        };
    }

    let items = dag::executable_items(wf, state);
    let mut actions = Vec::new();

    for item_id in &items {
        if let Some(sep) = item_id.find('/') {
            // 並列サブステップ
            let parent_id = &item_id[..sep];
            let sub_id = &item_id[sep + 1..];
            let parent_step = wf.steps.iter().find(|s| s.id == parent_id).unwrap();
            let parallel = parent_step.parallel.as_deref().unwrap_or(&[]);
            if let Some(sub) = parallel.iter().find(|s| s.id == sub_id) {
                let step_name = sub.name.as_deref().unwrap_or(sub_id);
                if sub.actions.is_empty() {
                    actions.push(ActionItem {
                        step_id: item_id.clone(),
                        action_index: 0,
                        step_name: step_name.to_string(),
                        action: ResolvedAction::Manual {
                            description: sub.description.clone().unwrap_or_default(),
                            checklist_key: None,
                        },
                    });
                } else {
                    for (i, action) in sub.actions.iter().enumerate() {
                        actions.push(ActionItem {
                            step_id: item_id.clone(),
                            action_index: i,
                            step_name: step_name.to_string(),
                            action: resolve(action, config),
                        });
                    }
                }
            }
        } else {
            // 通常ステップ
            let step = wf.steps.iter().find(|s| s.id == *item_id).unwrap();
            if step.actions.is_empty() && step.parallel.is_none() {
                actions.push(ActionItem {
                    step_id: item_id.clone(),
                    action_index: 0,
                    step_name: step.name.clone(),
                    action: ResolvedAction::Manual {
                        description: step.description.clone().unwrap_or_default(),
                        checklist_key: step.checklist_key.clone(),
                    },
                });
            } else {
                for (i, action) in step.actions.iter().enumerate() {
                    actions.push(ActionItem {
                        step_id: item_id.clone(),
                        action_index: i,
                        step_name: step.name.clone(),
                        action: resolve(action, config),
                    });
                }
            }
        }
    }

    WorkflowOutput {
        session_id: state.session_id.clone(),
        workflow: state.workflow.clone(),
        status: if actions.is_empty() { FlowStatus::Blocked } else { FlowStatus::InProgress },
        actions,
    }
}

fn resolve(action: &Action, config: &Config) -> ResolvedAction {
    match action {
        Action::Run { command, gate } => ResolvedAction::Run {
            command: resolve_template(command, config),
            gate: *gate,
        },
        Action::Agent { prompt, background } => ResolvedAction::Agent {
            prompt: resolve_template(prompt, config),
            background: *background,
        },
        Action::Skill { skill, args } => ResolvedAction::Skill {
            skill: skill.clone(),
            args: args.clone(),
        },
        Action::Workflow { workflow, inputs } => ResolvedAction::Workflow {
            workflow: workflow.clone(),
            inputs: inputs.clone(),
        },
    }
}

fn resolve_template(s: &str, config: &Config) -> String {
    let mut result = s.to_string();
    for (key, value) in &config.commands {
        result = result.replace(&format!("{{{{commands.{}}}}}", key), value);
    }
    result
}
