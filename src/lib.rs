#![deny(unused)]
#![warn(clippy::pedantic)]
#![allow(
    clippy::module_name_repetitions,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::must_use_candidate,
    clippy::too_many_lines,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::cast_possible_wrap,
    clippy::struct_excessive_bools,
    clippy::similar_names,
    clippy::doc_markdown,
    clippy::items_after_statements,
    clippy::manual_let_else,
    clippy::unreadable_literal,
    clippy::float_cmp,
    clippy::wildcard_in_or_patterns,
    clippy::match_same_arms,
    clippy::match_wildcard_for_single_variants
)]

pub mod analytics;
pub mod attribution;
pub mod cli;
pub mod commands;
pub mod config;
pub mod core;
pub mod error;
pub mod feed;
pub mod git;
pub mod help;
pub mod hooks;
pub mod paths;
pub mod project;
pub mod project_config;
pub mod redact;
pub mod remote;
pub mod session;
pub mod setup;
pub mod taps;
pub mod tools;
pub mod trace;
pub mod tui;
pub mod utils;
