//! Model selection: the administrator either forces one model or lets
//! users pick from an allow-list. Model IDs always come from here, never
//! from code.

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "mode")]
pub enum ModelSelection {
    /// Every request uses this model.
    Forced { model: String },
    /// Users may choose from `allowed`; `default` is used when they do not.
    UserChoice {
        allowed: Vec<String>,
        default: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ModelError {
    #[error("model selection is misconfigured: {0}")]
    Config(String),
    #[error("model `{requested}` is not allowed")]
    NotAllowed { requested: String },
}

impl ModelSelection {
    /// Check the configuration itself.
    pub fn validate(&self) -> Result<(), ModelError> {
        match self {
            ModelSelection::Forced { model } => {
                if model.trim().is_empty() {
                    return Err(ModelError::Config("forced model is empty".into()));
                }
            }
            ModelSelection::UserChoice { allowed, default } => {
                if allowed.is_empty() {
                    return Err(ModelError::Config("allow-list is empty".into()));
                }
                if allowed.iter().any(|m| m.trim().is_empty()) {
                    return Err(ModelError::Config("allow-list has an empty entry".into()));
                }
                if !allowed.contains(default) {
                    return Err(ModelError::Config(
                        "default model is not in the allow-list".into(),
                    ));
                }
            }
        }
        Ok(())
    }
}

/// Pick the model for one request. A user request for a different model
/// than a forced one is an error rather than silently overridden, so the
/// UI can tell the user.
pub fn resolve_model(
    selection: &ModelSelection,
    requested: Option<&str>,
) -> Result<String, ModelError> {
    selection.validate()?;
    let requested = requested.map(str::trim).filter(|s| !s.is_empty());
    match selection {
        ModelSelection::Forced { model } => match requested {
            None => Ok(model.clone()),
            Some(r) if r == model => Ok(model.clone()),
            Some(r) => Err(ModelError::NotAllowed {
                requested: r.to_string(),
            }),
        },
        ModelSelection::UserChoice { allowed, default } => match requested {
            None => Ok(default.clone()),
            Some(r) if allowed.iter().any(|m| m == r) => Ok(r.to_string()),
            Some(r) => Err(ModelError::NotAllowed {
                requested: r.to_string(),
            }),
        },
    }
}
