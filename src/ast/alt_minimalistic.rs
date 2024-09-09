use std::sync::Arc;
use std::fmt;
// use std::cell::RefCell;
// use std::rc::Rc;
use serde::{Deserialize, Serialize};
use tree_sitter::Range;
use crate::ast::treesitter::structs::{RangeDef, SymbolType};

use tokio::sync::{Mutex as AMutex, Notify as ANotify};


#[derive(Serialize, Deserialize, Clone)]
pub struct Usage {
    // Linking means trying to match targets_for_guesswork against official_path, the longer
    // the matched path the more probability the linking was correct
    pub targets_for_guesswork: Vec<String>, // ?::DerivedFrom1::f ?::DerivedFrom2::f ?::f
    pub resolved_as: String,
    pub debug_hint: String,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct AltDefinition {
    pub official_path: Vec<String>,   // file::namespace::class::method becomes ["file", "namespace", "class", "method"]
    pub symbol_type: SymbolType,
    pub derived_from: Vec<Usage>,
    pub usages: Vec<Usage>,
    #[serde(with = "RangeDef")]
    pub full_range: Range,
    #[serde(with = "RangeDef")]
    pub declaration_range: Range,
    #[serde(with = "RangeDef")]
    pub definition_range: Range,
}

impl AltDefinition {
    pub fn path(&self) -> String {
        self.official_path.join("::")
    }

    pub fn name(&self) -> String {
        self.official_path.last().cloned().unwrap_or_default()
    }
}

impl fmt::Debug for AltDefinition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let usages_paths: Vec<String> = self.usages.iter()
            .map(|link| format!("{:?}", link))
            .collect();
        let derived_from_paths: Vec<String> = self.derived_from.iter()
            .map(|link| format!("{:?}", link))
            .collect();

        let usages_str = if usages_paths.is_empty() {
            String::new()
        } else {
            format!(", usages: {}", usages_paths.join(" "))
        };

        let derived_from_str = if derived_from_paths.is_empty() {
            String::new()
        } else {
            format!(", derived_from: {}", derived_from_paths.join(" "))
        };

        write!(
            f,
            "AltDefinition {{ {}{}{} }}",
            self.official_path.join("::"),
            usages_str,
            derived_from_str
        )
    }
}


impl fmt::Debug for Usage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // self.target_for_guesswork
        write!(
            f,
            "Link{{ {} {} }}",
            self.debug_hint,
            if self.resolved_as.len() > 0 { self.resolved_as.clone() } else { self.targets_for_guesswork.join(" ") + &", unresolved" }
        )
    }
}



pub struct AltIndex {
    pub sleddb: Arc<sled::Db>, // doesn't need a mutex
}

pub struct AltStatus {
    pub astate_notify: Arc<ANotify>,
    pub astate: String,
    pub files_unparsed: usize,
    pub files_total: usize,
    pub ast_index_files_total: usize,
    pub ast_index_symbols_total: usize,
}

pub struct AltState {
    pub alt_index: Arc<AMutex<AltIndex>>,
    pub alt_status: Arc<AMutex<AltStatus>>,
}
