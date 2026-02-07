#![forbid(unsafe_code)]

//! Filesystem-backed tools + session persistence adapter.

use async_trait::async_trait;
use pi_contracts::{NonEmptyString, PiError, SessionId, ToolSpec};
use pi_core::{SessionStore, Tool, ToolContext, ToolResult, Transcript};
use serde::Deserialize;
use serde_json::Value as Json;
use std::{path::PathBuf, sync::Arc};
use tokio::fs;

fn schema_object(props: Json, required: &[&str]) -> Json {
    serde_json::json!({
        "type":"object",
        "properties": props,
        "required": required,
        "additionalProperties": false
    })
}

/// `read` tool.
pub struct ReadTool;

#[derive(Debug, Deserialize)]
struct ReadArgs {
    path: String,
    #[serde(default)]
    start_line: Option<usize>, // 1-based
    #[serde(default)]
    end_line: Option<usize>,   // 1-based
}

#[async_trait]
impl Tool for ReadTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: NonEmptyString::new("read").unwrap(),
            description: "Read a UTF-8 text file (optionally by line range).".into(),
            parameters: schema_object(
                serde_json::json!({
                    "path": {"type":"string"},
                    "start_line": {"type":"integer","minimum":1},
                    "end_line": {"type":"integer","minimum":1}
                }),
                &["path"],
            ),
        }
    }

    async fn execute(&self, args: Json, ctx: ToolContext) -> Result<ToolResult, PiError> {
        let a: ReadArgs = serde_json::from_value(args)?;
        let p = ctx.cwd.join(a.path);
        let txt = fs::read_to_string(&p).await.map_err(PiError::from)?;
        let out = match (a.start_line, a.end_line) {
            (None, None) => txt,
            (s, e) => {
                let lines: Vec<&str> = txt.lines().collect();
                let start = s.unwrap_or(1).saturating_sub(1);
                let end = e.unwrap_or(lines.len()).min(lines.len());
                lines.get(start..end).unwrap_or(&[]).join("\n")
            }
        };
        Ok(ToolResult::text(out))
    }
}

/// `write` tool.
pub struct WriteTool;

#[derive(Debug, Deserialize)]
struct WriteArgs {
    path: String,
    content: String,
}

#[async_trait]
impl Tool for WriteTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: NonEmptyString::new("write").unwrap(),
            description: "Write a UTF-8 text file, creating parent directories if needed.".into(),
            parameters: schema_object(
                serde_json::json!({
                    "path": {"type":"string"},
                    "content": {"type":"string"}
                }),
                &["path", "content"],
            ),
        }
    }

    async fn execute(&self, args: Json, ctx: ToolContext) -> Result<ToolResult, PiError> {
        let a: WriteArgs = serde_json::from_value(args)?;
        let p = ctx.cwd.join(a.path);
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).await?;
        }
        fs::write(&p, a.content).await?;
        Ok(ToolResult::text(format!("wrote {}", p.display())))
    }
}

/// `edit` tool.
pub struct EditTool;

#[derive(Debug, Deserialize)]
struct EditArgs {
    path: String,
    edits: Vec<Edit>,
}

#[derive(Debug, Deserialize)]
struct Edit {
    find: String,
    replace: String,
}

#[async_trait]
impl Tool for EditTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: NonEmptyString::new("edit").unwrap(),
            description: "Apply exact find/replace edits to a UTF-8 text file (sequential).".into(),
            parameters: schema_object(
                serde_json::json!({
                    "path": {"type":"string"},
                    "edits": {
                      "type":"array",
                      "items":{
                        "type":"object",
                        "properties":{
                          "find":{"type":"string"},
                          "replace":{"type":"string"}
                        },
                        "required":["find","replace"],
                        "additionalProperties":false
                      }
                    }
                }),
                &["path", "edits"],
            ),
        }
    }

    async fn execute(&self, args: Json, ctx: ToolContext) -> Result<ToolResult, PiError> {
        let a: EditArgs = serde_json::from_value(args)?;
        let p = ctx.cwd.join(a.path);
        let mut txt = fs::read_to_string(&p).await?;
        for (i, e) in a.edits.into_iter().enumerate() {
            let n = txt.matches(&e.find).count();
            if n != 1 {
                return Err(PiError::Tool(format!("edit[{i}]: expected 1 match for find-string, got {n}")));
            }
            txt = txt.replacen(&e.find, &e.replace, 1);
        }
        fs::write(&p, txt).await?;
        Ok(ToolResult::text(format!("edited {}", p.display())))
    }
}

/// Session store: directory of JSON transcripts.
#[derive(Clone)]
pub struct JsonDirSessionStore {
    dir: PathBuf,
}

impl JsonDirSessionStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    fn path(&self, id: SessionId) -> PathBuf {
        self.dir.join(format!("{}.json", id.0))
    }
}

#[async_trait]
impl SessionStore for JsonDirSessionStore {
    async fn load(&self, id: SessionId) -> Result<Option<Transcript>, PiError> {
        let p = self.path(id);
        match fs::read_to_string(&p).await {
            Ok(s) => Ok(Some(serde_json::from_str::<Transcript>(&s)?)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(PiError::from(e)),
        }
    }

    async fn save(&self, id: SessionId, transcript: &Transcript) -> Result<(), PiError> {
        fs::create_dir_all(&self.dir).await?;
        let p = self.path(id);
        fs::write(p, serde_json::to_string_pretty(transcript)?).await?;
        Ok(())
    }
}

/// Convenience: builds the default coding-tools set.
pub fn coding_tools() -> Vec<Arc<dyn Tool>> {
    vec![
        Arc::new(ReadTool) as Arc<dyn Tool>,
        Arc::new(WriteTool) as Arc<dyn Tool>,
        Arc::new(EditTool) as Arc<dyn Tool>,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn edit_requires_single_match() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("a.txt");
        fs::write(&p, "x x").await.unwrap();

        let tool = EditTool;
        let err = tool
            .execute(
                serde_json::json!({"path":"a.txt","edits":[{"find":"x","replace":"y"}]}),
                ToolContext { cwd: dir.path().to_path_buf() },
            )
            .await
            .unwrap_err();

        match err {
            PiError::Tool(s) => assert!(s.contains("expected 1 match")),
            _ => panic!("expected tool error"),
        }
    }
}
