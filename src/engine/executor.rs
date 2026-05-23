use crate::config::types::{Config, Workflow};
use crate::engine::dag;
use crate::engine::state::WorkflowState;
use crate::protocol::output::{FlowStatus, TaskOutput, WorkflowOutput};

pub fn build_next(wf: &Workflow, state: &WorkflowState, config: &Config) -> WorkflowOutput {
    if dag::is_workflow_complete(wf, state) {
        return WorkflowOutput {
            workflow_id: state.workflow_id.clone(),
            workflow: state.workflow.clone(),
            status: FlowStatus::Completed,
            tasks: vec![],
        };
    }

    let items = dag::executable_items(wf, state);
    let mut tasks = Vec::new();

    for item_id in &items {
        let task = match wf.tasks.iter().find(|t| t.id == *item_id) {
            Some(t) => t,
            None => continue,
        };
        tasks.push(TaskOutput {
            task_id: task.id.clone(),
            task: task.task.clone(),
            prompt: task.prompt.as_deref().map(|p| resolve_template(p, config)),
            skills: task.skills.clone(),
            agents: task.agents.clone(),
            outputs: task.outputs.clone(),
            deny: task.deny.clone(),
            approval: task.approval,
        });
    }

    WorkflowOutput {
        workflow_id: state.workflow_id.clone(),
        workflow: state.workflow.clone(),
        status: if tasks.is_empty() {
            FlowStatus::Blocked
        } else {
            FlowStatus::InProgress
        },
        tasks,
    }
}

/// Replaces `{{vars.<key>}}` placeholders with values from config.vars.
/// Unknown keys are left as-is.
pub fn resolve_template(s: &str, config: &Config) -> String {
    let mut result = s.to_string();
    for (key, value) in &config.vars {
        result = result.replace(&format!("{{{{vars.{}}}}}", key), value);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::types::{Config, Task, Workflow};
    use crate::engine::state::{StepStatus, WorkflowState};
    use std::collections::HashMap;

    fn config_with_vars(pairs: &[(&str, &str)]) -> Config {
        let mut vars = HashMap::new();
        for (k, v) in pairs {
            vars.insert(k.to_string(), v.to_string());
        }
        Config {
            imports: vec![],
            vars,
            workflows: HashMap::new(),
        }
    }

    fn workflow_with_prompt(prompt: &str) -> Workflow {
        Workflow {
            name: "test".to_string(),
            description: None,
            tasks: vec![Task {
                id: "run".to_string(),
                prompt: Some(prompt.to_string()),
                ..Task::default()
            }],
        }
    }

    fn workflow_manual() -> Workflow {
        Workflow {
            name: "test".to_string(),
            description: None,
            tasks: vec![Task {
                id: "design".to_string(),
                task: Some("Write the design doc".to_string()),
                ..Task::default()
            }],
        }
    }

    #[test]
    fn resolve_template_substitutes_var() {
        let config = config_with_vars(&[("test", "make test")]);
        let result = resolve_template("{{vars.test}}", &config);
        assert_eq!(result, "make test");
    }

    #[test]
    fn resolve_template_leaves_unknown_key() {
        let config = config_with_vars(&[("test", "make test")]);
        let result = resolve_template("{{vars.build}}", &config);
        assert_eq!(result, "{{vars.build}}");
    }

    #[test]
    fn build_next_returns_task_with_resolved_prompt() {
        let wf = workflow_with_prompt("run {{vars.test}}");
        let config = config_with_vars(&[("test", "make test")]);
        let state = WorkflowState::new("test", &wf);

        let output = build_next(&wf, &state, &config);
        assert_eq!(output.tasks.len(), 1);
        assert_eq!(output.tasks[0].prompt.as_deref(), Some("run make test"));
    }

    #[test]
    fn build_next_returns_manual_task_with_task_name() {
        let wf = workflow_manual();
        let config = config_with_vars(&[]);
        let state = WorkflowState::new("test", &wf);

        let output = build_next(&wf, &state, &config);
        assert_eq!(output.tasks.len(), 1);
        assert_eq!(
            output.tasks[0].task.as_deref(),
            Some("Write the design doc")
        );
        assert!(output.tasks[0].prompt.is_none());
        assert!(output.tasks[0].agents.is_empty());
    }

    #[test]
    fn build_next_returns_agents_list() {
        let wf = Workflow {
            name: "test".to_string(),
            description: None,
            tasks: vec![Task {
                id: "parallel".to_string(),
                agents: vec!["run-test".to_string(), "run-lint".to_string()],
                ..Task::default()
            }],
        };
        let config = config_with_vars(&[]);
        let state = WorkflowState::new("test", &wf);

        let output = build_next(&wf, &state, &config);
        assert_eq!(output.tasks.len(), 1);
        assert_eq!(output.tasks[0].task_id, "parallel");
        assert_eq!(output.tasks[0].agents, vec!["run-test", "run-lint"]);
    }

    #[test]
    fn build_next_returns_completed_when_all_done() {
        let wf = workflow_with_prompt("do the thing");
        let config = config_with_vars(&[]);
        let mut state = WorkflowState::new("test", &wf);
        state.tasks.get_mut("run").unwrap().status = StepStatus::Completed;

        let output = build_next(&wf, &state, &config);
        assert!(matches!(output.status, FlowStatus::Completed));
        assert!(output.tasks.is_empty());
    }

    #[test]
    fn build_next_includes_approval_flag() {
        let wf = Workflow {
            name: "test".to_string(),
            description: None,
            tasks: vec![Task {
                id: "impl".to_string(),
                prompt: Some("implement it".to_string()),
                approval: true,
                ..Task::default()
            }],
        };
        let config = config_with_vars(&[]);
        let state = WorkflowState::new("test", &wf);

        let output = build_next(&wf, &state, &config);
        assert!(output.tasks[0].approval);
    }
}
