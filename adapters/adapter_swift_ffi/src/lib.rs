//! Swift FFI adapter.
//!
//! This crate provides a small C ABI surface intended to be called from Swift.
//! The API is intentionally string-based to keep the boundary stable.

use once_cell::sync::Lazy;
use pi_adapter_fs::coding_tools;
use pi_adapter_openai::OpenAiChatProvider;
use pi_adapter_shell::bash_tool;
use pi_contracts::{ChatMessage, NonEmptyString, PiError};
use pi_core::{Agent, AgentConfig, ToolContext, ToolSet, Transcript};
use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::path::PathBuf;
use std::ptr;
use std::sync::Once;

static RT: Lazy<tokio::runtime::Runtime> = Lazy::new(|| {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("pi-swift-ffi")
        .build()
        .expect("tokio runtime")
});

static DOTENV: Once = Once::new();

fn load_dotenv_once() {
    DOTENV.call_once(|| {
        let _ = dotenvy::dotenv();
    });
}

fn nonempty_opt(s: Option<String>) -> Option<String> {
    s.and_then(|s| {
        let t = s.trim();
        (!t.is_empty()).then_some(t.to_string())
    })
}

fn cstr_opt(ptr: *const c_char) -> Result<Option<String>, PiError> {
    if ptr.is_null() {
        return Ok(None);
    }
    // SAFETY: caller promises `ptr` is a valid NUL-terminated string.
    let s = unsafe { CStr::from_ptr(ptr) }
        .to_str()
        .map_err(|_| PiError::Invalid("invalid utf-8 string".into()))?
        .to_string();
    Ok(Some(s))
}

fn cstr_req(ptr: *const c_char, name: &str) -> Result<String, PiError> {
    nonempty_opt(cstr_opt(ptr)?).ok_or_else(|| PiError::Invalid(format!("{name} is required")))
}

fn to_c_string(s: impl Into<String>) -> *mut c_char {
    let s = s.into().replace('\0', "\u{FFFD}");
    CString::new(s)
        .expect("replaced NULs above")
        .into_raw()
}

fn write_out(out: *mut *mut c_char, s: impl Into<String>) {
    if out.is_null() {
        return;
    }
    // SAFETY: caller provides a valid pointer to a `char*` slot.
    unsafe {
        *out = to_c_string(s);
    }
}

fn clear_out(out: *mut *mut c_char) {
    if out.is_null() {
        return;
    }
    // SAFETY: caller provides a valid pointer to a `char*` slot.
    unsafe {
        *out = ptr::null_mut();
    }
}

async fn run_prompt_inner(
    api_key: String,
    base_url: String,
    model: String,
    system_prompt: Option<String>,
    cwd: PathBuf,
    prompt: String,
) -> Result<Transcript, PiError> {
    let model = NonEmptyString::new(model)?;
    let provider = OpenAiChatProvider::new(base_url, api_key);

    let mut tools = coding_tools();
    tools.push(bash_tool());

    let agent = Agent::new(
        provider,
        ToolSet::new(tools),
        AgentConfig {
            model,
            system_prompt,
            max_steps: 32,
            temperature: None,
            max_tokens: None,
        },
    );

    let mut tr: Transcript = vec![];
    agent
        .run_to_end(&mut tr, &prompt, ToolContext { cwd })
        .await?;
    Ok(tr)
}

fn last_assistant_content(tr: &Transcript) -> Result<String, PiError> {
    tr.iter()
        .rev()
        .find_map(|m| match m {
            ChatMessage::Assistant { content, .. } => Some(content.clone()),
            _ => None,
        })
        .ok_or_else(|| PiError::Provider("no assistant message in transcript".into()))
}

fn resolve_api_key(api_key_opt: Option<String>) -> Result<String, PiError> {
    if let Some(k) = nonempty_opt(api_key_opt) {
        return Ok(k);
    }
    std::env::var("OPENAI_API_KEY").map_err(|_| PiError::Invalid("OPENAI_API_KEY not set".into()))
}

fn resolve_base_url(base_url_opt: Option<String>) -> String {
    nonempty_opt(base_url_opt)
        .or_else(|| std::env::var("OPENAI_BASE_URL").ok())
        .unwrap_or_else(|| "https://api.openai.com".into())
}

fn resolve_cwd(cwd_opt: Option<String>) -> Result<PathBuf, PiError> {
    Ok(match nonempty_opt(cwd_opt) {
        Some(p) => PathBuf::from(p),
        None => std::env::current_dir()?,
    })
}

fn resolve_model(model_opt: Option<String>) -> String {
    nonempty_opt(model_opt).unwrap_or_else(|| "gpt-4o-mini".into())
}

/// Frees a string allocated by this library.
///
/// # Safety
/// - `s` must be either null or a pointer returned by this library (e.g. via `pi_run_prompt*`).
/// - `s` must not be freed more than once.
#[no_mangle]
pub unsafe extern "C" fn pi_string_free(s: *mut c_char) {
    if s.is_null() {
        return;
    }
    // SAFETY: must be a string allocated by `CString::into_raw` in this library.
    drop(CString::from_raw(s));
}

/// Runs the agent to completion and returns the final assistant content as a UTF-8 string.
///
/// Returns 0 on success. On failure returns non-zero and writes an error message to `out_error`.
///
/// # Safety
/// - All `*const c_char` inputs must be either null or valid pointers to NUL-terminated UTF-8 strings.
/// - `prompt` must be non-null and point to a non-empty NUL-terminated UTF-8 string.
/// - `out_response`/`out_error` must be either null, or valid pointers to `char*` slots that will be
///   written by this function.
/// - On success, the caller must free `*out_response` via `pi_string_free`.
/// - On failure, the caller must free `*out_error` via `pi_string_free`.
#[no_mangle]
pub unsafe extern "C" fn pi_run_prompt(
    api_key: *const c_char,
    base_url: *const c_char,
    model: *const c_char,
    system_prompt: *const c_char,
    cwd: *const c_char,
    prompt: *const c_char,
    out_response: *mut *mut c_char,
    out_error: *mut *mut c_char,
) -> i32 {
    load_dotenv_once();
    clear_out(out_response);
    clear_out(out_error);

    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<*mut c_char, PiError> {
        let api_key = resolve_api_key(cstr_opt(api_key)?)?;
        let base_url = resolve_base_url(cstr_opt(base_url)?);
        let model = resolve_model(cstr_opt(model)?);
        let system_prompt = nonempty_opt(cstr_opt(system_prompt)?);
        let cwd = resolve_cwd(cstr_opt(cwd)?)?;
        let prompt = cstr_req(prompt, "prompt")?;

        let tr = RT.block_on(run_prompt_inner(api_key, base_url, model, system_prompt, cwd, prompt))?;
        let s = last_assistant_content(&tr)?;
        Ok(to_c_string(s))
    }));

    match r {
        Ok(Ok(s)) => {
            if !out_response.is_null() {
                // SAFETY: `out_response` points to a `char*` slot.
                unsafe {
                    *out_response = s;
                }
            } else {
                // SAFETY: `s` was allocated in this library.
                unsafe { pi_string_free(s) };
            }
            0
        }
        Ok(Err(e)) => {
            write_out(out_error, e.to_string());
            1
        }
        Err(_) => {
            write_out(out_error, "panic across FFI boundary");
            2
        }
    }
}

/// Runs the agent to completion and returns the full transcript as JSON.
///
/// Returns 0 on success. On failure returns non-zero and writes an error message to `out_error`.
///
/// # Safety
/// - All `*const c_char` inputs must be either null or valid pointers to NUL-terminated UTF-8 strings.
/// - `prompt` must be non-null and point to a non-empty NUL-terminated UTF-8 string.
/// - `out_transcript_json`/`out_error` must be either null, or valid pointers to `char*` slots that
///   will be written by this function.
/// - On success, the caller must free `*out_transcript_json` via `pi_string_free`.
/// - On failure, the caller must free `*out_error` via `pi_string_free`.
#[no_mangle]
pub unsafe extern "C" fn pi_run_prompt_transcript_json(
    api_key: *const c_char,
    base_url: *const c_char,
    model: *const c_char,
    system_prompt: *const c_char,
    cwd: *const c_char,
    prompt: *const c_char,
    out_transcript_json: *mut *mut c_char,
    out_error: *mut *mut c_char,
) -> i32 {
    load_dotenv_once();
    clear_out(out_transcript_json);
    clear_out(out_error);

    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<*mut c_char, PiError> {
        let api_key = resolve_api_key(cstr_opt(api_key)?)?;
        let base_url = resolve_base_url(cstr_opt(base_url)?);
        let model = resolve_model(cstr_opt(model)?);
        let system_prompt = nonempty_opt(cstr_opt(system_prompt)?);
        let cwd = resolve_cwd(cstr_opt(cwd)?)?;
        let prompt = cstr_req(prompt, "prompt")?;

        let tr = RT.block_on(run_prompt_inner(api_key, base_url, model, system_prompt, cwd, prompt))?;
        let json = serde_json::to_string(&tr)?;
        Ok(to_c_string(json))
    }));

    match r {
        Ok(Ok(s)) => {
            if !out_transcript_json.is_null() {
                // SAFETY: `out_transcript_json` points to a `char*` slot.
                unsafe {
                    *out_transcript_json = s;
                }
            } else {
                // SAFETY: `s` was allocated in this library.
                unsafe { pi_string_free(s) };
            }
            0
        }
        Ok(Err(e)) => {
            write_out(out_error, e.to_string());
            1
        }
        Err(_) => {
            write_out(out_error, "panic across FFI boundary");
            2
        }
    }
}
