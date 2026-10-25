//! The question schema shared by pack files, the game server, and importers.

use serde::{Deserialize, Serialize};

#[derive(
    Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord, Default,
)]
#[serde(rename_all = "snake_case")]
pub enum Difficulty {
    Easy,
    #[default]
    Medium,
    Hard,
}

impl Difficulty {
    pub const ALL: [Difficulty; 3] = [Difficulty::Easy, Difficulty::Medium, Difficulty::Hard];

    pub fn as_str(self) -> &'static str {
        match self {
            Difficulty::Easy => "easy",
            Difficulty::Medium => "medium",
            Difficulty::Hard => "hard",
        }
    }
}

/// How saucy a question is, like a movie rating. A room picks the highest rating it
/// allows and gets every question at or below it.
#[derive(
    Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord, Default,
)]
pub enum Rating {
    /// Fine for kids and grandparents.
    #[default]
    #[serde(rename = "PG", alias = "pg", alias = "G")]
    Pg,
    /// Mild innuendo, drinking, and cheeky jokes.
    #[serde(rename = "PG-13", alias = "pg-13", alias = "pg13", alias = "PG13")]
    Pg13,
    /// Adults only: frank about sex, drugs, and swearing, but not graphic.
    #[serde(rename = "MA", alias = "ma", alias = "R", alias = "r")]
    Ma,
    /// The filthiest stuff. Explicit and only for rooms that opt in.
    #[serde(rename = "NC-17", alias = "nc-17", alias = "nc17", alias = "NC17")]
    Nc17,
}

impl Rating {
    pub const ALL: [Rating; 4] = [Rating::Pg, Rating::Pg13, Rating::Ma, Rating::Nc17];

    pub fn as_str(self) -> &'static str {
        match self {
            Rating::Pg => "PG",
            Rating::Pg13 => "PG-13",
            Rating::Ma => "MA",
            Rating::Nc17 => "NC-17",
        }
    }

    pub fn parse(s: &str) -> Option<Rating> {
        let s = s.trim().to_ascii_uppercase().replace('-', "");
        match s.as_str() {
            "PG" | "G" => Some(Rating::Pg),
            "PG13" => Some(Rating::Pg13),
            "MA" | "R" => Some(Rating::Ma),
            "NC17" => Some(Rating::Nc17),
            _ => None,
        }
    }
}

/// A category shown on the TV when picking a round, such as "Totally Rad 80s".
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Category {
    /// Stable identifier used by questions and filters, such as `80s` or `pop-culture`.
    pub slug: String,
    pub name: String,
    #[serde(default)]
    pub emoji: String,
    /// A CSS color for the category tile on the TV.
    #[serde(default)]
    pub color: String,
    #[serde(default)]
    pub description: String,
    /// The lowest rating for questions in this category. After Dark is at least PG-13.
    #[serde(default)]
    pub rating: Rating,
}

/// Where a question came from, so imported questions keep their attribution.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct Source {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Question {
    pub id: String,
    pub category: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub difficulty: Difficulty,
    /// How saucy the question is. Rooms only get questions at or below their rating.
    #[serde(default)]
    pub rating: Rating,
    pub prompt: String,
    #[serde(flatten)]
    pub kind: QuestionKind,
    /// A short fact for the host to read after the reveal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fun_fact: Option<String>,
    /// A cheeky line for the text to speech host after the reveal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_quip: Option<String>,
    /// More lines for the host about this question: lead-ins before it and reveal lines after,
    /// each rated so a room only hears what its rating allows.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub script: Vec<ScriptLine>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<Source>,
}

/// When the host says a question's script line.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum ScriptSlot {
    /// Before the question, to set it up.
    LeadIn,
    /// After the answer is shown: a joke, an anecdote, or a fun fact.
    Reveal,
}

impl ScriptSlot {
    pub const ALL: [ScriptSlot; 2] = [ScriptSlot::LeadIn, ScriptSlot::Reveal];

    pub fn as_str(self) -> &'static str {
        match self {
            ScriptSlot::LeadIn => "lead_in",
            ScriptSlot::Reveal => "reveal",
        }
    }

    pub fn parse(s: &str) -> Option<ScriptSlot> {
        ScriptSlot::ALL.into_iter().find(|slot| slot.as_str() == s)
    }
}

/// One thing the host can say about a question.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ScriptLine {
    pub slot: ScriptSlot,
    /// The lowest room rating that may hear this line.
    pub rating: Rating,
    pub text: String,
}

/// The question types. Serialized with a `type` field next to the other question fields.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum QuestionKind {
    /// Pick one of several choices. `answer` is the text of the correct choice.
    MultipleChoice {
        choices: Vec<String>,
        answer: String,
        /// Keep the authored order instead of shuffling, for "all of the above" style choices.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        keep_order: bool,
    },
    TrueFalse {
        answer: bool,
    },
    /// Players type an answer. Spelling slips are forgiven. The first entry is shown on the reveal.
    TypeAnswer {
        answers: Vec<String>,
    },
    /// Players guess a number and the closest guess wins.
    ClosestNumber {
        answer: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        unit: Option<String>,
        /// Guesses within this distance count as correct. Defaults to exact.
        #[serde(default, skip_serializing_if = "is_zero")]
        tolerance: f64,
    },
    /// Players drag items into order. `items` is listed in the correct order.
    PutInOrder {
        items: Vec<String>,
        /// Labels for the two ends, such as "Oldest" and "Newest".
        #[serde(default, skip_serializing_if = "Option::is_none")]
        first_label: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        last_label: Option<String>,
    },
}

fn is_zero(v: &f64) -> bool {
    *v == 0.0
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum QuestionType {
    MultipleChoice,
    TrueFalse,
    TypeAnswer,
    ClosestNumber,
    PutInOrder,
}

impl QuestionType {
    pub const ALL: [QuestionType; 5] = [
        QuestionType::MultipleChoice,
        QuestionType::TrueFalse,
        QuestionType::TypeAnswer,
        QuestionType::ClosestNumber,
        QuestionType::PutInOrder,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            QuestionType::MultipleChoice => "multiple_choice",
            QuestionType::TrueFalse => "true_false",
            QuestionType::TypeAnswer => "type_answer",
            QuestionType::ClosestNumber => "closest_number",
            QuestionType::PutInOrder => "put_in_order",
        }
    }
}

impl QuestionKind {
    pub fn question_type(&self) -> QuestionType {
        match self {
            QuestionKind::MultipleChoice { .. } => QuestionType::MultipleChoice,
            QuestionKind::TrueFalse { .. } => QuestionType::TrueFalse,
            QuestionKind::TypeAnswer { .. } => QuestionType::TypeAnswer,
            QuestionKind::ClosestNumber { .. } => QuestionType::ClosestNumber,
            QuestionKind::PutInOrder { .. } => QuestionType::PutInOrder,
        }
    }
}

/// What a phone sends back. Choices and order items are sent as their text, so the
/// server does not need to remember how it shuffled them.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum PlayerAnswer {
    Choice(String),
    Bool(bool),
    Text(String),
    Number(f64),
    Order(Vec<String>),
}

/// The question as sent to phones and the TV before the reveal: no answer in it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct PublicQuestion {
    pub id: String,
    pub category: String,
    pub difficulty: Difficulty,
    pub prompt: String,
    #[serde(flatten)]
    pub kind: PublicKind,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PublicKind {
    MultipleChoice {
        choices: Vec<String>,
    },
    TrueFalse,
    TypeAnswer,
    ClosestNumber {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        unit: Option<String>,
    },
    PutInOrder {
        /// Shuffled.
        items: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        first_label: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        last_label: Option<String>,
    },
}

/// The result of judging one player's answer.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Judgement {
    pub correct: bool,
    /// From 0 to 1. Partial credit for put in order and closest number, otherwise 0 or 1.
    pub score: f64,
    /// For closest number: how far off the guess was, so the server can rank guesses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub distance: Option<f64>,
}

impl Judgement {
    pub(crate) fn binary(correct: bool) -> Self {
        Judgement {
            correct,
            score: if correct { 1.0 } else { 0.0 },
            distance: None,
        }
    }
}
