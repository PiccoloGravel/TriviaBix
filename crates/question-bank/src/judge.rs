//! Judging player answers and building the answer-free view sent to phones.

use rand::seq::SliceRandom;
use rand::Rng;

use crate::model::*;

impl Question {
    pub fn question_type(&self) -> QuestionType {
        self.kind.question_type()
    }

    /// Judge one player's answer. An answer of the wrong shape is simply wrong.
    pub fn check(&self, answer: &PlayerAnswer) -> Judgement {
        match (&self.kind, answer) {
            (QuestionKind::MultipleChoice { answer: right, .. }, PlayerAnswer::Choice(pick)) => {
                Judgement::binary(pick.trim() == right.trim())
            }
            (QuestionKind::TrueFalse { answer: right }, PlayerAnswer::Bool(pick)) => {
                Judgement::binary(pick == right)
            }
            (QuestionKind::TypeAnswer { answers }, PlayerAnswer::Text(text)) => {
                Judgement::binary(answers.iter().any(|a| text_matches(a, text)))
            }
            (
                QuestionKind::ClosestNumber {
                    answer: right,
                    tolerance,
                    ..
                },
                PlayerAnswer::Number(guess),
            ) => {
                if !guess.is_finite() {
                    return Judgement::binary(false);
                }
                let distance = (guess - right).abs();
                let scale = right.abs().max(1.0);
                Judgement {
                    correct: distance <= *tolerance,
                    score: (1.0 - distance / scale).clamp(0.0, 1.0),
                    distance: Some(distance),
                }
            }
            (QuestionKind::PutInOrder { items, .. }, PlayerAnswer::Order(order)) => {
                let in_place = items
                    .iter()
                    .zip(order)
                    .filter(|(want, got)| want.trim() == got.trim())
                    .count();
                let correct = in_place == items.len() && order.len() == items.len();
                Judgement {
                    correct,
                    score: in_place as f64 / items.len().max(1) as f64,
                    distance: None,
                }
            }
            _ => Judgement::binary(false),
        }
    }

    /// The question without its answer, with choices and order items shuffled.
    pub fn public_view<R: Rng + ?Sized>(&self, rng: &mut R) -> PublicQuestion {
        let kind = match &self.kind {
            QuestionKind::MultipleChoice {
                choices,
                keep_order,
                ..
            } => {
                let mut choices = choices.clone();
                if !keep_order {
                    choices.shuffle(rng);
                }
                PublicKind::MultipleChoice { choices }
            }
            QuestionKind::TrueFalse { .. } => PublicKind::TrueFalse,
            QuestionKind::TypeAnswer { .. } => PublicKind::TypeAnswer,
            QuestionKind::ClosestNumber { unit, .. } => {
                PublicKind::ClosestNumber { unit: unit.clone() }
            }
            QuestionKind::PutInOrder {
                items,
                first_label,
                last_label,
            } => {
                let mut shuffled = items.clone();
                // Never hand out the answer already in order.
                for _ in 0..8 {
                    shuffled.shuffle(rng);
                    if shuffled != *items || items.len() < 2 {
                        break;
                    }
                }
                PublicKind::PutInOrder {
                    items: shuffled,
                    first_label: first_label.clone(),
                    last_label: last_label.clone(),
                }
            }
        };
        PublicQuestion {
            id: self.id.clone(),
            category: self.category.clone(),
            difficulty: self.difficulty,
            prompt: self.prompt.clone(),
            kind,
        }
    }

    /// The correct answer as text for the TV and the host to read out.
    pub fn answer_text(&self) -> String {
        match &self.kind {
            QuestionKind::MultipleChoice { answer, .. } => answer.clone(),
            QuestionKind::TrueFalse { answer } => {
                if *answer { "True" } else { "False" }.to_string()
            }
            QuestionKind::TypeAnswer { answers } => answers.first().cloned().unwrap_or_default(),
            QuestionKind::ClosestNumber { answer, unit, .. } => {
                let n = format_number(*answer);
                match unit {
                    Some(u) => format!("{n} {u}"),
                    None => n,
                }
            }
            QuestionKind::PutInOrder { items, .. } => items.join(", "),
        }
    }
}

fn format_number(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e15 {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

/// Lowercase, fold common accents, turn `&` into "and", keep letters and digits, and drop
/// a leading "the", "a", or "an".
pub fn normalize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars().flat_map(char::to_lowercase) {
        let c = fold_accent(c);
        if c == '&' {
            out.push_str(" and ");
        } else if c.is_alphanumeric() {
            out.push(c);
        } else if c == '\'' || c == '’' || c == '.' {
            // "Rock 'n' Roll" and "Dr." should not split words.
        } else {
            out.push(' ');
        }
    }
    let words: Vec<&str> = out.split_whitespace().collect();
    let words = match words.first() {
        Some(&("the" | "a" | "an")) if words.len() > 1 => &words[1..],
        _ => &words[..],
    };
    words.join(" ")
}

fn fold_accent(c: char) -> char {
    match c {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' => 'a',
        'ç' => 'c',
        'è' | 'é' | 'ê' | 'ë' => 'e',
        'ì' | 'í' | 'î' | 'ï' => 'i',
        'ñ' => 'n',
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' => 'o',
        'ù' | 'ú' | 'û' | 'ü' => 'u',
        'ý' | 'ÿ' => 'y',
        other => other,
    }
}

/// Whether a typed answer matches an accepted answer, forgiving small typos.
pub fn text_matches(accepted: &str, typed: &str) -> bool {
    let a = normalize(accepted);
    let t = normalize(typed);
    if t.is_empty() {
        return false;
    }
    if a == t || a.replace(' ', "") == t.replace(' ', "") {
        return true;
    }
    // Numbers must be exact: "1985" is not "1986".
    if a.chars().any(|c| c.is_ascii_digit()) {
        return false;
    }
    let allowed = match a.chars().count() {
        0..=4 => 0,
        5..=8 => 1,
        _ => 2,
    };
    levenshtein(&a, &t) <= allowed
}

fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0; b.len() + 1];
    for i in 1..=a.len() {
        cur[0] = i;
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(kind: QuestionKind) -> Question {
        Question {
            id: "t".into(),
            category: "general".into(),
            tags: vec![],
            difficulty: Difficulty::Easy,
            rating: Rating::Pg,
            prompt: "?".into(),
            kind,
            fun_fact: None,
            host_quip: None,
            source: None,
        }
    }

    #[test]
    fn typed_answers_forgive_typos_and_articles() {
        assert!(text_matches("The Transformers", "transformers"));
        assert!(text_matches(
            "Smells Like Teen Spirit",
            "smells like teen sprit"
        ));
        assert!(text_matches("Piña Colada", "pina colada"));
        assert!(text_matches("Guns N' Roses", "guns n roses"));
        assert!(text_matches("AC/DC", "acdc"));
        assert!(!text_matches("MTV", "VH1"));
        assert!(!text_matches("Ken", "Kev"));
        assert!(!text_matches("1985", "1986"));
        assert!(!text_matches("Madonna", ""));
    }

    #[test]
    fn closest_number_reports_distance() {
        let question = q(QuestionKind::ClosestNumber {
            answer: 1989.0,
            unit: None,
            tolerance: 0.0,
        });
        let exact = question.check(&PlayerAnswer::Number(1989.0));
        assert!(exact.correct);
        let off = question.check(&PlayerAnswer::Number(1985.0));
        assert!(!off.correct);
        assert_eq!(off.distance, Some(4.0));
        assert!(off.score > 0.99);
    }

    #[test]
    fn put_in_order_gives_partial_credit() {
        let items: Vec<String> = ["a", "b", "c", "d"].map(String::from).to_vec();
        let question = q(QuestionKind::PutInOrder {
            items: items.clone(),
            first_label: None,
            last_label: None,
        });
        assert!(question.check(&PlayerAnswer::Order(items)).correct);
        let half = question.check(&PlayerAnswer::Order(
            ["a", "b", "d", "c"].map(String::from).to_vec(),
        ));
        assert!(!half.correct);
        assert_eq!(half.score, 0.5);
    }

    #[test]
    fn public_view_hides_answer_and_shuffles_order() {
        let items: Vec<String> = ["a", "b", "c"].map(String::from).to_vec();
        let question = q(QuestionKind::PutInOrder {
            items: items.clone(),
            first_label: None,
            last_label: None,
        });
        let mut rng = rand::thread_rng();
        for _ in 0..20 {
            match question.public_view(&mut rng).kind {
                PublicKind::PutInOrder { items: shown, .. } => assert_ne!(shown, items),
                _ => unreachable!(),
            }
        }
        let tf = q(QuestionKind::TrueFalse { answer: true });
        let json = serde_json::to_string(&tf.public_view(&mut rng)).unwrap();
        assert!(!json.contains("answer"), "{json}");
    }

    #[test]
    fn wrong_answer_shape_is_wrong() {
        let question = q(QuestionKind::TrueFalse { answer: true });
        assert!(!question.check(&PlayerAnswer::Text("true".into())).correct);
    }
}
