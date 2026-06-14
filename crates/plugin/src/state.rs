//! Argv inputs and the result envelope exchanged with plugin CLIs.

use serde::{Deserialize, Serialize};

/// Inputs the host passes to a spawned plugin action via argv.
#[derive(Clone, Debug, Default)]
pub struct ActionArgs {
    /// Daemon REST base URL (`--endpoint`).
    pub endpoint: String,
    /// Selected item ids (`--selected a,b,c`), empty when not targeted.
    pub selected: Vec<String>,
    /// Active item id (`--active id`), when targeted.
    pub active: Option<String>,
    /// Form parameters (`--param name=value`), order-preserving.
    pub params: Vec<(String, String)>,
}

/// Output from a plugin action invocation (printed as one JSON object).
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RunOutput {
    /// "ok" or "error".
    pub status: String,
    /// Text result content produced by the action.
    #[serde(default)]
    pub result: Option<String>,
    /// Content type of the result field.
    #[serde(default)]
    pub content_type: Option<String>,
    /// Suggested download filename for result content.
    #[serde(default)]
    pub filename: Option<String>,
    /// Error message when status is "error".
    #[serde(default)]
    pub message: Option<String>,
}

impl RunOutput {
    /// Successful result with the given text.
    #[must_use]
    pub fn ok(result: impl Into<String>) -> Self {
        Self {
            status: "ok".to_string(),
            result: Some(result.into()),
            content_type: None,
            filename: None,
            message: None,
        }
    }

    /// Error result with the given message.
    #[must_use]
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            status: "error".to_string(),
            result: None,
            content_type: None,
            filename: None,
            message: Some(message.into()),
        }
    }

    /// Set the result content type.
    #[must_use]
    pub fn content_type(mut self, ct: impl Into<String>) -> Self {
        self.content_type = Some(ct.into());
        self
    }

    /// Set the suggested download filename.
    #[must_use]
    pub fn filename(mut self, filename: impl Into<String>) -> Self {
        self.filename = Some(filename.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::{ActionArgs, RunOutput};

    #[test]
    fn run_output_ok_has_no_message() {
        let out = RunOutput::ok("hello").filename("x.bib");
        assert_eq!(out.status, "ok");
        assert_eq!(out.result.as_deref(), Some("hello"));
        assert_eq!(out.filename.as_deref(), Some("x.bib"));
        assert!(out.message.is_none());
    }

    #[test]
    fn action_args_default_is_empty() {
        let args = ActionArgs::default();
        assert!(args.endpoint.is_empty());
        assert!(args.selected.is_empty());
        assert!(args.active.is_none());
        assert!(args.params.is_empty());
    }
}
