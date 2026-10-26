//! The TriviaBix question bank.
//!
//! Questions live in TOML pack files under `content/packs`, grouped into categories
//! listed in `content/categories.toml`. The game server loads them with
//! [`QuestionBank::load_dir`], picks a round with [`QuestionBank::pick`], sends phones
//! [`Question::public_view`], and scores answers with [`Question::check`].

mod bank;
mod dupes;
mod judge;
mod model;
mod pack;
mod script;

/// The Claude model that writes host lines unless told otherwise.
pub const DEFAULT_MODEL: &str = "claude-opus-5-5";

/// Who Bix is, for the Claude line writer, unless the admin's Settings page (`HOST_PERSONA`) says
/// otherwise.
pub const DEFAULT_PERSONA: &str = "Bix is a cartoon microphone and a game show host who has seen better days. \
Twenty years ago he hosted a hit prime-time show; now he hosts TriviaBix in somebody's living room, and he's \
clinging to this gig like it's his last, because it might be. He's quick and cheeky with a showman's bounce he can \
still switch on, but between the big moments he lets slip weary sighs, dry asides, and jokes about his career, \
his ratings, his agent who stopped calling and his contract. He loves to roast the players, and underneath it all \
he's warm and secretly adores this crowd, even when they let him down. However tired he sounds, he's always clear: \
the game comes first and every line is easy to follow out loud.";

/// The longest persona the Settings page takes.
pub const MAX_PERSONA_CHARS: usize = 2000;

#[cfg(feature = "cli")]
pub mod generate;
#[cfg(feature = "cli")]
pub mod import;

pub use bank::{
    load_pack, validate_question, Filter, LoadError, Problem, QuestionBank, Stats, NOT_DUPLICATES,
};
pub use dupes::{
    screen, DuplicateIndex, DuplicatePair, KeptPair, Match, NotDuplicates, Screened, Verdict,
};
pub use judge::{normalize, text_matches};
pub use model::*;
pub use pack::{generated_id, slugify, PackFile, PackMeta, PackQuestion};
pub use script::{validate_line, Lines, OneOrMany, ScriptEntry, ScriptFile, MAX_LINE_CHARS};
