use std::path::Path;
use anyhow::{Context, Result, bail};
use super::types::{Config, Action};

pub fn load_config(cwd: &Path) -> Result<Config> {
    let path = cwd.join(".workflow/config.yml");
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!(".workflow/config.yml が見つかりません: {}", path.display()))?;
    let config: Config = serde_yaml::from_str(&content)
        .context("config.yml のパースに失敗しました")?;
    validate(&config)?;
    Ok(config)
}

fn validate(config: &Config) -> Result<()> {
    if !config.commands.contains_key("test") {
        bail!("config.yml の commands.test が未定義です");
    }
    for (slug, wf) in &config.workflows {
        if wf.steps.is_empty() {
            bail!("ワークフロー '{}' にステップがありません", slug);
        }
        let ids: std::collections::HashSet<&str> =
            wf.steps.iter().map(|s| s.id.as_str()).collect();

        for step in &wf.steps {
            if !step.actions.is_empty() && step.parallel.is_some() {
                bail!(
                    "ワークフロー '{}' のステップ '{}' は actions と parallel を同時に持てません",
                    slug, step.id
                );
            }
            // actions フィールドは Vec なので is_some は不要 - parallel チェックのみ
            for req in &step.requires {
                if !ids.contains(req.as_str()) {
                    bail!(
                        "ワークフロー '{}' のステップ '{}' の requires に未定義のステップ '{}' があります",
                        slug, step.id, req
                    );
                }
            }
            for action in &step.actions {
                if let Action::Run { command: _, gate: _ } = action {
                    // gate はコマンドキー参照ではなく bool なので commands 照合不要
                }
            }
        }
    }
    Ok(())
}
