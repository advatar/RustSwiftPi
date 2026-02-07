#![forbid(unsafe_code)]

//! Shell execution tool adapter (`bash`).

use async_trait::async_trait;
use pi_contracts::{NonEmptyString, PiError, ToolSpec};
use pi_core::{Tool, ToolContext, ToolResult};
use serde::Deserialize;
use serde_json::Value as Json;
use std::time::Duration;
use tokio::{process::Command, time::timeout};

fn schema_object(props: Json, required: &[&str]) -> Json {
    serde_json::json!({
        "type":"object",
        "properties": props,
        "required": required,
        "additionalProperties": false
    })
}

pub struct BashTool;

#[derive(Debug, Deserialize)]
struct BashArgs {
    command: String,
    #[serde(default)]
    timeout_ms: Option<u64>,
}

#[async_trait]
impl Tool for BashTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: NonEmptyString::new("bash").unwrap(),
            description: "Run a shell command (sh -lc) in the current working directory.".into(),
            parameters: schema_object(
                serde_json::json!({
                    "command": {"type":"string"},
                    "timeout_ms": {"type":"integer","minimum":1}
                }),
                &["command"],
            ),
        }
    }

    async fn execute(&self, args: Json, ctx: ToolContext) -> Result<ToolResult, PiError> {
        let a: BashArgs = serde_json::from_value(args)?;
        let mut cmd = Command::new("sh");
        cmd.arg("-lc").arg(a.command);
        cmd.current_dir(ctx.cwd);
        cmd.kill_on_drop(true);
        let fut = cmd.output();
        let out = match a.timeout_ms {
            Some(ms) => timeout(Duration::from_millis(ms), fut)
                .await
                .map_err(|_| PiError::Timeout("bash timed out".into()))??,
            None => fut.await?,
        };

        let stdout = String::from_utf8_lossy(&out.stdout).to_string();
        let stderr = String::from_utf8_lossy(&out.stderr).to_string();
        let code = out.status.code().unwrap_or(-1);

        Ok(ToolResult::text(format!(
            "exit_code: {code}\nstdout:\n{stdout}\nstderr:\n{stderr}"
        )))
    }
}

/// Convenience: returns the bash tool as an `Arc<dyn Tool>`.
pub fn bash_tool() -> std::sync::Arc<dyn Tool> {
    std::sync::Arc::new(BashTool)
}
