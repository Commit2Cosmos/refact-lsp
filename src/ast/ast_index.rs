use std::collections::{HashMap, HashSet};

use fst::{Set, set, Streamer};
use itertools::Itertools;
use rayon::prelude::*;
use ropey::Rope;
use sorted_vec::SortedVec;
use strsim::jaro_winkler;
use tracing::info;
use tree_sitter::Point;
use url::Url;

use crate::ast::comments_wrapper::get_language_id_by_filename;
use crate::ast::fst_extra_automation::Substring;
use crate::ast::structs::{FileASTMarkup, SymbolsSearchResultStruct};
use crate::ast::treesitter::ast_instance_structs::{AstSymbolInstanceArc, SymbolInformation};
use crate::ast::treesitter::language_id::LanguageId;
use crate::ast::treesitter::parsers::get_new_parser_by_filename;
use crate::ast::treesitter::structs::SymbolType;
use crate::ast::usages_declarations_merger::{FilePathIterator, find_decl_by_caller_guid, find_decl_by_name};
use crate::files_in_workspace::DocumentInfo;

#[derive(Debug)]
pub struct AstIndex {
    symbols_by_name: HashMap<String, Vec<AstSymbolInstanceArc>>,
    symbols_by_guid: HashMap<String, AstSymbolInstanceArc>,
    path_by_symbols: HashMap<Url, Vec<AstSymbolInstanceArc>>,
    symbols_search_index: HashMap<Url, Set<Vec<u8>>>,
    type_guid_to_dependand_guids: HashMap<String, Vec<String>>,
    has_changes: bool,
}


#[derive(Debug, Clone, Copy)]
pub(crate) enum RequestSymbolType {
    Declaration,
    Usage,
    All,
}


fn make_a_query(
    nodes_indexes: &HashMap<Url, Set<Vec<u8>>>,
    query_str: &str,
    exception_doc: Option<DocumentInfo>,
) -> Vec<String> {
    let matcher = Substring::new(query_str, true);
    let mut stream_builder = set::OpBuilder::new();

    for (doc, set) in nodes_indexes {
        if let Some(ref exception) = exception_doc {
            if *doc == exception.uri {
                continue;
            }
        }
        stream_builder = stream_builder.add(set.search(matcher.clone()));
    }

    let mut stream = stream_builder.union();
    let mut found_keys = Vec::new();
    while let Some(key) = stream.next() {
        if let Ok(key_str) = String::from_utf8(key.to_vec()) {
            found_keys.push(key_str);
        }
    }
    found_keys
}

impl AstIndex {
    pub fn init() -> AstIndex {
        AstIndex {
            symbols_by_name: HashMap::new(),
            symbols_by_guid: HashMap::new(),
            path_by_symbols: HashMap::new(),
            symbols_search_index: HashMap::new(),
            type_guid_to_dependand_guids: HashMap::new(),
            has_changes: false,
        }
    }

    pub fn parse(doc: &DocumentInfo) -> Result<Vec<AstSymbolInstanceArc>, String> {
        let mut parser = match get_new_parser_by_filename(&doc.get_path()) {
            Ok(parser) => parser,
            Err(err) => {
                return Err(err.message);
            }
        };
        let text = match doc.read_file_blocked() {
            Ok(s) => s,
            Err(e) => return Err(e.to_string())
        };

        let t_ = std::time::Instant::now();
        let symbol_instances = parser.parse(text.as_str(), &doc.uri);
        let t_elapsed = t_.elapsed();

        info!(
            "parsed {}, {} symbols, took {:.3}s to parse",
            crate::nicer_logs::last_n_chars(&doc.uri.to_string(), 30),
            symbol_instances.len(), t_elapsed.as_secs_f32()
        );
        Ok(symbol_instances)
    }

    pub fn add_or_update_symbols_index(
        &mut self,
        doc: &DocumentInfo,
        symbols: &Vec<AstSymbolInstanceArc>,
    ) -> Result<(), String> {
        let has_removed = self.remove(&doc);
        if has_removed {
            self.resolve_types(symbols);
            self.merge_usages_to_declarations(symbols);
            self.create_extra_indexes(symbols);
            self.has_changes = false;
        } else {
            // TODO: we don't want to update the whole index for a single file
            // even if we might miss some new cross-references
            // later we should think about some kind of force update, ie once in a while self.has_changes=false
            self.has_changes = true;
        }

        let mut symbol_names: SortedVec<String> = SortedVec::new();
        for symbol in symbols.iter() {
            let symbol_ref = symbol.read().expect("the data might be broken");
            self.symbols_by_name.entry(symbol_ref.name().to_string()).or_insert_with(Vec::new).push(symbol.clone());
            self.symbols_by_guid.insert(symbol_ref.guid().to_string(), symbol.clone());
            self.path_by_symbols.entry(doc.uri.clone()).or_insert_with(Vec::new).push(symbol.clone());
            symbol_names.push(symbol_ref.name().to_string());
        }
        let meta_names_set = match Set::from_iter(symbol_names.iter()) {
            Ok(set) => set,
            Err(e) => return Err(format!("Error creating set: {}", e)),
        };
        self.symbols_search_index.insert(doc.uri.clone(), meta_names_set);

        Ok(())
    }

    pub fn add_or_update(&mut self, doc: &DocumentInfo) -> Result<(), String> {
        let symbols = AstIndex::parse(doc)?;
        self.add_or_update_symbols_index(doc, &symbols)
    }

    pub fn remove(&mut self, doc: &DocumentInfo) -> bool {
        let has_removed = self.symbols_search_index.remove(&doc.uri).is_some();
        let mut removed_guids = HashSet::new();
        for symbol in self.path_by_symbols
            .remove(&doc.uri)
            .unwrap_or_default()
            .iter() {
            let (name, guid) = {
                let symbol_ref = symbol.read().expect("the data might be broken");
                (symbol_ref.name().to_string(), symbol_ref.guid().to_string())
            };
            self.symbols_by_name
                .entry(name)
                .and_modify(|v| {
                    let indices_to_remove = v
                        .iter()
                        .enumerate()
                        .filter(|(_idx, s)| s.read().expect("the data might be broken").guid() == guid)
                        .map(|(idx, _s)| idx)
                        .collect::<Vec<_>>();
                    indices_to_remove.iter().for_each(|i| { v.remove(*i); });
                });

            self.symbols_by_guid.remove(&guid);
            if self.type_guid_to_dependand_guids.contains_key(&guid) {
                // TODO: we should do the removing more precisely,
                // some leftovers still are in the values, but it doesn't break the overall thing for now
                self.type_guid_to_dependand_guids.remove(&guid);
            }
            removed_guids.insert(guid);
        }
        for symbol in self.symbols_by_guid.values_mut() {
            symbol.write().expect("the data might be broken").remove_linked_guids(&removed_guids);
        }
        self.has_changes = true;
        has_removed
    }

    pub fn clear_index(&mut self) {
        self.symbols_by_name.clear();
        self.symbols_by_guid.clear();
        self.path_by_symbols.clear();
        self.symbols_search_index.clear();
        self.has_changes = true;
    }

    pub fn search_by_name(
        &self,
        query: &str,
        request_symbol_type: RequestSymbolType,
        exception_doc: Option<DocumentInfo>,
        language: Option<LanguageId>,
    ) -> Result<Vec<SymbolsSearchResultStruct>, String> {
        fn exact_search(
            symbols_by_name: &HashMap<String, Vec<AstSymbolInstanceArc>>,
            query: &str,
            request_symbol_type: &RequestSymbolType,
        ) -> Vec<AstSymbolInstanceArc> {
            symbols_by_name
                .get(query)
                .map(|x| x.clone())
                .unwrap_or_default()
                .iter()
                .cloned()
                .filter(|s| {
                    let s_ref = s.read().expect("the data might be broken");
                    match request_symbol_type {
                        RequestSymbolType::Declaration => s_ref.is_declaration(),
                        RequestSymbolType::Usage => !s_ref.is_declaration(),
                        RequestSymbolType::All => true,
                    }
                })
                .collect()
        }

        fn fuzzy_search(
            search_index: &HashMap<Url, Set<Vec<u8>>>,
            symbols_by_name: &HashMap<String, Vec<AstSymbolInstanceArc>>,
            query: &str,
            exception_doc: Option<DocumentInfo>,
            request_symbol_type: &RequestSymbolType,
        ) -> Vec<AstSymbolInstanceArc> {
            make_a_query(search_index, query, exception_doc)
                .iter()
                .map(|name| symbols_by_name
                    .get(name)
                    .map(|x| x.clone())
                    .unwrap_or_default())
                .flatten()
                .filter(|s| {
                    let s_ref = s.read().expect("the data might be broken");
                    match request_symbol_type {
                        RequestSymbolType::Declaration => s_ref.is_declaration(),
                        RequestSymbolType::Usage => !s_ref.is_declaration(),
                        RequestSymbolType::All => true,
                    }
                })
                .collect()
        }

        let mut symbols = exact_search(&self.symbols_by_name, query, &request_symbol_type);
        if symbols.is_empty() {
            symbols = fuzzy_search(
                &self.symbols_search_index, &self.symbols_by_name,
                query, exception_doc, &request_symbol_type,
            );
        }

        let mut filtered_search_results = symbols
            .iter()
            .filter(|s| {
                let s_ref = s.read().expect("the data might be broken");
                *s_ref.language() == language.unwrap_or(*s_ref.language())
            })
            .map(|s| {
                let s_ref = s.read().expect("the data might be broken");
                (s_ref.symbol_info_struct(), (jaro_winkler(query, s_ref.name()) as f32).max(f32::MIN_POSITIVE))
            })
            .collect::<Vec<_>>();

        filtered_search_results
            .sort_by(|(_, dist_1), (_, dist_2)|
                dist_1.partial_cmp(dist_2).unwrap_or(std::cmp::Ordering::Equal)
            );

        let mut search_results: Vec<SymbolsSearchResultStruct> = vec![];
        for (key, dist) in filtered_search_results
            .iter()
            .rev() {
            let content = match key.get_content_blocked() {
                Ok(content) => content,
                Err(err) => {
                    info!("Error opening the file {:?}: {}", key.file_url, err);
                    continue;
                }
            };
            search_results.push(SymbolsSearchResultStruct {
                symbol_declaration: key.clone(),
                content: content,
                sim_to_query: dist.clone(),
            });
        }
        Ok(search_results)
    }

    pub fn search_by_content(
        &self,
        query: &str,
        request_symbol_type: RequestSymbolType,
        exception_doc: Option<DocumentInfo>,
        language: Option<LanguageId>,
    ) -> Result<Vec<SymbolsSearchResultStruct>, String> {
        let search_results = self.path_by_symbols
            .iter()
            .filter(|(path, _symbols)| {
                let file_path = match path.to_file_path() {
                    Ok(fp) => fp,
                    Err(_) => return false,
                };
                let language_id = match get_language_id_by_filename(&file_path) {
                    Some(lid) => lid,
                    None => return false,
                };
                let correct_language = language.map_or(true, |l| l == language_id);
                let correct_doc = exception_doc.clone().map_or(true, |doc| doc.uri != **path);
                correct_doc && correct_language
            })
            .collect::<Vec<_>>()
            .par_iter()
            .filter_map(|(path, symbols)| {
                let mut found_symbols = vec![];
                let file_path = match path.to_file_path() {
                    Ok(path) => path,
                    Err(_) => return None
                };
                let file_content = match std::fs::read_to_string(&file_path) {
                    Ok(content) => content,
                    Err(err) => {
                        info!("Error opening the file {:?}: {}", &file_path, err);
                        return None;
                    }
                };
                let text_rope = Rope::from_str(file_content.as_str());
                for symbol in symbols.iter() {
                    let s_ref = symbol.read().expect("the data might be broken");
                    let symbol_content = text_rope
                        .slice(text_rope.line_to_char(s_ref.full_range().start_point.row)..
                            text_rope.line_to_char(s_ref.full_range().end_point.row))
                        .to_string();
                    match symbol_content.find(query) {
                        Some(_) => found_symbols.push(symbol.clone()),
                        None => { continue; }
                    }
                }
                Some(found_symbols)
            })
            .flatten()
            .filter(|s| {
                let s_ref = s.read().expect("the data might be broken");
                match request_symbol_type {
                    RequestSymbolType::Declaration => s_ref.is_declaration(),
                    RequestSymbolType::Usage => !s_ref.is_declaration(),
                    RequestSymbolType::All => true,
                }
            })
            .filter_map(|s| {
                let info_struct = s.read().expect("the data might be broken").symbol_info_struct();
                let content = info_struct.get_content_blocked().ok()?;
                Some(SymbolsSearchResultStruct {
                    symbol_declaration: info_struct,
                    content: content,
                    sim_to_query: -1.0,
                })
            })
            .collect::<Vec<_>>();

        Ok(search_results)
    }

    pub fn search_related_declarations(&self, guid: &str) -> Result<Vec<SymbolsSearchResultStruct>, String> {
        match self.symbols_by_guid.get(guid) {
            Some(symbol) => {
                Ok(symbol
                    .read().expect("the data might be broken")
                    .types()
                    .iter()
                    .filter_map(|t| t.guid.clone())
                    .filter_map(|g| self.symbols_by_guid.get(&g))
                    .filter_map(|s| {
                        let info_struct = s.read().expect("the data might be broken").symbol_info_struct();
                        let content = info_struct.get_content_blocked().ok()?;
                        Some(SymbolsSearchResultStruct {
                            symbol_declaration: info_struct,
                            content: content,
                            sim_to_query: -1.0,
                        })
                    })
                    .collect::<Vec<_>>())
            }
            _ => Ok(vec![])
        }
    }

    pub fn search_symbols_by_declarations_usage(
        &self,
        declaration_guid: &str,
        exception_doc: Option<DocumentInfo>,
    ) -> Result<Vec<SymbolsSearchResultStruct>, String> {
        Ok(self.type_guid_to_dependand_guids
            .get(declaration_guid)
            .map(|x| x.clone())
            .unwrap_or_default()
            .iter()
            .filter_map(|guid| self.symbols_by_guid.get(guid))
            .filter(|s| {
                let s_ref = s.read().expect("the data might be broken");
                exception_doc.clone().map_or(true, |doc| doc.uri != *s_ref.file_url())
            })
            .filter_map(|s| {
                let info_struct = s.read().expect("the data might be broken").symbol_info_struct();
                let content = info_struct.get_content_blocked().ok()?;
                Some(SymbolsSearchResultStruct {
                    symbol_declaration: info_struct,
                    content: content,
                    sim_to_query: -1.0,
                })
            })
            .collect::<Vec<_>>())
    }

    pub fn retrieve_cursor_symbols_by_declarations(
        &self,
        doc: &DocumentInfo,
        code: &str,
        cursor: Point,
        top_n_near_cursor: usize,
        top_n_usage_for_each_decl: usize,
    ) -> (Vec<SymbolInformation>, Vec<SymbolInformation>, Vec<SymbolInformation>) {
        let file_symbols = self.parse_single_file(doc, code);
        let language = get_language_id_by_filename(&doc.uri.to_file_path().unwrap_or_default());
        let unfiltered_cursor_symbols = file_symbols
            .iter()
            .unique_by(|s| s.read().expect("the data might be broken").guid().to_string())
            .sorted_by_key(|a| a.read().expect("the data might be broken").distance_to_cursor(&cursor))
            .collect::<Vec<_>>();
        let cursor_symbols = unfiltered_cursor_symbols
            .iter()
            .cloned()
            .filter_map(|s| {
                let s_ref = s.read().expect("the data might be broken");
                if s_ref.is_declaration() {
                    Some(s)
                } else {
                    s_ref.get_linked_decl_guid()
                        .clone()
                        .map(|guid| self.symbols_by_guid.get(&guid))
                        .flatten()
                }
            })
            .cloned()
            .collect::<Vec<_>>();
        let declarations = cursor_symbols
            .iter()
            .filter(|s| !s.read().expect("the data might be broken").types().is_empty())
            .take(top_n_near_cursor)
            .map(|s| {
                s.read().expect("the data might be broken")
                    .types()
                    .iter()
                    .filter_map(|t| t.guid.clone())
                    .filter_map(|g| self.symbols_by_guid.get(&g))
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .flatten()
            .filter(|s| {
                let s_ref = s.read().expect("the data might be broken");
                *s_ref.language() == language.unwrap_or(*s_ref.language())
            })
            .map(|s| s.read().expect("the data might be broken").symbol_info_struct())
            .unique_by(|s| s.guid.clone())
            .collect::<Vec<_>>();
        let usages = declarations
            .iter()
            .map(|s| {
                self.search_symbols_by_declarations_usage(&s.guid, Some(doc.clone()))
                    .unwrap_or_default()
                    .iter()
                    .map(|x| x.symbol_declaration.clone())
                    .sorted_by_key(|s| {
                        match s.symbol_type {
                            SymbolType::ClassFieldDeclaration => 1,
                            SymbolType::VariableDefinition => 1,
                            SymbolType::FunctionDeclaration => 1,
                            SymbolType::FunctionCall => 2,
                            SymbolType::VariableUsage => 2,
                            _ => 0,
                        }
                    })
                    .take(top_n_usage_for_each_decl)
                    .collect::<Vec<_>>()
            })
            .flatten()
            .collect::<Vec<_>>();
        (
            unfiltered_cursor_symbols
                .iter()
                .map(|s| s.read().expect("the data might be broken").symbol_info_struct())
                .collect(),
            declarations
                .iter()
                .filter(|s| doc.uri != s.file_url)
                .cloned()
                .collect::<Vec<_>>(),
            usages
                .iter()
                .filter(|s| doc.uri != s.file_url)
                .cloned()
                .collect::<Vec<_>>(),
        )
    }

    pub async fn file_markup(
        &self,
        doc: &DocumentInfo,
    ) -> Result<FileASTMarkup, String> {
        // fn within_range(
        //     decl_range: &Range,
        //     line_idx: usize,
        // ) -> bool {
        //     decl_range.start_point.row <= line_idx
        //         && decl_range.end_point.row >= line_idx
        // }

        // fn sorted_candidates_within_line(
        //     symbols: &Vec<AstSymbolInstanceArc>,
        //     line_idx: usize,
        // ) -> (Vec<AstSymbolInstanceArc>, bool) {
        //     let filtered_symbols = symbols
        //         .iter()
        //         .filter(|s| within_range(&s.read().expect("the data might be broken").full_range(), line_idx))
        //         .sorted_by_key(
        //             |s| {
        //                 let s_ref = s.read().expect("the data might be broken");
        //                 s_ref.full_range().end_point.row - s_ref.full_range().start_point.row
        //             }
        //         )
        //         .rev()
        //         .cloned()
        //         .collect::<Vec<_>>();
        //     let is_signature = symbols
        //         .iter()
        //         .map(|s| within_range(&s.read().expect("the data might be broken").declaration_range(), line_idx))
        //         .any(|x| x);
        //     (filtered_symbols, is_signature)
        // }

        let symbols: Vec<AstSymbolInstanceArc> = self.path_by_symbols
            .get(&doc.uri)
            .map(|symbols| {
                symbols
                    .iter()
                    .filter(|s| s.read().expect("the data might be broken").is_declaration())
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        let file_content = match doc.read_file().await {
            Ok(content) => content,
            Err(e) => return Err(e.to_string())
        };

        let file_ast_markup = FileASTMarkup {
            file_url: doc.uri.clone(),
            file_content: file_content,
            guid2symbol: symbols.iter().map(|s| {
                let s_ref = s.read().expect("the data might be broken");
                (s_ref.guid().to_string(), s_ref.symbol_info_struct())
            }).collect(),
        };
        Ok(file_ast_markup)
    }

    pub fn get_by_file_path(
        &self,
        request_symbol_type: RequestSymbolType,
        doc: &DocumentInfo,
    ) -> Result<Vec<SymbolInformation>, String> {
        let symbols = self.path_by_symbols
            .get(&doc.uri)
            .map(|symbols| {
                symbols
                    .iter()
                    .filter(|s| {
                        let s_ref = s.read().expect("the data might be broken");
                        match request_symbol_type {
                            RequestSymbolType::Declaration => s_ref.is_declaration(),
                            RequestSymbolType::Usage => !s_ref.is_declaration(),
                            RequestSymbolType::All => true,
                        }
                    })
                    .map(|s| s.read().expect("the data might be broken").symbol_info_struct())
                    .collect()
            })
            .unwrap_or_default();
        Ok(symbols)
    }

    // pub fn get_file_paths(&self) -> Vec<Url> {
    //     self.symbols_search_index.iter().map(|(path, _)| path.clone()).collect()
    // }

    pub fn get_symbols_names(
        &self,
        request_symbol_type: RequestSymbolType,
    ) -> Vec<String> {
        self.symbols_by_guid
            .iter()
            .filter(|(_guid, s)| {
                let s_ref = s.read().expect("the data might be broken");
                match request_symbol_type {
                    RequestSymbolType::Declaration => s_ref.is_declaration(),
                    RequestSymbolType::Usage => !s_ref.is_declaration(),
                    RequestSymbolType::All => true,
                }
            })
            .map(|(_guid, s)| s.read().expect("the data might be broken").name().to_string())
            .collect()
    }

    pub fn rebuild_index(&mut self) {
        if !self.has_changes {
            return;
        }
        let symbols = self.symbols_by_guid.values().cloned().collect::<Vec<_>>();
        info!("Building ast declarations");
        let t0 = std::time::Instant::now();
        self.resolve_types(&symbols);
        info!("Building ast declarations finished, took {:.3}s", t0.elapsed().as_secs_f64());

        info!("Merging usages and declarations");
        let t1 = std::time::Instant::now();
        self.merge_usages_to_declarations(&symbols);
        info!("Merging usages and declarations finished, took {:.3}s", t1.elapsed().as_secs_f64());

        info!("Creating extra indexes");
        let t1 = std::time::Instant::now();
        self.create_extra_indexes(&symbols);
        info!("Creating extra indexes finished, took {:.3}s", t1.elapsed().as_secs_f64());

        self.has_changes = false;
    }

    fn resolve_types(&self, symbols: &Vec<AstSymbolInstanceArc>) {
        for symbol in symbols {
            let (type_names, symb_type, symb_path) = {
                let s_ref = symbol.read().expect("the data might be broken");
                (s_ref.types(), s_ref.symbol_type(), s_ref.file_url().to_file_path().unwrap_or_default())
            };
            if symb_type == SymbolType::ImportDeclaration
                || symb_type == SymbolType::CommentDefinition
                || symb_type == SymbolType::FunctionCall
                || symb_type == SymbolType::VariableUsage {
                continue;
            }

            let mut new_guids = vec![];
            for (idx, t) in type_names.iter().enumerate() {
                // TODO: make a type inference by `inference_info`
                if t.is_pod || t.guid.is_some() || t.name.is_none() {
                    new_guids.push(t.guid.clone());
                    continue;
                }

                let name = t.name.clone().expect("filter has invalid condition");
                let maybe_guid = match self.symbols_by_name.get(&name) {
                    Some(symbols) => {
                        symbols
                            .iter()
                            .filter(|s| s.read().expect("the data might be broken").is_type())
                            .sorted_by(|a, b| {
                                let path_a = a.read().expect("the data might be broken")
                                    .file_url().to_file_path().unwrap_or_default();
                                let path_b = b.read().expect("the data might be broken")
                                    .file_url().to_file_path().unwrap_or_default();
                                FilePathIterator::compare_paths(&symb_path, &path_a, &path_b)
                            })
                            .next()
                            .map(|s| s.read().expect("the data might be broken").guid().to_string())
                    }
                    None => {
                        new_guids.push(None);
                        continue;
                    }
                };

                match maybe_guid {
                    Some(guid) => {
                        new_guids.push(Some(guid));
                        info!("Found type name {} at index {}", name, idx);
                    }
                    None => {
                        new_guids.push(None);
                        info!("Could not find type name {} at index {}", name, idx);
                    }
                }
            }
            assert_eq!(new_guids.len(), type_names.len());
            symbol
                .write().expect("the data might be broken")
                .set_guids_to_types(&new_guids);
        }
    }

    fn merge_usages_to_declarations(&self, symbols: &Vec<AstSymbolInstanceArc>) {
        fn get_caller_depth(
            symbol: &AstSymbolInstanceArc,
            guid_by_symbols: &HashMap<String, AstSymbolInstanceArc>,
            current_depth: usize,
        ) -> usize {
            let caller_guid = match symbol
                .read().expect("the data might be broken")
                .get_caller_guid()
                .clone() {
                Some(g) => g,
                None => return current_depth,
            };
            match guid_by_symbols.get(&caller_guid) {
                Some(s) => get_caller_depth(
                    s, guid_by_symbols, current_depth + 1,
                ),
                None => current_depth,
            }
        }

        let mut depth: usize = 0;
        loop {
            let symbols_to_process = symbols
                .iter()
                .filter(|symbol| {
                    symbol.read().expect("the data might be broken").get_linked_decl_guid().is_none()
                })
                .filter(|symbol| {
                    let s_ref = symbol.read().expect("the data might be broken");
                    let valid_depth = get_caller_depth(symbol, &self.symbols_by_guid, 0) == depth;
                    valid_depth && (s_ref.symbol_type() == SymbolType::FunctionCall
                        || s_ref.symbol_type() == SymbolType::VariableUsage)
                })
                .collect::<Vec<_>>();

            if symbols_to_process.is_empty() {
                break;
            }

            for (idx, usage_symbol) in symbols_to_process
                .iter().enumerate() {
                info!("Processing symbol ({}/{})", idx, symbols_to_process.len());
                let caller_guid = usage_symbol
                    .read().expect("the data might be broken")
                    .get_caller_guid().clone();
                let decl_guid = match caller_guid {
                    Some(ref guid) => {
                        match find_decl_by_caller_guid(
                            *usage_symbol,
                            &guid,
                            &self.symbols_by_guid,
                        ) {
                            Some(decl_guid) => { Some(decl_guid) }
                            None => find_decl_by_name(
                                *usage_symbol,
                                &self.path_by_symbols,
                                &self.symbols_by_guid,
                                &self.symbols_by_name,
                                1,
                            )
                        }
                    }
                    None => find_decl_by_name(
                        *usage_symbol,
                        &self.path_by_symbols,
                        &self.symbols_by_guid,
                        &self.symbols_by_name,
                        1,
                    )
                };

                match decl_guid {
                    Some(guid) => {
                        {
                            usage_symbol
                                .write()
                                .expect("the data might be broken")
                                .set_linked_decl_guid(Some(guid))
                        }
                        info!("found declaration {}", usage_symbol.read().expect("").name());
                    }
                    None => {
                        info!("could not find declaration {}", usage_symbol.read().expect("").name());
                    }
                }
            }
            depth += 1;
        }
    }

    fn create_extra_indexes(&mut self, symbols: &Vec<AstSymbolInstanceArc>) {
        for symbol in symbols
            .iter()
            .filter(|s| !s.read().expect("the data might be broken").is_type())
            .cloned() {
            let (s_guid, mut types, is_declaration) = {
                let s_ref = symbol.read().expect("the data might be broken");
                (s_ref.guid().to_string(), s_ref.types(), s_ref.is_declaration())
            };
            types = if is_declaration {
                types
            } else {
                symbol.read().expect("the data might be broken")
                    .get_linked_decl_guid()
                    .clone()
                    .map(|guid| self.symbols_by_guid.get(&guid))
                    .flatten()
                    .map(|s| s.read().expect("the data might be broken").types())
                    .unwrap_or_default()
            };
            for guid in types
                .iter()
                .filter_map(|t| t.guid.clone()) {
                self.type_guid_to_dependand_guids.entry(guid).or_default().push(s_guid.clone());
            }
        }
    }

    fn parse_single_file(
        &self,
        doc: &DocumentInfo,
        code: &str,
    ) -> Vec<AstSymbolInstanceArc> {
        let doc = match DocumentInfo::from_pathbuf_and_text(
            &doc.uri.to_file_path().unwrap_or_default(),
            &code.to_string(),
        ) {
            Ok(doc) => doc,
            Err(_) => {
                info!("Could not parse file {:?}", doc.uri.to_file_path().unwrap_or_default());
                return vec![];
            }
        };
        let symbols = AstIndex::parse(&doc).unwrap_or_default();
        self.resolve_types(&symbols);
        self.merge_usages_to_declarations(&symbols);
        symbols
    }
}


