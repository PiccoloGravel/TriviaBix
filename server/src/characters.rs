//! The characters players pick from when they join. Each one can only be
//! used by one player per room.

use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct Character {
    pub id: &'static str,
    pub emoji: &'static str,
    pub name: &'static str,
}

const fn c(id: &'static str, emoji: &'static str, name: &'static str) -> Character {
    Character { id, emoji, name }
}

pub const CHARACTERS: &[Character] = &[
    c("fox", "🦊", "Sly Fox"),
    c("trex", "🦖", "T-Rex"),
    c("octopus", "🐙", "Octo"),
    c("robot", "🤖", "Robot"),
    c("alien", "👽", "Alien"),
    c("unicorn", "🦄", "Unicorn"),
    c("frog", "🐸", "Frog"),
    c("panda", "🐼", "Panda"),
    c("flamingo", "🦩", "Flamingo"),
    c("taco", "🌮", "Taco"),
    c("pizza", "🍕", "Pizza"),
    c("ghost", "👻", "Ghost"),
    c("wizard", "🧙", "Wizard"),
    c("sloth", "🦥", "Sloth"),
    c("penguin", "🐧", "Penguin"),
    c("avocado", "🥑", "Avocado"),
    c("cowboy", "🤠", "Cowboy"),
    c("clown", "🤡", "Clown"),
    c("poop", "💩", "Poo"),
    c("devil", "😈", "Lil Devil"),
    c("shark", "🦈", "Shark"),
    c("sheep", "🐑", "Sheep"),
    c("donut", "🍩", "Donut"),
    c("cactus", "🌵", "Cactus"),
];

pub fn exists(id: &str) -> bool {
    CHARACTERS.iter().any(|c| c.id == id)
}
