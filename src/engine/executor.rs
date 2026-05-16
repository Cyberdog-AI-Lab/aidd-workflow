use crate::config::types::{Action, Config, Workflow};
use crate::engine::dag;
use crate::engine::state::WorkflowState;
use crate::protocol::output::{ActionItem, FlowStatus, ResolvedAction, WorkflowOutput};

pub fn build_next(wf: &Workflow, state: &WorkflowState, config: &Config) -> WorkflowOutput {
    if dag::is_workflow_complete(wf, state) {
        return WorkflowOutput {
            workflow_id: state.workflow_id.clone(),
            workflow: state.workflow.clone(),
            status: FlowStatus::Completed,
            actions: vec![],
        };
    }

    let items = dag::executable_items(wf, state);
    let mut actions = Vec::new();

    for item_id in &items {
        if let Some(sep) = item_id.find('/') {
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
                        parallel: true,
                        pre_commands: vec![],
                        post_commands: vec![],
                        action: ResolvedAction::Manual {
                            description: sub.description.clone().unwrap_or_default(),
                        },
                    });
                } else {
                    for (i, action) in sub.actions.iter().enumerate() {
                        actions.push(ActionItem {
                            step_id: item_id.clone(),
                            action_index: i,
                            step_name: step_name.to_string(),
                            parallel: true,
                            pre_commands: vec![],
                            post_commands: vec![],
                            action: resolve(action, config),
                        });
                    }
                }
            }
        } else {
            let step = wf.steps.iter().find(|s| s.id == *item_id).unwrap();
            if step.actions.is_empty() && step.parallel.is_none() {
                actions.push(ActionItem {
                    step_id: item_id.clone(),
                    action_index: 0,
                    step_name: step.name.clone(),
                    parallel: false,
                    pre_commands: resolve_commands(&step.pre_commands, config),
                    post_commands: resolve_commands(&step.post_commands, config),
                    action: ResolvedAction::Manual {
                        description: step.description.clone().unwrap_or_default(),
                    },
                });
            } else {
                for (i, action) in step.actions.iter().enumerate() {
                    actions.push(ActionItem {
                        step_id: item_id.clone(),
                        action_index: i,
                        step_name: step.name.clone(),
                        parallel: false,
                        pre_commands: if i == 0 {
                            resolve_commands(&step.pre_commands, config)
                        } else {
                            vec![]
                        },
                        post_commands: if i == step.actions.len() - 1 {
                            resolve_commands(&step.post_commands, config)
                        } else {
                            vec![]
                        },
                        action: resolve(action, config),
                    });
                }
            }
        }
    }

    WorkflowOutput {
        workflow_id: state.workflow_id.clone(),
        workflow: state.workflow.clone(),
        status: if actions.is_empty() {
            FlowStatus::Blocked
        } else {
            FlowStatus::InProgress
        },
        actions,
    }
}

fn resolve(action: &Action, config: &Config) -> ResolvedAction {
    match action {
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

fn resolve_commands(cmds: &[String], config: &Config) -> Vec<String> {
    cmds.iter().map(|c| resolve_template(c, config)).collect()
}

pub fn resolve_template(s: &str, config: &Config) -> String {
    let mut result = s.to_string();
    for (key, value) in &config.commands {
        result = result.replace(&format!("{{{{commands.{}}}}}", key), value);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::types::{Action, Config, Step, Workflow};
    use crate::engine::state::{StepStatus, WorkflowState};
    use std::collections::HashMap;

    fn config_with_test_cmd(cmd: &str) -> Config {
        let mut commands = HashMap::new();
        commands.insert("test".to_string(), cmd.to_string());
        Config {
            imports: vec![],
            commands,
            workflows: HashMap::new(),
        }
    }

    fn workflow_with_agent_action() -> Workflow {
        Workflow {
            name: "test".to_string(),
            description: None,
            steps: vec![Step {
                id: "run".to_string(),
                name: "Run".to_string(),
                description: None,
                actions: vec![Action::Agent {
                    prompt: "do the thing".to_string(),
                    background: false,
                }],
                parallel: None,
                requires: vec![],
                pre_commands: vec![],
                post_commands: vec![],
                allow_files: vec![],
                deny: None,
                guards: vec![],
            }],
        }
    }

    fn workflow_manual() -> Workflow {
        Workflow {
            name: "test".to_string(),
            description: None,
            steps: vec![Step {
                id: "design".to_string(),
                name: "Design".to_string(),
                description: Some("Write the design doc".to_string()),
                actions: vec![],
                parallel: None,
                requires: vec![],
                pre_commands: vec![],
                post_commands: vec!["make docs-check".to_string()],
                allow_files: vec![],
                deny: None,
                guards: vec![],
            }],
        }
    }

    #[test]
    fn resolve_template_substitutes_command() {
        let config = config_with_test_cmd("make test");
        let result = resolve_template("{{commands.test}}", &config);
        assert_eq!(result, "make test");
    }

    #[test]
    fn resolve_template_leaves_unknown_key() {
        let config = config_with_test_cmd("make test");
        let result = resolve_template("{{commands.build}}", &config);
        assert_eq!(result, "{{commands.build}}");
    }

    #[test]
    fn build_next_returns_agent_action() {
        let wf = workflow_with_agent_action();
        let config = config_with_test_cmd("make test");
        let state = WorkflowState::new("test", &wf);

        let output = build_next(&wf, &state, &config);
        assert_eq!(output.actions.len(), 1);
        match &output.actions[0].action {
            ResolvedAction::Agent { prompt, .. } => assert_eq!(prompt, "do the thing"),
            _ => panic!("expected Agent"),
        }
    }

    #[test]
    fn build_next_returns_manual_with_post_commands() {
        let wf = workflow_manual();
        let config = config_with_test_cmd("make test");
        let state = WorkflowState::new("test", &wf);

        let output = build_next(&wf, &state, &config);
        assert_eq!(output.actions.len(), 1);
        assert_eq!(output.actions[0].post_commands, vec!["make docs-check"]);
        match &output.actions[0].action {
            ResolvedAction::Manual { description } => {
                assert_eq!(description, "Write the design doc");
            }
            _ => panic!("expected Manual"),
        }
    }

    #[test]
    fn parallel_sub_step_actions_have_parallel_true() {
        use crate::config::types::SubStep;
        let wf = Workflow {
            name: "test".to_string(),
            description: None,
            steps: vec![Step {
                id: "p".to_string(),
                name: "Parallel".to_string(),
                description: None,
                actions: vec![],
                parallel: Some(vec![
                    SubStep {
                        id: "x".to_string(),
                        name: None,
                        description: None,
                        actions: vec![Action::Agent {
                            prompt: "do x".to_string(),
                            background: false,
                        }],
                        requires: vec![],
                    },
                    SubStep {
                        id: "y".to_string(),
                        name: None,
                        description: None,
                        actions: vec![Action::Agent {
                            prompt: "do y".to_string(),
                            background: false,
                        }],
                        requires: vec![],
                    },
                ]),
                requires: vec![],
                pre_commands: vec![],
                post_commands: vec![],
                allow_files: vec![],
                deny: None,
                guards: vec![],
            }],
        };
        let config = config_with_test_cmd("make test");
        let state = WorkflowState::new("test", &wf);

        let output = build_next(&wf, &state, &config);
        assert_eq!(output.actions.len(), 2);
        assert!(output.actions[0].parallel);
        assert!(output.actions[1].parallel);
    }

    #[test]
    fn sequential_step_action_has_parallel_false() {
        let wf = workflow_with_agent_action();
        let config = config_with_test_cmd("make test");
        let state = WorkflowState::new("test", &wf);

        let output = build_next(&wf, &state, &config);
        assert!(!output.actions[0].parallel);
    }

    #[test]
    fn build_next_returns_completed_when_all_done() {
        let wf = workflow_with_agent_action();
        let config = config_with_test_cmd("make test");
        let mut state = WorkflowState::new("test", &wf);
        state.steps.get_mut("run").unwrap().status = StepStatus::Completed;

        let output = build_next(&wf, &state, &config);
        assert!(matches!(output.status, FlowStatus::Completed));
        assert!(output.actions.is_empty());
    }

    #[test]
    fn pre_commands_on_first_action_only() {
        use crate::config::types::Action;
        let wf = Workflow {
            name: "test".to_string(),
            description: None,
            steps: vec![Step {
                id: "s".to_string(),
                name: "S".to_string(),
                description: None,
                actions: vec![
                    Action::Agent {
                        prompt: "first".to_string(),
                        background: false,
                    },
                    Action::Agent {
                        prompt: "second".to_string(),
                        background: false,
                    },
                ],
                parallel: None,
                requires: vec![],
                pre_commands: vec!["cargo check".to_string()],
                post_commands: vec!["cargo test".to_string()],
                allow_files: vec![],
                deny: None,
                guards: vec![],
            }],
        };
        let config = config_with_test_cmd("make test");
        let state = WorkflowState::new("test", &wf);

        let output = build_next(&wf, &state, &config);
        assert_eq!(output.actions.len(), 2);
        assert_eq!(output.actions[0].pre_commands, vec!["cargo check"]);
        assert!(output.actions[0].post_commands.is_empty());
        assert!(output.actions[1].pre_commands.is_empty());
        assert_eq!(output.actions[1].post_commands, vec!["cargo test"]);
    }
}
