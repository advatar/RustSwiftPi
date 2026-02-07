#![forbid(unsafe_code)]

//! Slack (pi-mom) adapter (stub).

use pi_contracts::PiError;

/// Placeholder to keep workspace compiling.
///
/// Drop-in implementations will land incrementally.
pub fn not_implemented(feature: &str) -> PiError {
    PiError::Adapter(format!("{feature} not implemented in this drop"))
}
