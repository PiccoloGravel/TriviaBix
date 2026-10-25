// Bix, the host, drawn as an SVG microphone with a face. Every mood's eyes, brows and mouth are in
// the drawing at once; the `mood-*` class on the <svg> picks which ones show (see bix.css).
//
//   const face = Bix.create("smug");   // an <svg>, sized by its container
//   Bix.mood(face, "shocked");
//
// Add a mood: list it in MOODS, give the parts that should show a class with its name, and add
// `.bix.mood-<name> .<name>` to the visibility rule and a body animation in bix.css.

const Bix = {
  MOODS: ["happy", "excited", "smug", "shocked", "sad", "laugh", "cool"],
  count: 0,

  create(mood = "happy") {
    const id = `bix${++Bix.count}`;
    const wrap = document.createElement("div");
    wrap.innerHTML = `
<svg class="bix" viewBox="-20 -10 240 240" aria-label="Bix the host">
  <defs>
    <radialGradient id="${id}-head" cx="38%" cy="30%" r="75%">
      <stop offset="0" stop-color="#fff6c2"/><stop offset=".45" stop-color="#ffd23f"/><stop offset="1" stop-color="#e08a00"/>
    </radialGradient>
  </defs>
  <g class="bix-body">
    <g class="bix-arm left"><path d="M34 128 Q14 150 10 172" /><circle cx="10" cy="176" r="11"/></g>
    <g class="bix-arm right"><path d="M166 128 Q186 150 190 172" /><circle cx="190" cy="176" r="11"/></g>
    <path class="bix-handle" d="M76 160 H124 L114 222 Q100 228 86 222 Z"/>
    <rect class="bix-band" x="72" y="160" width="56" height="12" rx="6"/>
    <circle class="bix-headball" cx="100" cy="95" r="78" fill="url(#${id}-head)"/>
    <ellipse class="bix-shine" cx="62" cy="44" rx="18" ry="9" transform="rotate(-30 62 44)"/>
    <path class="bix-tie" d="M100 176 L76 164 L76 190 Z M100 176 L124 164 L124 190 Z"/>
    <circle class="bix-tie" cx="100" cy="177" r="7"/>
    <ellipse class="bix-cheek" cx="52" cy="122" rx="13" ry="8"/>
    <ellipse class="bix-cheek" cx="148" cy="122" rx="13" ry="8"/>

    <g class="v bix-eyes happy excited sad smug">
      <ellipse class="white" cx="72" cy="88" rx="17" ry="21"/><ellipse class="white" cx="128" cy="88" rx="17" ry="21"/>
      <circle class="pupil" cx="75" cy="92" r="9"/><circle class="pupil" cx="131" cy="92" r="9"/>
      <circle class="glint" cx="78" cy="88" r="3"/><circle class="glint" cx="134" cy="88" r="3"/>
    </g>
    <g class="v smug"><path class="lid" d="M53 88 A19 22 0 0 1 91 88 Z M109 88 A19 22 0 0 1 147 88 Z"/>
      <path class="line" d="M54 88 H90 M110 88 H146"/></g>
    <g class="v bix-eyes shocked">
      <ellipse class="white" cx="72" cy="86" rx="20" ry="25"/><ellipse class="white" cx="128" cy="86" rx="20" ry="25"/>
      <circle class="pupil" cx="72" cy="88" r="5"/><circle class="pupil" cx="128" cy="88" r="5"/>
    </g>
    <g class="v laugh"><path class="line" d="M56 94 Q72 74 88 94 M112 94 Q128 74 144 94"/></g>
    <g class="v cool">
      <path class="shades" d="M46 76 H94 Q96 106 72 106 Q48 106 46 76 Z M106 76 H154 Q152 106 128 106 Q104 106 106 76 Z"/>
      <path class="line" d="M94 80 Q100 74 106 80 M40 78 H46 M154 78 H160"/>
      <path class="shine-line" d="M56 84 L66 96 M116 84 L126 96"/>
    </g>

    <g class="v happy cool"><path class="brow" d="M56 58 Q72 50 88 58 M112 58 Q128 50 144 58"/></g>
    <g class="v excited laugh shocked"><path class="brow" d="M54 50 Q72 38 88 48 M112 48 Q128 38 146 50"/></g>
    <g class="v smug"><path class="brow" d="M56 66 Q72 62 88 66 M112 52 Q128 40 146 50"/></g>
    <g class="v sad"><path class="brow" d="M56 64 Q72 62 88 52 M112 52 Q128 62 144 64"/></g>

    <g class="mouth">
      <path class="v happy line" d="M72 128 Q100 156 128 128"/>
      <path class="v smug cool line" d="M78 138 Q106 148 128 126"/>
      <path class="v sad line" d="M76 148 Q100 128 124 148"/>
      <ellipse class="v shocked open" cx="100" cy="142" rx="13" ry="17"/>
      <g class="v excited laugh">
        <path class="open" d="M66 124 Q100 130 134 124 Q130 170 100 170 Q70 170 66 124 Z"/>
        <path class="teeth" d="M72 127 Q100 132 128 127 L127 136 Q100 140 73 136 Z"/>
        <ellipse class="tongue" cx="100" cy="160" rx="16" ry="8"/>
      </g>
    </g>
    <g class="mouth-talk">
      <ellipse class="open" cx="100" cy="140" rx="20" ry="15"/>
      <ellipse class="tongue" cx="100" cy="149" rx="11" ry="5"/>
    </g>

    <path class="v sad tear" d="M60 108 Q54 120 60 124 Q66 120 60 108 Z"/>
    <path class="v laugh tear" d="M48 96 Q42 108 48 112 Q54 108 48 96 Z M152 96 Q146 108 152 112 Q158 108 152 96 Z"/>
    <path class="v shocked sweat" d="M160 36 Q152 50 160 56 Q168 50 160 36 Z"/>
    <g class="v excited sparkles"><path d="M-4 30 l4 10 l10 4 l-10 4 l-4 10 l-4 -10 l-10 -4 l10 -4 Z M204 20 l3 8 l8 3 l-8 3 l-3 8 l-3 -8 l-8 -3 l8 -3 Z"/></g>
  </g>
</svg>`;
    const svg = wrap.firstElementChild;
    Bix.mood(svg, mood);
    return svg;
  },

  /** Switches the face. Unknown moods fall back to happy. */
  mood(svg, mood) {
    if (!Bix.MOODS.includes(mood)) mood = "happy";
    for (const m of Bix.MOODS) svg.classList.toggle(`mood-${m}`, m === mood);
    svg.dataset.mood = mood;
  },
};
