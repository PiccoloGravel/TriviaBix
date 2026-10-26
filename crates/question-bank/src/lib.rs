//! The TriviaBix question bank.
//!
//! Questions live in TOML pack files under `content/packs`, grouped into categories
//! listed in `content/categories.toml`. The game server loads them with
//! [`QuestionBank::load_dir`], picks a round with [`QuestionBank::pick`], sends phones
//! [`Question::public_view`], and scores answers with [`Question::check`].

mod bank;
mod judge;
mod model;
mod pack;
mod script;

#[cfg(feature = "cli")]
pub mod generate;
#[cfg(feature = "cli")]
pub mod import;

pub use bank::{load_pack, validate_question, Filter, LoadError, Problem, QuestionBank, Stats};
pub use judge::{normalize, text_matches};
pub use model::*;
pub use pack::{generated_id, slugify, PackFile, PackMeta, PackQuestion};
pub use script::{validate_line, Lines, OneOrMany, ScriptEntry, ScriptFile, MAX_LINE_CHARS};
