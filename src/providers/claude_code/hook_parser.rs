#![allow(dead_code)]

use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct BashInput {
    pub command: String,
}

#[derive(Debug, Deserialize, Default)]
pub struct BashResponse {
    #[serde(default)]
    pub stdout: String,
}

#[derive(Debug, Deserialize)]
pub struct EditInput {
    pub file_path: String,
}

#[derive(Debug, Deserialize)]
pub struct PostBashEvent {
    pub cwd: Option<String>,
    pub tool_input: BashInput,
    #[serde(default)]
    pub tool_response: BashResponse,
}

#[derive(Debug, Deserialize)]
pub struct PostEditEvent {
    pub cwd: Option<String>,
    pub tool_input: EditInput,
}

pub type PreEditEvent = PostEditEvent;

#[derive(Debug, Deserialize)]
pub struct PreBashEvent {
    pub cwd: Option<String>,
    pub tool_input: BashInput,
}
