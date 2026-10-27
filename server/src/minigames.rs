//! Mini games between trivia rounds: short party games played on the phones while the TV shows
//! the action. Each one has a rules card, the game itself, and results worth a few bonus points.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use rand::Rng;
use rand::seq::{IndexedRandom, SliceRandom};
use tokio::time::Instant;

use crate::protocol::{MiniEntryView, MinigameView};

/// How long the rules card stays up, at least.
pub const RULES: Duration = Duration::from_secs(6);
/// How long the results stay up, at least.
pub const RESULTS: Duration = Duration::from_secs(7);
const MASH: Duration = Duration::from_secs(10);
const CLOSEST: Duration = Duration::from_secs(20);
/// Quick Draw waits a random time in this range before GO.
const READY_MS: (u64, u64) = (2_000, 5_000);
const DRAW: Duration = Duration::from_secs(3);
/// Taps closer together than this don't count, so nobody wins with a script.
const MIN_TAP_GAP: Duration = Duration::from_millis(50);
/// Button Mash sends the room's tap counts at most this often.
const PUSH_EVERY: Duration = Duration::from_millis(300);
/// Bonus points for first, second and third place.
pub const PLACES: [i64; 3] = [300, 200, 100];
/// Extra for guessing a Closest Guess answer exactly.
pub const EXACT_BONUS: i64 = 200;
const MAX_GUESS: i64 = 1_000_000_000;
/// Stop the Clock: how long players have, and the moment they're aiming for.
const STOP_CLOCK: Duration = Duration::from_secs(20);
pub const TARGET_MS: i64 = 10_000;
const MAJORITY: Duration = Duration::from_secs(15);
const LOWBALL: Duration = Duration::from_secs(15);
/// Lowball: numbers run from 1 to this.
pub const LOWBALL_MAX: i64 = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Tap as many times as you can.
    Mash,
    /// Guess a number; nearest wins.
    Closest,
    /// Wait for GO, then tap first. Tapping early is a false start.
    QuickDraw,
    /// Tap when you think 10 seconds have passed; the clock disappears after 3.
    StopClock,
    /// Pick the answer you think most people will pick.
    Majority,
    /// Lowest number nobody else picked wins.
    Lowball,
}

pub const KINDS: [Kind; 6] = [
    Kind::Mash,
    Kind::Closest,
    Kind::QuickDraw,
    Kind::StopClock,
    Kind::Majority,
    Kind::Lowball,
];

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Mash => "mash",
            Kind::Closest => "closest",
            Kind::QuickDraw => "quick_draw",
            Kind::StopClock => "stop_clock",
            Kind::Majority => "majority",
            Kind::Lowball => "lowball",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Kind::Mash => "Button Mash",
            Kind::Closest => "Closest Guess",
            Kind::QuickDraw => "Quick Draw",
            Kind::StopClock => "Stop the Clock",
            Kind::Majority => "Majority Rules",
            Kind::Lowball => "Lowball",
        }
    }

    pub fn rules(self) -> &'static str {
        match self {
            Kind::Mash => "Tap your phone as fast as you can for 10 seconds. Most taps wins.",
            Kind::Closest => "Guess the number. Closest guess wins, and spot on pays extra.",
            Kind::QuickDraw => "Wait for GO, then tap. Fastest wins. Tap early and you're out!",
            Kind::StopClock => {
                "Tap when you think exactly 10 seconds have passed. The clock vanishes after 3!"
            }
            Kind::Majority => {
                "Pick what you think MOST people will pick. The biggest group scores."
            }
            Kind::Lowball => {
                "Pick a number from 1 to 20. The lowest number nobody else picked wins."
            }
        }
    }

    /// The host's line announcing it.
    pub fn event(self) -> &'static str {
        match self {
            Kind::Mash => "mini_mash",
            Kind::Closest => "mini_closest",
            Kind::QuickDraw => "mini_quickdraw",
            Kind::StopClock => "mini_stopclock",
            Kind::Majority => "mini_majority",
            Kind::Lowball => "mini_lowball",
        }
    }
}

/// A Majority Rules question.
pub struct Poll {
    pub prompt: &'static str,
    pub options: &'static [&'static str],
}

/// Safe for any room, like the estimates.
pub const POLLS: &[Poll] = &[
    Poll {
        prompt: "Pizza or tacos?",
        options: &["Pizza", "Tacos"],
    },
    Poll {
        prompt: "Cats or dogs?",
        options: &["Cats", "Dogs"],
    },
    Poll {
        prompt: "Beach or mountains?",
        options: &["Beach", "Mountains"],
    },
    Poll {
        prompt: "Pancakes or waffles?",
        options: &["Pancakes", "Waffles"],
    },
    Poll {
        prompt: "Morning person or night owl?",
        options: &["Morning person", "Night owl"],
    },
    Poll {
        prompt: "Coffee, tea, or neither?",
        options: &["Coffee", "Tea", "Neither"],
    },
    Poll {
        prompt: "Sweet or salty snacks?",
        options: &["Sweet", "Salty"],
    },
    Poll {
        prompt: "Call or text?",
        options: &["Call", "Text"],
    },
    Poll {
        prompt: "Pineapple on pizza?",
        options: &["Yes please", "Absolutely not"],
    },
    Poll {
        prompt: "Is a hot dog a sandwich?",
        options: &["Yes", "No"],
    },
    Poll {
        prompt: "Toilet paper: over or under?",
        options: &["Over", "Under"],
    },
    Poll {
        prompt: "Window seat or aisle seat?",
        options: &["Window", "Aisle"],
    },
    Poll {
        prompt: "Time machine: past or future?",
        options: &["Past", "Future"],
    },
    Poll {
        prompt: "Superpower: flying or invisibility?",
        options: &["Flying", "Invisibility"],
    },
    Poll {
        prompt: "Pirates, ninjas or robots?",
        options: &["Pirates", "Ninjas", "Robots"],
    },
    Poll {
        prompt: "Summer or winter?",
        options: &["Summer", "Winter"],
    },
    Poll {
        prompt: "Movie night: theater or couch?",
        options: &["Theater", "Couch"],
    },
    Poll {
        prompt: "Fries: curly, waffle or straight?",
        options: &["Curly", "Waffle", "Straight"],
    },
    Poll {
        prompt: "Chocolate: milk, dark or white?",
        options: &["Milk", "Dark", "White"],
    },
    Poll {
        prompt: "Road trip or flight?",
        options: &["Road trip", "Flight"],
    },
    Poll {
        prompt: "Ketchup or mustard?",
        options: &["Ketchup", "Mustard"],
    },
    Poll {
        prompt: "Books or movies?",
        options: &["Books", "Movies"],
    },
    Poll {
        prompt: "Shower: morning or night?",
        options: &["Morning", "Night"],
    },
    Poll {
        prompt: "Cereal: milk first or cereal first?",
        options: &["Milk first", "Cereal first"],
    },
    Poll {
        prompt: "Zombies or aliens?",
        options: &["Zombies", "Aliens"],
    },
    Poll {
        prompt: "Best pet name: Biscuit, Noodle or Captain?",
        options: &["Biscuit", "Noodle", "Captain"],
    },
];

/// A Closest Guess question.
pub struct Estimate {
    pub prompt: &'static str,
    pub answer: i64,
    pub unit: &'static str,
}

/// Safe for any room, so every rating can play them.
pub const ESTIMATES: &[Estimate] = &[
    Estimate {
        prompt: "How many keys are on a standard piano?",
        answer: 88,
        unit: "keys",
    },
    Estimate {
        prompt: "How many bones are in an adult human body?",
        answer: 206,
        unit: "bones",
    },
    Estimate {
        prompt: "How tall is the Eiffel Tower, in metres, to the tip?",
        answer: 330,
        unit: "metres",
    },
    Estimate {
        prompt: "In what year did the Titanic sink?",
        answer: 1912,
        unit: "",
    },
    Estimate {
        prompt: "How many member countries does the United Nations have?",
        answer: 193,
        unit: "countries",
    },
    Estimate {
        prompt: "In what year did people first walk on the Moon?",
        answer: 1969,
        unit: "",
    },
    Estimate {
        prompt: "How many teeth does an adult have, wisdom teeth included?",
        answer: 32,
        unit: "teeth",
    },
    Estimate {
        prompt: "How long is a marathon, in kilometres (rounded)?",
        answer: 42,
        unit: "km",
    },
    Estimate {
        prompt: "What's the boiling point of water in Fahrenheit?",
        answer: 212,
        unit: "°F",
    },
    Estimate {
        prompt: "How many Earth days does a year on Mars last?",
        answer: 687,
        unit: "days",
    },
    Estimate {
        prompt: "How many episodes of Friends were made?",
        answer: 236,
        unit: "episodes",
    },
    Estimate {
        prompt: "In what year did the first iPhone go on sale?",
        answer: 2007,
        unit: "",
    },
    Estimate {
        prompt: "How many time zones does Russia span?",
        answer: 11,
        unit: "time zones",
    },
    Estimate {
        prompt: "How tall is Mount Everest, in metres?",
        answer: 8849,
        unit: "metres",
    },
    Estimate {
        prompt: "How many Oscars did the movie Titanic win?",
        answer: 11,
        unit: "Oscars",
    },
    Estimate {
        prompt: "In what year did the Berlin Wall fall?",
        answer: 1989,
        unit: "",
    },
    Estimate {
        prompt: "How many elements are on the periodic table?",
        answer: 118,
        unit: "elements",
    },
    Estimate {
        prompt: "How many feet are in a mile?",
        answer: 5280,
        unit: "feet",
    },
    Estimate {
        prompt: "How many seconds are in a day?",
        answer: 86400,
        unit: "seconds",
    },
    Estimate {
        prompt: "How many stripes are on the US flag?",
        answer: 13,
        unit: "stripes",
    },
    Estimate {
        prompt: "In what year did the Eiffel Tower open?",
        answer: 1889,
        unit: "",
    },
    Estimate {
        prompt: "What's a perfect score in ten-pin bowling?",
        answer: 300,
        unit: "points",
    },
    Estimate {
        prompt: "How many Pokémon were in the original Pokédex?",
        answer: 151,
        unit: "Pokémon",
    },
    Estimate {
        prompt: "How many years did the Hundred Years' War actually last?",
        answer: 116,
        unit: "years",
    },
    Estimate {
        prompt: "How many hearts does an octopus have?",
        answer: 3,
        unit: "hearts",
    },
    Estimate {
        prompt: "How many squares are on a chessboard?",
        answer: 64,
        unit: "squares",
    },
    Estimate {
        prompt: "How many minutes of playing time are in an NBA game?",
        answer: 48,
        unit: "minutes",
    },
    Estimate {
        prompt: "How many symphonies did Beethoven finish?",
        answer: 9,
        unit: "symphonies",
    },
    Estimate {
        prompt: "How many US presidents have there been, counting Grover Cleveland once?",
        answer: 45,
        unit: "presidents",
    },
    Estimate {
        prompt: "How many minutes does it take sunlight to reach Earth (rounded)?",
        answer: 8,
        unit: "minutes",
    },
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Rules,
    /// Quick Draw only: waiting for GO.
    Ready,
    Play,
    Results,
}

impl Step {
    fn name(self) -> &'static str {
        match self {
            Step::Rules => "rules",
            Step::Ready => "ready",
            Step::Play => "play",
            Step::Results => "results",
        }
    }
}

/// What a tap did.
pub enum Tapped {
    /// Counted; worth sending everyone the new state.
    Push,
    /// Counted, but the room was updated a moment ago.
    Quiet,
}

/// One mini game in progress.
pub struct Minigame {
    pub kind: Kind,
    pub step: Step,
    estimate: Option<&'static Estimate>,
    poll: Option<&'static Poll>,
    taps: HashMap<u32, u32>,
    last_tap: HashMap<u32, Instant>,
    last_push: Option<Instant>,
    guesses: HashMap<u32, i64>,
    go_at: Option<Instant>,
    reactions: HashMap<u32, Duration>,
    false_starts: HashSet<u32>,
    /// Bonus points won, once the results are in.
    points: HashMap<u32, i64>,
}

impl Minigame {
    /// `question` is the index into [`ESTIMATES`] for a Closest Guess, or into [`POLLS`] for
    /// Majority Rules.
    pub fn new(kind: Kind, question: Option<usize>) -> Self {
        Minigame {
            kind,
            step: Step::Rules,
            estimate: question
                .filter(|_| kind == Kind::Closest)
                .and_then(|i| ESTIMATES.get(i)),
            poll: question
                .filter(|_| kind == Kind::Majority)
                .and_then(|i| POLLS.get(i)),
            taps: HashMap::new(),
            last_tap: HashMap::new(),
            last_push: None,
            guesses: HashMap::new(),
            go_at: None,
            reactions: HashMap::new(),
            false_starts: HashSet::new(),
            points: HashMap::new(),
        }
    }

    /// Picks a Closest Guess question the room hasn't had yet, forgetting the used ones once
    /// they've all been asked.
    pub fn pick_estimate(used: &mut Vec<usize>, rng: &mut impl Rng) -> usize {
        pick_unused(used, ESTIMATES.len(), rng)
    }

    /// Picks a Majority Rules question the same way.
    pub fn pick_poll(used: &mut Vec<usize>, rng: &mut impl Rng) -> usize {
        pick_unused(used, POLLS.len(), rng)
    }
}

/// An index below `len` that isn't in `used` (forgetting `used` once it holds them all).
fn pick_unused(used: &mut Vec<usize>, len: usize, rng: &mut impl Rng) -> usize {
    if used.len() >= len {
        used.clear();
    }
    let fresh: Vec<usize> = (0..len).filter(|i| !used.contains(i)).collect();
    let i = *fresh.choose(rng).unwrap_or(&0);
    used.push(i);
    i
}

impl Minigame {
    /// Ends the rules card. Returns how long the next step lasts.
    pub fn start(&mut self, rng: &mut impl Rng) -> Duration {
        match self.kind {
            Kind::Mash => {
                self.step = Step::Play;
                MASH
            }
            Kind::Closest => {
                self.step = Step::Play;
                CLOSEST
            }
            Kind::QuickDraw => {
                self.step = Step::Ready;
                Duration::from_millis(rng.random_range(READY_MS.0..=READY_MS.1))
            }
            Kind::StopClock => {
                self.step = Step::Play;
                self.go_at = Some(Instant::now());
                STOP_CLOCK
            }
            Kind::Majority => {
                self.step = Step::Play;
                MAJORITY
            }
            Kind::Lowball => {
                self.step = Step::Play;
                LOWBALL
            }
        }
    }

    /// Quick Draw: GO! Returns how long players have to tap.
    pub fn go(&mut self, now: Instant) -> Duration {
        self.step = Step::Play;
        self.go_at = Some(now);
        DRAW
    }

    pub fn tap(&mut self, pid: u32, now: Instant) -> Result<Tapped, &'static str> {
        match (self.kind, self.step) {
            (Kind::Mash, Step::Play) => {
                if self
                    .last_tap
                    .get(&pid)
                    .is_some_and(|t| now.saturating_duration_since(*t) < MIN_TAP_GAP)
                {
                    return Ok(Tapped::Quiet);
                }
                self.last_tap.insert(pid, now);
                *self.taps.entry(pid).or_default() += 1;
                if self
                    .last_push
                    .is_some_and(|t| now.saturating_duration_since(t) < PUSH_EVERY)
                {
                    return Ok(Tapped::Quiet);
                }
                self.last_push = Some(now);
                Ok(Tapped::Push)
            }
            (Kind::QuickDraw, Step::Ready) => {
                if self.false_starts.insert(pid) {
                    Ok(Tapped::Push)
                } else {
                    Ok(Tapped::Quiet)
                }
            }
            (Kind::QuickDraw, Step::Play) => {
                if self.false_starts.contains(&pid) {
                    return Err("False start! You're out of this one.");
                }
                if self.reactions.contains_key(&pid) {
                    return Ok(Tapped::Quiet);
                }
                let go = self.go_at.unwrap_or(now);
                self.reactions
                    .insert(pid, now.saturating_duration_since(go));
                Ok(Tapped::Push)
            }
            (Kind::StopClock, Step::Play) => {
                if self.reactions.contains_key(&pid) {
                    return Ok(Tapped::Quiet);
                }
                let go = self.go_at.unwrap_or(now);
                self.reactions
                    .insert(pid, now.saturating_duration_since(go));
                Ok(Tapped::Push)
            }
            (_, Step::Rules | Step::Ready) => Err("Not yet! Wait for it…"),
            _ => Err("Not taking taps right now."),
        }
    }

    pub fn guess(&mut self, pid: u32, value: i64) -> Result<(), &'static str> {
        let range = match self.kind {
            Kind::Closest => 0..=MAX_GUESS,
            Kind::Majority => 0..=self.poll.map_or(0, |p| p.options.len() as i64 - 1),
            Kind::Lowball => 1..=LOWBALL_MAX,
            _ => return Err("Not taking guesses right now."),
        };
        if self.step != Step::Play {
            return Err("Not taking guesses right now.");
        }
        if !range.contains(&value) {
            return Err("That's out of range.");
        }
        if self.guesses.contains_key(&pid) {
            return Err("You already locked in a guess.");
        }
        self.guesses.insert(pid, value);
        Ok(())
    }

    /// A player has nothing left to do in this game.
    fn done(&self, pid: u32) -> bool {
        match self.kind {
            Kind::Mash => false,
            Kind::Closest | Kind::Majority | Kind::Lowball => self.guesses.contains_key(&pid),
            Kind::StopClock => self.reactions.contains_key(&pid),
            Kind::QuickDraw => {
                self.reactions.contains_key(&pid) || self.false_starts.contains(&pid)
            }
        }
    }

    /// Everyone playing is done, so the game can end early. Button Mash always runs its clock.
    pub fn all_done(&self, players: &[u32]) -> bool {
        self.step == Step::Play && !players.is_empty() && players.iter().all(|&pid| self.done(pid))
    }

    /// The number each player's result is ranked by: taps, the guess, or reaction time in ms.
    fn value(&self, pid: u32) -> Option<i64> {
        match self.kind {
            Kind::Mash => self.taps.get(&pid).map(|&n| n as i64),
            Kind::Closest | Kind::Majority | Kind::Lowball => self.guesses.get(&pid).copied(),
            Kind::QuickDraw | Kind::StopClock => {
                self.reactions.get(&pid).map(|d| d.as_millis() as i64)
            }
        }
    }

    /// How a result ranks; lower is better. `None` doesn't place.
    fn rank_key(&self, pid: u32) -> Option<i64> {
        let v = self.value(pid)?;
        match self.kind {
            Kind::Mash => (v > 0).then_some(-v),
            Kind::Closest => Some((v - self.estimate.map_or(0, |e| e.answer)).abs()),
            Kind::QuickDraw => Some(v),
            Kind::StopClock => Some((v - TARGET_MS).abs()),
            // A number someone else also picked doesn't count.
            Kind::Lowball => (self.guesses.values().filter(|&&g| g == v).count() == 1).then_some(v),
            // The biggest groups all win; the bigger your group, the better.
            Kind::Majority => Some(-(self.guesses.values().filter(|&&g| g == v).count() as i64)),
        }
    }

    /// Ends the game and works out the bonus points. Returns the winners (sharing first place)
    /// and every player's points.
    pub fn finish(&mut self, players: &[u32]) -> (Vec<u32>, Vec<(u32, i64)>) {
        self.step = Step::Results;
        let mut keyed: Vec<(u32, i64)> = players
            .iter()
            .filter_map(|&pid| self.rank_key(pid).map(|k| (pid, k)))
            .collect();
        keyed.sort_by_key(|&(_, k)| k);
        if self.kind == Kind::Majority {
            return self.finish_majority(players, &keyed);
        }
        let mut winners = Vec::new();
        let mut rank = 0;
        for (i, &(pid, key)) in keyed.iter().enumerate() {
            if i == 0 || keyed[i - 1].1 != key {
                rank = i;
            }
            let mut points = PLACES.get(rank).copied().unwrap_or(0);
            if self.kind == Kind::Closest && key == 0 {
                points += EXACT_BONUS;
            }
            if rank == 0 {
                winners.push(pid);
            }
            self.points.insert(pid, points);
        }
        let all = players
            .iter()
            .map(|&pid| (pid, self.points.get(&pid).copied().unwrap_or(0)))
            .collect();
        (winners, all)
    }

    /// Majority Rules: everyone in the biggest group (or groups) scores; nobody does when every
    /// pick is on its own.
    fn finish_majority(
        &mut self,
        players: &[u32],
        keyed: &[(u32, i64)],
    ) -> (Vec<u32>, Vec<(u32, i64)>) {
        let best = keyed.first().map_or(0, |k| k.1);
        let alone = best > -2 && keyed.len() > 1;
        let winners: Vec<u32> = keyed
            .iter()
            .filter(|k| k.1 == best && !alone)
            .map(|k| k.0)
            .collect();
        for &pid in &winners {
            self.points.insert(pid, PLACES[0]);
        }
        let all = players
            .iter()
            .map(|&pid| (pid, self.points.get(&pid).copied().unwrap_or(0)))
            .collect();
        (winners, all)
    }

    /// What the TV and phones see. `viewer` is the phone asking, so a Closest Guess only shows
    /// each player their own number until the results.
    pub fn view(&self, players: &[u32], viewer: Option<u32>) -> MinigameView {
        let results = self.step == Step::Results;
        let mut entries: Vec<MiniEntryView> = players
            .iter()
            .map(|&pid| {
                let hidden = matches!(
                    self.kind,
                    Kind::Closest | Kind::StopClock | Kind::Majority | Kind::Lowball
                );
                let secret = hidden && !results && viewer != Some(pid);
                MiniEntryView {
                    player_id: pid,
                    value: self.value(pid).filter(|_| !secret),
                    done: self.done(pid),
                    false_start: self.false_starts.contains(&pid),
                    points: self.points.get(&pid).copied().unwrap_or(0),
                }
            })
            .collect();
        if results {
            entries.sort_by_key(|e| (-e.points, self.rank_key(e.player_id).unwrap_or(i64::MAX)));
        }
        MinigameView {
            kind: self.kind.name(),
            title: self.kind.title(),
            rules: self.kind.rules(),
            step: self.step.name(),
            prompt: self
                .estimate
                .map(|e| e.prompt)
                .or(self.poll.map(|p| p.prompt)),
            unit: self.estimate.map(|e| e.unit),
            options: self.poll.map_or(&[][..], |p| p.options),
            max_pick: (self.kind == Kind::Lowball).then_some(LOWBALL_MAX),
            target_ms: (self.kind == Kind::StopClock).then_some(TARGET_MS),
            answer: self.estimate.filter(|_| results).map(|e| e.answer),
            entries,
        }
    }
}

/// The mini games for one trivia game, in a random order so they don't repeat until all have
/// been played.
pub fn shuffled(rng: &mut impl Rng) -> Vec<Kind> {
    let mut kinds = KINDS.to_vec();
    kinds.shuffle(rng);
    kinds
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mash_counts_taps_but_not_too_fast() {
        let mut m = Minigame::new(Kind::Mash, None);
        let t0 = Instant::now();
        assert!(m.tap(1, t0).is_err(), "no taps on the rules card");
        m.start(&mut rand::rng());
        for i in 0..10 {
            m.tap(1, t0 + Duration::from_millis(i * 100)).unwrap();
            // A second tap 10ms later is too fast to count.
            m.tap(1, t0 + Duration::from_millis(i * 100 + 10)).unwrap();
        }
        for i in 0..4 {
            m.tap(2, t0 + Duration::from_millis(i * 100)).unwrap();
        }
        assert!(!m.all_done(&[1, 2, 3]), "button mash runs its clock");
        let (winners, points) = m.finish(&[1, 2, 3]);
        assert_eq!(winners, vec![1]);
        assert_eq!(points, vec![(1, 300), (2, 200), (3, 0)]);
        let view = m.view(&[1, 2, 3], None);
        assert_eq!(view.entries[0].value, Some(10));
    }

    #[test]
    fn closest_guess_ranks_by_distance_and_hides_guesses() {
        let i = ESTIMATES.iter().position(|e| e.answer == 88).unwrap();
        let mut m = Minigame::new(Kind::Closest, Some(i));
        m.start(&mut rand::rng());
        m.guess(1, 100).unwrap();
        m.guess(2, 88).unwrap();
        assert!(m.guess(2, 90).is_err(), "one guess each");
        assert!(m.guess(3, -5).is_err());
        assert!(!m.all_done(&[1, 2, 3]));
        m.guess(3, 76).unwrap();
        assert!(m.all_done(&[1, 2, 3]));
        let view = m.view(&[1, 2, 3], Some(1));
        assert_eq!(view.entries[0].value, Some(100), "you see your own guess");
        assert_eq!(view.entries[1].value, None, "but nobody else's");
        assert_eq!(view.answer, None);
        let (winners, points) = m.finish(&[1, 2, 3]);
        assert_eq!(winners, vec![2]);
        // 1 and 3 are both 12 away, so they share second.
        assert_eq!(points, vec![(1, 200), (2, 300 + EXACT_BONUS), (3, 200)]);
        let view = m.view(&[1, 2, 3], None);
        assert_eq!(view.answer, Some(88));
        assert_eq!(view.entries[0].player_id, 2);
    }

    #[test]
    fn quick_draw_punishes_false_starts() {
        let mut m = Minigame::new(Kind::QuickDraw, None);
        let len = m.start(&mut rand::rng());
        assert!(
            len >= Duration::from_millis(READY_MS.0) && len <= Duration::from_millis(READY_MS.1)
        );
        let t0 = Instant::now();
        m.tap(3, t0).unwrap();
        m.go(t0);
        assert!(m.tap(3, t0 + Duration::from_millis(100)).is_err());
        m.tap(1, t0 + Duration::from_millis(400)).unwrap();
        m.tap(2, t0 + Duration::from_millis(250)).unwrap();
        assert!(m.all_done(&[1, 2, 3]));
        let (winners, points) = m.finish(&[1, 2, 3]);
        assert_eq!(winners, vec![2]);
        assert_eq!(points, vec![(1, 200), (2, 300), (3, 0)]);
        assert!(m.view(&[1, 2, 3], None).entries[2].false_start);
    }

    #[test]
    fn stop_the_clock_is_closest_to_ten_seconds() {
        let mut m = Minigame::new(Kind::StopClock, None);
        m.start(&mut rand::rng());
        let t0 = m.go_at.unwrap();
        m.tap(1, t0 + Duration::from_millis(9_000)).unwrap();
        m.tap(2, t0 + Duration::from_millis(10_300)).unwrap();
        m.tap(2, t0 + Duration::from_millis(12_000)).unwrap();
        assert!(!m.all_done(&[1, 2, 3]));
        assert_eq!(
            m.view(&[1, 2, 3], None).entries[0].value,
            None,
            "no peeking"
        );
        m.tap(3, t0 + Duration::from_millis(15_000)).unwrap();
        assert!(m.all_done(&[1, 2, 3]));
        let (winners, points) = m.finish(&[1, 2, 3]);
        assert_eq!(winners, vec![2]);
        assert_eq!(points, vec![(1, 200), (2, 300), (3, 100)]);
    }

    #[test]
    fn majority_rules_pays_the_biggest_group() {
        let i = POLLS.iter().position(|p| p.options.len() == 3).unwrap();
        let mut m = Minigame::new(Kind::Majority, Some(i));
        m.start(&mut rand::rng());
        assert!(m.guess(1, 3).is_err(), "only the options");
        m.guess(1, 0).unwrap();
        m.guess(2, 2).unwrap();
        m.guess(3, 2).unwrap();
        m.guess(4, 1).unwrap();
        assert_eq!(m.view(&[1, 2, 3, 4], None).options.len(), 3);
        let (winners, points) = m.finish(&[1, 2, 3, 4]);
        assert_eq!(winners, vec![2, 3]);
        assert_eq!(points, vec![(1, 0), (2, 300), (3, 300), (4, 0)]);

        // Everyone on their own: nobody scores.
        let mut m = Minigame::new(Kind::Majority, Some(i));
        m.start(&mut rand::rng());
        m.guess(1, 0).unwrap();
        m.guess(2, 1).unwrap();
        let (winners, _) = m.finish(&[1, 2]);
        assert!(winners.is_empty());
    }

    #[test]
    fn lowball_wins_with_the_lowest_unique_number() {
        let mut m = Minigame::new(Kind::Lowball, None);
        m.start(&mut rand::rng());
        assert!(m.guess(1, 0).is_err() && m.guess(1, 21).is_err());
        m.guess(1, 1).unwrap();
        m.guess(2, 1).unwrap();
        m.guess(3, 4).unwrap();
        m.guess(4, 2).unwrap();
        let (winners, points) = m.finish(&[1, 2, 3, 4]);
        assert_eq!(winners, vec![4]);
        assert_eq!(points, vec![(1, 0), (2, 0), (3, 200), (4, 300)]);
    }

    #[test]
    fn estimates_dont_repeat_until_all_are_used() {
        let mut used = Vec::new();
        let mut rng = rand::rng();
        let mut seen = HashSet::new();
        for _ in 0..ESTIMATES.len() {
            assert!(seen.insert(Minigame::pick_estimate(&mut used, &mut rng)));
        }
        Minigame::pick_estimate(&mut used, &mut rng);
        assert_eq!(used.len(), 1);
    }
}
