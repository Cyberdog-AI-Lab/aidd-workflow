use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct BashInput {
    pub command: String,
}

#[derive(Debug, Deserialize)]
pub struct EditInput {
    pub file_path: String,
}

#[derive(Debug, Deserialize)]
pub struct PostEditEvent {
    pub tool_input: EditInput,
}

pub type PreEditEvent = PostEditEvent;

#[derive(Debug, Deserialize)]
pub struct PreBashEvent {
    pub tool_input: BashInput,
}
