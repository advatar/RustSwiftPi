#![forbid(unsafe_code)]

use clap::Parser;
use pi_adapter_fs::JsonDirSessionStore;
use pi_adapter_openai::OpenAiChatProvider;
use pi_adapter_shell::bash_tool;
use pi_contracts::{ChatMessage, NonEmptyString, PiError, SessionId};
use pi_core::{Agent, AgentConfig, SessionStore, ToolContext, ToolSet};
use std::{
    io::{self, Write},
    path::{Path, PathBuf},
};
use tracing_subscriber::EnvFilter;

#[derive(Parser, Debug)]
#[command(name="pi", version, about="pi-mono-rust: minimal coding-agent CLI")]
struct Args {
    /// Model id (OpenAI).
    #[arg(long, default_value="gpt-4o-mini")]
    model: String,

    /// One-shot prompt (non-interactive).
    #[arg(short, long)]
    prompt: Option<String>,

    /// Working directory for tools (default: current directory).
    #[arg(long)]
    cwd: Option<PathBuf>,

    /// System prompt.
    #[arg(long)]
    system: Option<String>,
}

fn pi_dir(cwd: &Path) -> PathBuf {
    cwd.join(".pi")
}

fn session_id_path(cwd: &Path) -> PathBuf {
    pi_dir(cwd).join("session_id")
}

async fn load_or_create_session_id(cwd: &Path) -> Result<SessionId, PiError> {
    let p = session_id_path(cwd);
    if let Ok(s) = tokio::fs::read_to_string(&p).await {
        if let Ok(u) = uuid::Uuid::parse_str(s.trim()) {
            return Ok(SessionId(u));
        }
    }
    tokio::fs::create_dir_all(pi_dir(cwd)).await?;
    let id = SessionId::new();
    tokio::fs::write(&p, id.0.to_string()).await?;
    Ok(id)
}

fn print_new_messages(tr: &[ChatMessage], from_idx: usize) {
    for m in &tr[from_idx..] {
        match m {
            ChatMessage::Assistant { content, tool_calls } => {
                if !content.trim().is_empty() {
                    println!("\nassistant> {content}");
                }
                if !tool_calls.is_empty() {
                    println!("\nassistant(tool_calls)> {} call(s)", tool_calls.len());
                    for tc in tool_calls {
                        println!(" - {} {}", tc.name, tc.id);
                    }
                }
            }
            ChatMessage::Tool { tool_call_id, content } => {
                println!("\ntool[{tool_call_id}]>\n{content}");
            }
            _ => {}
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), PiError> {
    // Allow local dev configuration via `.env` (ignored if missing).
    let _ = dotenvy::dotenv();

    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let args = Args::parse();
    let cwd = args.cwd.unwrap_or(std::env::current_dir().map_err(PiError::from)?);

    let model = NonEmptyString::new(args.model)?;
    let provider = OpenAiChatProvider::from_env()?;

    let mut tools = pi_adapter_fs::coding_tools();
    tools.push(bash_tool());

    let agent = Agent::new(
        provider,
        ToolSet::new(tools),
        AgentConfig {
            model,
            system_prompt: args.system,
            max_steps: 32,
            temperature: None,
            max_tokens: None,
        },
    );

    let session_id = load_or_create_session_id(cwd.as_path()).await?;
    let store = JsonDirSessionStore::new(pi_dir(cwd.as_path()).join("sessions"));

    let mut tr = store.load(session_id.clone()).await?.unwrap_or_default();

    if let Some(p) = args.prompt {
        let before = tr.len();
        agent.run_to_end(&mut tr, &p, ToolContext { cwd: cwd.clone() }).await?;
        store.save(session_id, &tr).await?;
        print_new_messages(&tr, before);
        return Ok(());
    }

    println!("pi-mono-rust interactive. /exit, /quit, /reset");
    let mut input = String::new();
    loop {
        input.clear();
        print!("\nuser> ");
        io::stdout().flush().ok();
        if io::stdin().read_line(&mut input).is_err() {
            break;
        }
        let line = input.trim_end().to_string();
        if line.is_empty() {
            continue;
        }
        match line.as_str() {
            "/exit" | "/quit" => break,
            "/reset" => {
                tr.clear();
                store.save(session_id.clone(), &tr).await?;
                println!("(reset)");
                continue;
            }
            _ => {}
        }

        let before = tr.len();
        if let Err(e) = agent.run_to_end(&mut tr, &line, ToolContext { cwd: cwd.clone() }).await {
            eprintln!("error: {e}");
        }
        store.save(session_id.clone(), &tr).await?;
        print_new_messages(&tr, before);
    }

    Ok(())
}
