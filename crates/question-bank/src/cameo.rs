//! Character cameos: characters Bix breaks into for a single line, like a noir detective for a
//! crime question or a fast-talking auctioneer for Closest Guess. A line in character starts with
//! the character's tag, `[as:noir] The dame said Paris. The dame was wrong.`; the TV puts Bix in
//! costume for it and plays it with the character's voice style. The tag is never spoken or shown.

/// One character Bix can play.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cameo {
    /// What goes in the tag: `[as:<slug>]`.
    pub slug: &'static str,
    /// Shown on the TV as "Bix as <name>".
    pub name: &'static str,
    /// The costume Bix wears on the TV.
    pub emoji: &'static str,
    /// Who the character is, for the line writer.
    pub who: &'static str,
    /// When the character fits, for the line writer.
    pub fits: &'static str,
    /// How fast the TV plays the line, times the usual speed.
    pub rate: f32,
    /// Whether the speed change shifts the pitch too, so slow sounds deep and fast sounds high.
    pub shift: bool,
}

/// Every character, in the order the writer is told about them.
pub const CAMEOS: &[Cameo] = &[
    Cameo {
        slug: "noir",
        name: "Detective Dirk Danger",
        emoji: "🕵️",
        who: "a hard-boiled 1940s private eye who narrates everything as a case: the question is a dame \
with a secret, the wrong answers are suspects, and the rain never stops",
        fits: "history, crime, mysteries, true stories, whodunits",
        rate: 0.9,
        shift: true,
    },
    Cameo {
        slug: "nature",
        name: "Sir Bixborough",
        emoji: "🦉",
        who: "a hushed nature documentary narrator observing the players like rare wildlife in their \
natural habitat, the couch",
        fits: "animals, nature, science, geography, and watching players think",
        rate: 0.92,
        shift: false,
    },
    Cameo {
        slug: "trailer",
        name: "The Trailer Guy",
        emoji: "🎬",
        who: "the booming movie trailer narrator: \"In a world...\", dramatic pauses, and every question \
pitched as this summer's blockbuster",
        fits: "movies, TV, celebrities, big dramatic moments",
        rate: 0.85,
        shift: true,
    },
    Cameo {
        slug: "announcer",
        name: "Big Bix Barnes",
        emoji: "📣",
        who: "a play-by-play sports announcer calling the question like the final seconds of a \
championship, complete with replays and stats nobody asked for",
        fits: "sports, games, records, fast answers, comebacks",
        rate: 1.15,
        shift: false,
    },
    Cameo {
        slug: "auctioneer",
        name: "Colonel Bid McGavel",
        emoji: "🔨",
        who: "a fast-talking country auctioneer who rattles off numbers and treats every guess like a bid",
        fits: "numbers, dates, prices, measurements, the Closest Guess mini game",
        rate: 1.3,
        shift: false,
    },
    Cameo {
        slug: "coach",
        name: "Coach Krusher",
        emoji: "🏋️",
        who: "a screaming gym coach who thinks every challenge is boot camp and every player is a \
disappointing rookie with potential",
        fits: "sports, fitness, the human body, the Button Mash mini game",
        rate: 1.1,
        shift: false,
    },
    Cameo {
        slug: "critic",
        name: "Sir Reginald Brushwell",
        emoji: "🧐",
        who: "an insufferably snooty art critic who reviews questions, answers and players like \
gallery pieces, mostly unfavourably",
        fits: "art, literature, fashion, design, culture",
        rate: 0.95,
        shift: false,
    },
    Cameo {
        slug: "chef",
        name: "Chef Gustavo",
        emoji: "👨‍🍳",
        who: "a temperamental celebrity TV chef who judges answers like dishes and is deeply \
wounded by bad ones",
        fits: "food, drink, cooking, restaurants",
        rate: 1.05,
        shift: false,
    },
    Cameo {
        slug: "rocker",
        name: "Bix Vicious",
        emoji: "🎸",
        who: "a burned-out 1980s rock star who claims he was there for everything, toured with \
everyone, and can't hear very well anymore",
        fits: "music, the 70s, 80s and 90s, pop culture",
        rate: 0.95,
        shift: true,
    },
    Cameo {
        slug: "psychic",
        name: "Madame Bixtrova",
        emoji: "🔮",
        who: "a dramatic fortune teller who peers into the mists, foresees the players' doom, and \
is confidently wrong about the future",
        fits: "space, the future, superstitions, mythology, the Quick Draw mini game, the final round",
        rate: 0.9,
        shift: true,
    },
];

/// The character with this slug, if there is one.
pub fn cameo(slug: &str) -> Option<&'static Cameo> {
    CAMEOS.iter().find(|c| c.slug == slug)
}

/// Splits a line into its cameo (if it opens with a known `[as:slug]` tag) and the words to say.
/// An unknown tag is left in the text, so `validate` can report it.
pub fn split_cameo(text: &str) -> (Option<&'static Cameo>, &str) {
    let trimmed = text.trim_start();
    if let Some(rest) = trimmed.strip_prefix("[as:") {
        if let Some((slug, said)) = rest.split_once(']') {
            if let Some(c) = cameo(slug.trim()) {
                return (Some(c), said.trim_start());
            }
        }
    }
    (None, text)
}

/// The slug in a line's opening `[as:...]` tag, known or not.
pub fn cameo_tag(text: &str) -> Option<&str> {
    let rest = text.trim_start().strip_prefix("[as:")?;
    Some(rest.split_once(']')?.0.trim())
}

/// The roster, as the line writer sees it.
pub fn roster() -> String {
    CAMEOS
        .iter()
        .map(|c| format!("- [as:{}] {}, {}. Fits: {}.", c.slug, c.name, c.who, c.fits))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_split_off() {
        let (c, said) = split_cameo("[as:noir] The dame said Paris.");
        assert_eq!(c.map(|c| c.name), Some("Detective Dirk Danger"));
        assert_eq!(said, "The dame said Paris.");
        assert_eq!(split_cameo("Plain line.").0, None);
        // Unknown characters and sound tags are left alone.
        assert_eq!(split_cameo("[as:pirate] Arr.").1, "[as:pirate] Arr.");
        assert_eq!(cameo_tag("[as:pirate] Arr."), Some("pirate"));
        assert_eq!(split_cameo("[Sigh] Fine.").1, "[Sigh] Fine.");
    }

    #[test]
    fn slugs_are_unique() {
        for (i, c) in CAMEOS.iter().enumerate() {
            assert!(CAMEOS[..i].iter().all(|o| o.slug != c.slug), "{}", c.slug);
            assert!((0.5..=1.5).contains(&c.rate));
        }
        assert!(roster().contains("[as:auctioneer]"));
    }
}
