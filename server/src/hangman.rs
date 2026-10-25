//! Speed hangman puzzles: one shared phrase per round that every phone guesses at once.
//! Only ASCII letters are hidden; spaces, digits and punctuation are shown from the start.

/// Misses (wrong letters plus wrong solves) the whole room can make before the puzzle is lost.
pub const MAX_STRIKES: usize = 6;

/// What shows on the TV in place of a letter nobody has found yet.
pub const BLANK: char = '_';

#[derive(Debug, Clone)]
pub struct Puzzle {
    pub id: i64,
    pub category: String,
    pub answer: String,
    guessed: Vec<char>,
    pub strikes: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Guess {
    /// The letter is in the phrase this many times.
    Hit(usize),
    Miss,
    AlreadyGuessed,
    NotALetter,
}

impl Puzzle {
    pub fn new(id: i64, category: String, answer: String) -> Self {
        Self {
            id,
            category,
            answer,
            guessed: Vec::new(),
            strikes: 0,
        }
    }

    /// Guesses one letter. A miss costs the room a strike.
    pub fn guess(&mut self, letter: char) -> Guess {
        let letter = letter.to_ascii_uppercase();
        if !letter.is_ascii_uppercase() {
            return Guess::NotALetter;
        }
        if self.guessed.contains(&letter) {
            return Guess::AlreadyGuessed;
        }
        self.guessed.push(letter);
        let count = self
            .answer
            .chars()
            .filter(|c| c.to_ascii_uppercase() == letter)
            .count();
        if count == 0 {
            self.strikes += 1;
            Guess::Miss
        } else {
            Guess::Hit(count)
        }
    }

    /// Whether `attempt` spells the answer, ignoring case, spaces and punctuation.
    pub fn solves(&self, attempt: &str) -> bool {
        let attempt = squash(attempt);
        !attempt.is_empty() && attempt == squash(&self.answer)
    }

    pub fn strike(&mut self) {
        self.strikes += 1;
    }

    /// Letters in the phrase that are still blank.
    pub fn hidden(&self) -> usize {
        self.answer.chars().filter(|c| self.is_hidden(*c)).count()
    }

    pub fn is_complete(&self) -> bool {
        self.hidden() == 0
    }

    pub fn is_hanged(&self) -> bool {
        self.strikes >= MAX_STRIKES
    }

    /// The phrase with unfound letters replaced by [`BLANK`].
    pub fn mask(&self) -> String {
        self.answer
            .chars()
            .map(|c| if self.is_hidden(c) { BLANK } else { c })
            .collect()
    }

    pub fn hits(&self) -> String {
        self.guessed.iter().filter(|l| self.contains(**l)).collect()
    }

    pub fn misses(&self) -> String {
        self.guessed
            .iter()
            .filter(|l| !self.contains(**l))
            .collect()
    }

    fn contains(&self, letter: char) -> bool {
        self.answer
            .chars()
            .any(|c| c.to_ascii_uppercase() == letter)
    }

    fn is_hidden(&self, c: char) -> bool {
        c.is_ascii_alphabetic() && !self.guessed.contains(&c.to_ascii_uppercase())
    }
}

fn squash(s: &str) -> String {
    s.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn puzzle(answer: &str) -> Puzzle {
        Puzzle::new(1, "Movies".into(), answer.into())
    }

    #[test]
    fn letters_reveal_and_misses_strike() {
        let mut p = puzzle("Jaws 2!");
        assert_eq!(p.mask(), "____ 2!");
        assert_eq!(p.guess('a'), Guess::Hit(1));
        assert_eq!(p.guess('A'), Guess::AlreadyGuessed);
        assert_eq!(p.guess('z'), Guess::Miss);
        assert_eq!(p.guess('2'), Guess::NotALetter);
        assert_eq!(p.mask(), "_a__ 2!");
        assert_eq!(
            (p.hits().as_str(), p.misses().as_str(), p.strikes),
            ("A", "Z", 1)
        );
        for c in ['j', 'w', 's'] {
            p.guess(c);
        }
        assert!(p.is_complete());
    }

    #[test]
    fn solving_ignores_case_and_punctuation() {
        let p = puzzle("Don't Stop Believin'");
        assert!(p.solves("dont stop believin"));
        assert!(p.solves("  DON'T STOP BELIEVIN'  "));
        assert!(!p.solves("don't stop believing"));
        assert!(!p.solves("!!!"));
    }

    #[test]
    fn six_strikes_and_you_are_out() {
        let mut p = puzzle("Up");
        for c in "abcdef".chars() {
            assert!(!p.is_hanged());
            p.guess(c);
        }
        assert!(p.is_hanged());
        assert_eq!(p.hidden(), 2);
    }
}
