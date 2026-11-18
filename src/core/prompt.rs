//! Prompt handshake contract.

/// Unique prompt correlation id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PromptId(pub u64);

/// Semantic prompt kinds requested by core.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptKind {
    /// `:` command-line prompt.
    CommandLine,
    /// `/` search prompt.
    Search,
    /// Review comment body composer.
    Comment,
    /// Extension point for future prompts.
    Custom(String),
}

/// Prompt request emitted by core and rendered by the UI adapter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptRequest {
    pub id: PromptId,
    pub kind: PromptKind,
    pub title: String,
    pub placeholder: Option<String>,
    pub initial_value: String,
}
