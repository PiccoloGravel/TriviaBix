-- How hard each question is. Trivia games run in rounds that get harder as they go, and the
-- room picks each round's questions by difficulty. Questions loaded from content packs carry the
-- pack's own `difficulty`; anything else starts out as medium.
ALTER TABLE questions ADD COLUMN difficulty TEXT NOT NULL DEFAULT 'medium'
    CHECK (difficulty IN ('easy', 'medium', 'hard'));

UPDATE questions SET difficulty = 'easy' WHERE source = 'seed' AND prompt IN (
    'What is the largest ocean on Earth?',
    'What is the capital of Canada?',
    'What is the tallest mountain above sea level?',
    'Which planet is known as the Red Planet?',
    'Who painted the Mona Lisa?',
    'Who was the first person to walk on the Moon?',
    'Which band released "Bohemian Rhapsody"?',
    'What is the wizarding school in Harry Potter?',
    'Which video game plumber wears a red cap?',
    'Which TV show is set at the Dunder Mifflin paper company?',
    'What is Batman''s real name?',
    'Pizza Margherita comes from which country?',
    'What is the main ingredient in guacamole?',
    'Sushi rolls are usually wrapped in what?',
    'What is the fastest land animal?',
    'What do giant pandas mostly eat?',
    'What is the largest mammal on Earth?',
    'How many players does a soccer team have on the field?',
    'How many rings are on the Olympic flag?',
    'Which company makes the iPhone?',
    '"Bonjour" means hello in which language?',
    'What color do you get by mixing blue and yellow?',
    'How many sides does a hexagon have?',
    'On Tinder, which way do you swipe to like someone?',
    'Which fruit emoji is the internet''s favorite stand-in for a butt?',
    'Which city''s motto is basically "what happens here, stays here"?',
    'What is the longest river in South America?',
    'What is the hardest natural substance?',
    'Which part of the cell is called the powerhouse?',
    'Which gas do plants absorb for photosynthesis?',
    'Which movie made "I''ll be back" famous?'
);

UPDATE questions SET difficulty = 'hard' WHERE source = 'seed' AND prompt IN (
    'Which river flows through Baghdad?',
    'Which country has the most lakes?',
    'Which planet has the most known moons?',
    'Which ancient wonder stood in Alexandria?',
    'How many hearts does an octopus have?',
    'Which country won the first FIFA World Cup in 1930?',
    'What is the name of the Rust programming language''s crab mascot?',
    'Casanova supposedly ate dozens of which food for breakfast?',
    'Which hormone is nicknamed the "cuddle hormone"?',
    'What is the smallest country in the world by area?'
);
