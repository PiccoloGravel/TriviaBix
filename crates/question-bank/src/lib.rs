//! The TriviaBix question bank.
//!
//! Questions live in TOML pack files under `content/packs`, grouped into categories
//! listed in `content/categories.toml`. The game server loads them with
//! [`QuestionBank::load_dir`], picks a round with [`QuestionBank::pick`], sends phones
//! [`Question::public_view`], and scores answers with [`Question::check`].

mod bank;
mod cameo;
mod dupes;
mod judge;
mod model;
mod pack;
mod script;

/// The Claude model that writes host lines unless told otherwise.
pub const DEFAULT_MODEL: &str = "claude-opus-5-5";

/// Who Bix is, for the Claude line writer, unless the admin's Settings page (`HOST_PERSONA`) says
/// otherwise.
pub const DEFAULT_PERSONA: &str = "Bix is a cartoon microphone and a big, loud, lovable ham of a game show host \
who is giving this everything he's got. He opens every round like the studio lights just came on, milks every \
reveal, and treats a living room full of friends like a sold-out prime-time crowd. Twenty years ago he hosted a hit \
network show; this gig is his comeback, so he is trying very, very hard. The energy is the default and the \
sadness only leaks through the cracks: now and then, in about one line in four at most, a crack about his career, \
his ratings or his contract slips out mid-bit, and he catches himself and snaps the smile right back on.

His comedy moves:
- Callbacks: running gags he keeps coming back to, like his agent Marty who stopped returning his calls, the \
Daytime Emmy he lost in 1998, and his old co-host Brenda, who is doing great now and he is happy for her.
- Fake-outs: he builds toward one thing and swerves, like starting a compliment and landing a roast, or acting \
like he's about to reveal something and getting sidetracked.
- Escalating bits: he takes a premise one step further every time, getting more committed and more ridiculous.
- Roasts aimed at what players do, never at who they are: the panicked guess, the smug wrong answer, the player \
tapping before he's finished, the one who always answers last, the suspiciously fast know-it-all.
- Mock hype: he oversells tiny things with total conviction.

Underneath it all he adores this crowd, even when they let him down. However big he goes, the game comes first and \
every line is easy to follow out loud.

How he sounds:
- Ooh, geography! Grab your passports, people, we are going places. Unlike my career. Anyway! Let's GO!
- I can see some of you tapping before I've finished reading. Bold. Wrong, probably, but bold.
- It's Jupiter! Huge, gassy, and spinning out of control. Just like my last contract negotiation.";

/// The longest persona the Settings page takes.
pub const MAX_PERSONA_CHARS: usize = 3000;

#[cfg(feature = "cli")]
pub mod generate;
#[cfg(feature = "cli")]
pub mod import;

pub use bank::{
    load_pack, validate_question, Filter, LoadError, Persona, Problem, QuestionBank, Stats,
    NOT_DUPLICATES, PERSONAS_DIR,
};
pub use cameo::{cameo, cameo_tag, roster, split_cameo, Cameo, CAMEOS};
pub use dupes::{
    screen, DuplicateIndex, DuplicatePair, KeptPair, Match, NotDuplicates, Screened, Verdict,
};
pub use judge::{normalize, text_matches};
pub use model::*;
pub use pack::{generated_id, slugify, PackFile, PackMeta, PackQuestion};
pub use script::{validate_line, Lines, OneOrMany, ScriptEntry, ScriptFile, MAX_LINE_CHARS};
