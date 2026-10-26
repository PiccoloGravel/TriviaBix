# Cutscenes

Short full-screen bits the TV plays at big moments: the show opening, each question, a clean sweep,
a new leader, the champion. They are plain JSON, drawn with CSS and SVG, so there is no build step
and nothing to install. Open **`/cutscenes.html`** on a running server to see Bix's faces and play
every scene with made-up players.

## How it fits together

1. The server tags every host line with a **cue** (`state.host.cue`): what the moment is.
2. [`index.json`](index.json) maps each cue to Bix's face and, optionally, scenes to play.
3. Each scene is a file here, like [`champion.json`](champion.json): a backdrop, timed layers and sounds.

### Cues the server sends

| Cue | When |
| --- | --- |
| `intro` | The show opens |
| `round` / `final_round` | A question, puzzle or song starts (the last one gets `final_round`) |
| `streak`, `everyone`, `nobody`, `only_one`, `fastest`, `streak_broken`, `roast`, `asleep` | The answer is revealed; the cue is the biggest thing that happened |
| `new_leader`, `last_place`, `tie`, `leader` | The scoreboard |
| `winner` / `finale_tie` | The final scores |

To add a brand new moment, set the cue in `server/src/room.rs` (where the host speaks) and add it to
`index.json`.

## Add a cutscene

1. Copy a scene, say `on-fire.json`, to `my-scene.json` and change it.
2. In `index.json`, add `"my-scene"` to a cue's `scenes` list. With several, one is picked at random.
3. Reload `/cutscenes.html` and click the cue (or the scene, under "Every scene") to watch it. The
   page can switch the room rating and category, and shows all the generated art with its tags.

No rebuild is needed when the server runs from this folder; the Docker image copies `web/` in, so
rebuild the image to ship it.

A cue entry looks like this. `chance` (0 to 1, default 1) keeps frequent moments from getting old.

```json
"roast": { "mood": "smug", "scenes": ["roasted"], "chance": 0.35 }
```

## Scene format

```json
{
  "duration": 3000,
  "backdrop": { "kind": "sunburst", "colors": ["#ff3d00", "#ffb400"] },
  "layers": [
    { "type": "player", "who": "subject", "y": 50, "size": 30, "at": 200, "in": "zoom", "loop": "pulse" },
    { "type": "text", "text": "{label}", "y": 16, "size": 12, "at": 500, "in": "slam" },
    { "type": "bix", "mood": "excited", "x": 86, "y": 70, "size": 26, "at": 400, "in": "right" }
  ],
  "sounds": [{ "sound": "airhorn", "at": 500 }]
}
```

Times are milliseconds from the start. Positions `x` and `y` are percent of the screen (the layer's
centre), and sizes are percent of the screen height, so scenes look the same on any TV.

### Backdrop

`kind`: `split` (two-tone diagonal), `sunburst` (spinning rays), `stripes` (moving hazard stripes),
`spotlight` (dark stage with a roaming light), `curtains` (theatre curtains that open at `open` ms),
`dim` (darkens the game behind), `flash` (solid `colors[0]`), or `none`.
`colors`: one or two colours. `enter`: `wipe` (default), `fade`, `iris` or `none`.

`art` puts a generated picture or looping clip over the drawn background (see
[Generated art](#generated-art)); the drawn `kind` shows whenever there's no art that fits.
`shade` (0 to 1, default 0.25) darkens it so text stays readable, and `pan: true` drifts it slowly.

```json
"backdrop": { "kind": "sunburst", "colors": ["#ffb400", "#ff9f1c"], "art": { "kind": ["clip", "backdrop"], "tags": ["winner"] }, "shade": 0.2 }
```

### Layers

Every layer takes `x`, `y`, `size`, `at`, `in`, `loop`, and optionally `inMs` (entrance length,
default 600), `rotate` (degrees), `z` (stacking), and `out` + `leave` to exit early.

| `type` | Extra fields |
| --- | --- |
| `text` | `text`, `style` (`title`, `sub`, `stamp`, `bubble`), `color`, `bg`, `width` (vw). Long one-line text shrinks to fit. |
| `bix` | `mood`: `happy`, `excited`, `smug`, `shocked`, `sad`, `laugh`, `cool` |
| `player` | `who`: `subject` (the player the moment is about), `first`, `last` or a number; `name: false` hides the name; `hat` drops an emoji on their head at `hatAt` ms |
| `players` | Everyone in a row; `stagger` ms between each |
| `emoji` | `char` |
| `effect` | `kind`: `rain`, `rise` or `burst`; `chars`, `count`, `size`, `spread` (ms), `life` (ms) |
| `image` | `src`: a GIF, WebP, PNG, SVG, MP4 or WebM in `assets/`, or `art` (generated art, below) with a `fallback`; `frame: true` gives it a white card border |
| `lottie` | `src`: a Lottie `.json` in `assets/`; `loopAnim: true` loops it. The free lottie-web player loads from cdnjs the first time one plays. |

- **`in`** (entrance): `pop`, `slam`, `zoom`, `drop`, `rise`, `left`, `right`, `spin`, `stamp`, `flip`, `fade`, `cut`.
- **`loop`** (while on screen): `bob`, `wobble`, `pulse`, `spin`, `shake`, `float`, `swing`, `flicker`, `crawl`,
  `roll` (tumbles across the whole screen, left to right).
- **`leave`** (at `out`): `fade`, `zoom`, `drop`, `up`, `left`, `right`.

### Placeholders in text

`{name}`, `{label}` (the callout, like "4 IN A ROW"), `{score}`, `{round}`, `{total}`, `{kind}`
(Question, Puzzle or Song), `{category}`, `{host}`.

### Sounds

Built-in effects are synthesised in the browser, so there are no audio files to license:
`whoosh`, `boing`, `ding`, `pop`, `tada`, `drumroll`, `sad_trombone`, `airhorn`, `crickets`, `slam`.
Or play a file: `{ "src": "assets/honk.mp3", "at": 0, "volume": 0.8 }`. Muting the host voice (M)
mutes these too.

## Generated art

[`art/`](../../art/README.md) makes backdrops, stickers, looping clips and a title card per category
with open AI models, and ships them to `assets/gen/` with a `manifest.json`. Scenes don't name files;
they ask for a **kind** and **tags**, and get a random piece that fits (and is rated at or below the
room's rating). So new scenes can mix art that already exists, and new art shows up in old scenes.

```json
{ "type": "image", "art": { "kind": "sticker", "tags": ["winner"] }, "size": 30, "fallback": { "type": "emoji", "char": "🏆" } }
```

- `kind`: `backdrop`, `sticker`, `clip` or `category`, or a list in order of preference
  (`["clip", "backdrop"]` is a moving background if there is one, else a still).
- `tags`: all must match; add `"any": true` to match any of them. Tags are listed in `art/catalog.toml`
  and on `/cutscenes.html`. The cue names (`winner`, `nobody`, `roast`...) are tags too.
- `"art": "sticker:trophy"` asks for one exact piece.
- `"art": { "kind": "category" }` is the current category's title card.
- `fallback`: what the layer becomes when nothing fits, such as an emoji or a text layer. Without
  one, the layer is left out.

The TV fetches the art in the background when the page loads, so scenes don't wait on Wi-Fi. The
round title card also shows the category's card behind it when there is one.

## Graphics you can drop in

- [LottieFiles](https://lottiefiles.com/free-animations) has thousands of free animations (check each
  one's licence). Download the Lottie JSON into `assets/` and use a `lottie` layer.
- GIFs and stickers work as `image` layers. Keep them small; the TV loads them mid-game.
- Bix lives in [`../bix.js`](../bix.js) and [`../bix.css`](../bix.css). Each mood is a set of eyes,
  brows and mouth in one SVG; adding a mood means drawing its parts and adding a CSS rule.
