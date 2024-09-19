use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use serde::{Deserialize, Serialize};
use tracing::warn;
use crate::at_commands::at_commands::AtCommandsContext;
use tokio::sync::{Mutex as AMutex, RwLock as ARwLock};
use crate::at_commands::at_file::{file_repair_candidates, return_one_candidate_or_a_good_error};
use crate::files_correction::get_project_dirs;
use crate::global_context::GlobalContext;

#[derive(Default, Debug, Serialize, Deserialize, Clone, PartialEq, Eq, Hash)]
pub enum PatchAction {
    #[default]
    PartialEdit,
    FullRewrite,
    NewFile,
    Other,
}

impl PatchAction {
    pub fn from_string(action: &str) -> Result<PatchAction, String> {
        match action {
            "📍PARTIAL_EDIT" => Ok(PatchAction::PartialEdit),
            "📍FULL_REWRITE" => Ok(PatchAction::FullRewrite),
            "📍NEW_FILE" => Ok(PatchAction::NewFile),
            "📍OTHER" => Ok(PatchAction::Other),
            _ => Err(format!("invalid action: {}", action)),
        }
    }
}

#[derive(Default, Debug, Serialize, Deserialize, Clone)]
pub struct TicketToApply {
    pub action: PatchAction,
    pub ticket: String,
    pub filename_before: String,
    pub filename_after: String,
    pub code: String,
}

pub async fn correct_and_validate_code_snippet(gcx: Arc<ARwLock<GlobalContext>>, snippet: &mut TicketToApply) -> Result<(), String> {
    fn good_error_text(reason: &str, snippet: &TicketToApply) -> String {
        format!("Failed to validate TICKET '{}': {}", snippet.ticket, reason)
    }
    async fn resolve_path(gcx: Arc<ARwLock<GlobalContext>>, path_str: &String) -> Result<String, String> {
        let candidates = file_repair_candidates(gcx.clone(), path_str, 10, false).await;
        return_one_candidate_or_a_good_error(gcx.clone(), path_str, &candidates, &get_project_dirs(gcx.clone()).await, false).await
    }

    let path_before = PathBuf::from(snippet.filename_before.as_str());
    let _path_after = PathBuf::from(snippet.filename_after.as_str());

    match snippet.action {
        PatchAction::PartialEdit => {
            snippet.filename_before = resolve_path(gcx.clone(), &snippet.filename_before).await
                .map_err(|e| good_error_text(&format!("failed to resolve filename_before: '{}'. Error:\n{}", snippet.filename_before, e), snippet))?;
        },
        PatchAction::FullRewrite => {
            snippet.filename_before = resolve_path(gcx.clone(), &snippet.filename_before).await
                .map_err(|e| good_error_text(&format!("failed to resolve filename_before: '{}'. Error:\n{}", snippet.filename_before, e), snippet))?;
        },
        PatchAction::NewFile => {
            if path_before.is_relative() {
                return Err(good_error_text(&format!("filename_before: '{}' must be absolute.", snippet.filename_before), snippet));
            }
        },
        PatchAction::Other => {}
    }
    Ok(())
}

fn parse_snippets(content: &str) -> Vec<TicketToApply> {
    fn process_snippet(lines: &[&str], line_num: usize) -> Result<(usize, TicketToApply), String> {
        let mut snippet = TicketToApply::default();
        let command_line = lines[line_num];
        let info_elements = command_line.trim().split(" ").collect::<Vec<&str>>();
        if info_elements.len() < 3 {
            return Err("failed to parse snippet, invalid command line: {}".to_string());
        }
        snippet.action = match PatchAction::from_string(info_elements[0]) {
            Ok(a) => a,
            Err(e) => {
                return Err(format!("failed to parse snippet, {e}"));
            }
        };
        snippet.ticket = info_elements[1].to_string();
        snippet.filename_before = info_elements[2].to_string();

        if let Some(code_block_fence_line) = lines.get(line_num + 1) {
            if !code_block_fence_line.contains("```") {
                return Err("failed to parse snippet, invalid code block fence".to_string());
            }
            for (idx, line) in lines.iter().enumerate().skip(line_num + 2) {
                if line.contains("```") {
                    return Ok((2 + idx, snippet));
                }
                snippet.code.push_str(format!("{}\n", line).as_str());
            }
            Err("failed to parse snippet, no ending fence for the code block".to_string())
        } else {
            Err("failed to parse snippet, no code block".to_string())
        }
    }

    let lines: Vec<&str> = content.lines().collect();
    let mut line_num = 0;
    let mut blocks: Vec<TicketToApply> = vec![];
    while line_num < lines.len() {
        let line = lines[line_num];
        if line.contains("📍") {
            match process_snippet(&lines, line_num) {
                Ok((new_line_num, snippet)) => {
                    line_num = new_line_num;
                    blocks.push(snippet);
                }
                Err(err) => {
                    warn!("Skipping the snippet due to the error: {err}");
                    line_num += 1;
                    continue;
                }
            };
        } else {
            line_num += 1;
        }
    }
    blocks
}

pub async fn get_code_snippets(
    ccx: Arc<AMutex<AtCommandsContext>>,
) -> HashMap<String, TicketToApply> {
    let messages = ccx.lock().await.messages.clone();
    let mut code_snippets: HashMap<String, TicketToApply> = HashMap::new();
    for message in messages
        .iter()
        .filter(|x| x.role == "assistant") {
        for snippet in parse_snippets(&message.content).into_iter() {
            code_snippets.insert(snippet.ticket.clone(), snippet);
        }
    }
    code_snippets
}
