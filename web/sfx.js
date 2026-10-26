// Game-show sound for the TV: synthesized stings and a thinking bed (no audio files needed), and
// background music from the operator's music library between rounds. Turned on and off with the
// "Music" button or the B key; the host's voice and Name That Tune clips are separate.

const Sfx = (() => {
  let ctx = null;
  let master = null;
  const state = { on: TBX.load("tbx:music") !== false };

  function audio() {
    if (!ctx) {
      const AC = window.AudioContext || window.webkitAudioContext;
      if (!AC) return null;
      ctx = new AC();
      master = ctx.createGain();
      master.gain.value = 0.5;
      master.connect(ctx.destination);
    }
    return ctx;
  }

  /** One enveloped note. `at` is seconds from now. */
  function note(freq, at, dur, { type = "triangle", gain = 0.3, slide = 0 } = {}) {
    const c = audio();
    if (!c || !state.on) return;
    const t = c.currentTime + at;
    const osc = c.createOscillator();
    const env = c.createGain();
    osc.type = type;
    osc.frequency.setValueAtTime(freq, t);
    if (slide) osc.frequency.exponentialRampToValueAtTime(freq * slide, t + dur);
    env.gain.setValueAtTime(0.0001, t);
    env.gain.exponentialRampToValueAtTime(gain, t + 0.015);
    env.gain.exponentialRampToValueAtTime(0.0001, t + dur);
    osc.connect(env).connect(master);
    osc.start(t);
    osc.stop(t + dur + 0.05);
  }

  /** A short burst of filtered noise: hi-hats and ticks. */
  function noise(at, dur, { gain = 0.08, freq = 7000 } = {}) {
    const c = audio();
    if (!c || !state.on) return;
    const t = c.currentTime + at;
    const buf = c.createBuffer(1, Math.ceil(c.sampleRate * dur), c.sampleRate);
    const data = buf.getChannelData(0);
    for (let i = 0; i < data.length; i++) data[i] = Math.random() * 2 - 1;
    const src = c.createBufferSource();
    src.buffer = buf;
    const filter = c.createBiquadFilter();
    filter.type = "highpass";
    filter.frequency.value = freq;
    const env = c.createGain();
    env.gain.setValueAtTime(gain, t);
    env.gain.exponentialRampToValueAtTime(0.0001, t + dur);
    src.connect(filter).connect(env).connect(master);
    src.start(t);
  }

  const hz = (semitones) => 440 * Math.pow(2, semitones / 12); // 0 = A4

  const STINGS = {
    // Rising arpeggio for a new round or the start of the game.
    round() {
      [-9, -5, -2, 3].forEach((n, i) => note(hz(n), i * 0.09, 0.35, { type: "square", gain: 0.12 }));
      note(hz(3), 0.36, 0.7, { gain: 0.25 });
    },
    // Ding-ding: the answer is in.
    reveal() {
      note(hz(10), 0, 0.5, { type: "sine", gain: 0.35 });
      note(hz(15), 0.13, 0.9, { type: "sine", gain: 0.35 });
    },
    // Low buzzer when time runs out.
    timeup() {
      note(110, 0, 0.45, { type: "sawtooth", gain: 0.12 });
      note(116, 0, 0.45, { type: "sawtooth", gain: 0.1 });
    },
    // Little drum fill into the scoreboard.
    scores() {
      [0, 0.07, 0.14, 0.21].forEach((at) => noise(at, 0.06, { gain: 0.1, freq: 2500 }));
      note(hz(-21), 0.28, 0.3, { type: "sine", gain: 0.4, slide: 0.5 });
    },
    // Ta-da-da-daaa for the winner.
    fanfare() {
      [[-9, 0], [-9, 0.15], [-9, 0.3]].forEach(([n, at]) => note(hz(n), at, 0.14, { type: "square", gain: 0.12 }));
      note(hz(-5), 0.45, 0.25, { type: "square", gain: 0.12 });
      note(hz(-2), 0.7, 0.25, { type: "square", gain: 0.12 });
      [-9, -5, -2, 3].forEach((n) => note(hz(n), 0.95, 1.4, { gain: 0.14 }));
    },
    // A player joined the lobby.
    join() {
      note(hz(7), 0, 0.12, { type: "sine", gain: 0.2 });
      note(hz(14), 0.07, 0.18, { type: "sine", gain: 0.2 });
    },
    tick() { noise(0, 0.03, { gain: 0.12, freq: 4000 }); },
  };

  // The thinking bed: a walking bass, hats and a ticking clock that speeds up near the end.
  const bed = {
    timer: null,
    play(ms) {
      this.stop();
      const c = audio();
      if (!c || !state.on || !ms) return;
      const ends = performance.now() + ms;
      const bass = [-21, -21, -14, -16, -21, -21, -12, -14];
      let step = 0;
      let next = c.currentTime + 0.05;
      this.timer = setInterval(() => {
        const left = ends - performance.now();
        if (left <= 0) return this.stop();
        const bpm = left < 5000 ? 150 : 112;
        const beat = 60 / bpm / 2;
        while (next < c.currentTime + 0.12) {
          const at = next - c.currentTime;
          if (step % 2 === 0) note(hz(bass[(step / 2) % bass.length]), at, beat * 1.6, { type: "triangle", gain: 0.22 });
          noise(at, 0.04, { gain: step % 2 ? 0.05 : 0.025 });
          if (left < 5000 && step % 2 === 0) note(hz(15), at, 0.05, { type: "square", gain: 0.05 });
          next += beat;
          step++;
        }
      }, 25);
    },
    stop() { clearInterval(this.timer); this.timer = null; },
  };

  return {
    get on() { return state.on; },
    set on(v) {
      state.on = v;
      TBX.save("tbx:music", v);
      if (!v) bed.stop();
    },
    /** Browsers start audio suspended until someone presses something. */
    unlock() {
      const c = audio();
      if (c && c.state === "suspended") c.resume().catch(() => {});
    },
    play(name) { if (STINGS[name]) STINGS[name](); },
    bed,
  };
})();

// Background music from the library. Plays a stretch of each song, quietly, and dips under the
// host's voice.
const Bgm = {
  audio: new Audio(),
  list: [],
  index: 0,
  playing: false,
  blocked: false,
  timer: null,
  fade: null,
  level: 0.35,
  onTrack: null,
  /** Seconds of each song before moving on. */
  stretch: 75,

  setList(list) {
    const urls = (list || []).map((t) => t.url).join();
    if (urls === this.list.map((t) => t.url).join()) return;
    this.list = list || [];
    this.index = 0;
  },

  start() {
    if (this.playing || !this.list.length || !Sfx.on) return;
    this.playing = true;
    this.next();
  },

  next() {
    clearTimeout(this.timer);
    if (!this.playing || !this.list.length) return;
    const t = this.list[this.index++ % this.list.length];
    const a = this.audio;
    a.src = t.url;
    a.volume = 0;
    const go = () => {
      a.currentTime = (t.start_ms || 0) / 1000;
      a.play().then(() => {
        this.blocked = false;
        this.fadeTo(this.target());
        this.onTrack && this.onTrack(t);
      }).catch(() => { this.blocked = true; this.playing = false; });
    };
    a.addEventListener("loadedmetadata", go, { once: true });
    a.onended = () => this.next();
    a.onerror = () => { this.timer = setTimeout(() => this.next(), 500); };
    this.timer = setTimeout(() => this.fadeTo(0, () => this.next()), this.stretch * 1000);
  },

  stop() {
    if (!this.playing) return;
    this.playing = false;
    clearTimeout(this.timer);
    this.fadeTo(0, () => this.audio.pause());
    this.onTrack && this.onTrack(null);
  },

  /** Quieter while the host talks. */
  target(talking) { return talking ? this.level * 0.3 : this.level; },
  duck(talking) { if (this.playing && !this.fade) this.audio.volume = this.target(talking); },

  fadeTo(volume, done) {
    clearInterval(this.fade);
    const a = this.audio;
    const step = (volume - a.volume) / 20;
    let n = 0;
    this.fade = setInterval(() => {
      a.volume = Math.min(1, Math.max(0, a.volume + step));
      if (++n >= 20) {
        clearInterval(this.fade);
        this.fade = null;
        a.volume = volume;
        done && done();
      }
    }, 50);
  },

  /** A press after a blocked start gets the music going. */
  unlock() { if (this.blocked) { this.blocked = false; this.start(); } },
};
