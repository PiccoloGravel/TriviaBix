"""Tests for the art pipeline, using the stub backend (no GPU): python3 -m unittest discover art"""

import json
import os
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import generate  # noqa: E402
from backends.models import GpuModels  # noqa: E402
from backends.stub import StubBackend  # noqa: E402
from library import Library, key_of  # noqa: E402

CUTSCENES = generate.ROOT / "web" / "cutscenes"
generate.SPEC["clip"]["frames"] = 9  # the stub draws every frame; a short clip keeps the tests quick
# A small catalog so a full run takes seconds.
CATALOG = """
[style]
version = "test-v1"
prompt = "house style"
sticker = "on white"
backdrop = "wide"
[category]
prompt = "{name}: {description}"
seed = 1
[[backdrop]]
id = "stage"
prompt = "a stage"
tags = ["stage", "intro"]
[[backdrop]]
id = "spicy"
prompt = "a spicy stage"
rating = "MA"
[[sticker]]
id = "trophy"
prompt = "a trophy"
tags = ["winner"]
[[clip]]
id = "stage"
from = "stage"
prompt = "lights pulse"
tags = ["intro"]
[[clip]]
id = "spicy"
from = "spicy"
prompt = "lights pulse"
"""
CATEGORIES = """
[[category]]
slug = "science"
name = "Science and Nature"
description = "Space."
[[category]]
slug = "after-dark"
name = "After Dark"
description = "Saucy."
rating = "PG-13"
"""


class Themes(unittest.TestCase):
    """Themes, category styles and the admin Art page's entries (art/studio.json in appdata)."""

    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp())
        (self.tmp / "catalog.toml").write_text(CATALOG)
        (self.tmp / "categories.toml").write_text(CATEGORIES)
        self.root, self.lib = self.tmp / "art", self.tmp / "assets"
        self.studio = {"themes": [{"id": "pixel", "name": "Pixel", "prompt": "pixel art", "palette": "pink and cyan"}],
                       "category_styles": {"science": "test tubes and stars"}}

    def jobs(self, studio=None, theme="bix"):
        return {j.key: j for j in generate.load_jobs(self.tmp / "catalog.toml", self.tmp / "categories.toml",
                                                     studio, theme)}

    def gen(self, theme, studio, *args):
        path = self.tmp / "studio.json"
        path.write_text(json.dumps(studio))
        return generate.main(["--out", str(generate.theme_out(self.root, theme)), "--library", str(self.lib),
                              "--catalog", str(self.tmp / "catalog.toml"), "--categories", str(self.tmp / "categories.toml"),
                              "--studio", str(path), "--theme", theme, *args])

    def test_an_empty_studio_changes_nothing(self):
        plain = self.jobs()
        self.assertEqual(plain, self.jobs({}))
        self.assertEqual({k: (j.prompt, j.seed, j.version) for k, j in plain.items()},
                         {k: (j.prompt, j.seed, j.version) for k, j in self.jobs({"themes": self.studio["themes"]}).items()},
                         "adding a theme leaves the default theme's art alone")

    def test_a_theme_swaps_the_look_and_keeps_the_rest(self):
        jobs = self.jobs(self.studio, "pixel")
        self.assertTrue(jobs["sticker:trophy"].prompt.startswith("pixel art, pink and cyan, a trophy, on white"))
        self.assertEqual(jobs["backdrop:stage"].version, "pixel-v1")
        self.assertEqual(jobs["clip:stage"].prompt, "lights pulse")
        self.assertEqual(generate.theme_out(self.root, "pixel"), self.root / "themes" / "pixel")
        self.assertEqual(generate.theme_out(self.root, "bix"), self.root)
        with self.assertRaises(SystemExit):
            self.jobs(self.studio, "nope")
        with self.assertRaises(SystemExit):
            self.jobs({"themes": [{"id": "bix", "prompt": "x"}]})

    def test_category_styles_go_on_the_card(self):
        jobs = self.jobs(self.studio)
        self.assertIn("Space., test tubes and stars, wide", jobs["category:science"].prompt)
        self.assertNotIn("test tubes", jobs["category:after-dark"].prompt)
        # A theme's own style for a category wins over the everywhere one.
        studio = {**self.studio, "themes": [{**self.studio["themes"][0], "category_styles": {"science": "pixel planets"}}]}
        self.assertIn("pixel planets", self.jobs(studio, "pixel")["category:science"].prompt)
        self.assertIn("test tubes", self.jobs(studio)["category:science"].prompt)

    def test_entries_add_change_and_hide_pieces(self):
        studio = {"entries": [
            {"kind": "sticker", "id": "duck", "prompt": "a rubber duck", "tags": ["funny"], "rating": "PG-13"},
            {"kind": "sticker", "id": "trophy", "tags": ["winner", "gold"]},
            {"kind": "backdrop", "id": "spicy", "hidden": True},
            {"kind": "clip", "id": "spicy", "hidden": True},
            {"kind": "backdrop", "id": "stage", "seed": 42, "theme": "pixel"},
            {"kind": "category", "id": "science", "seed": 9, "tags": ["nerdy"]},
        ], **{k: v for k, v in self.studio.items() if k == "themes"}}
        jobs = self.jobs(studio)
        self.assertTrue(jobs["sticker:duck"].custom)
        self.assertEqual(jobs["sticker:duck"].rating, "PG-13")
        self.assertEqual(jobs["sticker:trophy"].tags, ["winner", "gold"])
        self.assertNotIn("backdrop:spicy", jobs)
        self.assertEqual(jobs["category:science"].tags, ["category", "category:science", "nerdy"])
        self.assertEqual(jobs["category:science"].seed, 9)
        self.assertEqual(jobs["backdrop:stage"].seed, 0, "a new take in one theme leaves the others alone")
        self.assertEqual(self.jobs(studio, "pixel")["backdrop:stage"].seed, 42)
        with self.assertRaises(SystemExit):
            self.jobs({"entries": [{"kind": "sticker", "id": "nothing"}]})  # a new piece needs a prompt
        with self.assertRaises(SystemExit):
            self.jobs({"entries": [{"kind": "backdrop", "id": "spicy", "hidden": True}]})  # its clip has no backdrop

    def test_themes_are_kept_side_by_side_and_takes_come_back(self):
        self.assertEqual(self.gen("bix", self.studio, "--kinds", "backdrop,sticker"), 0)
        self.assertEqual(self.gen("pixel", self.studio, "--kinds", "backdrop,sticker"), 0)
        bix = json.loads((self.root / "manifest.json").read_text())["files"]
        pixel = json.loads((self.root / "themes" / "pixel" / "manifest.json").read_text())["files"]
        self.assertEqual(set(bix), set(pixel))
        self.assertNotEqual(bix["bg/stage.webp"]["original"], pixel["bg/stage.webp"]["original"])
        # A new take, then the old one back: the old one is free.
        studio = {**self.studio, "entries": [{"kind": "backdrop", "id": "stage", "seed": 5, "theme": "pixel"}]}
        self.assertEqual(self.gen("pixel", studio, "--kinds", "backdrop"), 0)
        made = len(list((self.lib / "image").rglob("*.png")))
        self.assertEqual(self.gen("pixel", self.studio, "--kinds", "backdrop"), 0,
                         "the first take is still in the asset store")
        self.assertEqual(len(list((self.lib / "image").rglob("*.png"))), made)
        listing = generate.listing(self.tmp / "catalog.toml", self.tmp / "categories.toml", studio, "pixel",
                                   self.root, Library(self.lib), "stub")
        stage = next(i for i in listing["items"] if i["key"] == "backdrop:stage")
        self.assertEqual(sorted(t["seed"] for t in stage["takes"]), [0, 5])
        self.assertEqual(stage["file"], "themes/pixel/bg/stage.webp")
        self.assertEqual(stage["status"], "stale", "the TV still has seed 0's file; seed 5 is asked for")
        made = {t["id"]: t["made"] for t in listing["themes"]}
        self.assertEqual(made["bix"], 3)
        self.assertEqual(next(c for c in listing["categories"] if c["slug"] == "science")["style"], "test tubes and stars")



class FakeGpu(StubBackend, GpuModels):
    """Draws like the stub but files art under the gpu backend's names, like a real GPU run would."""

    id = "gpu"
    image_steps = GpuModels.image_steps
    video_steps = GpuModels.video_steps
    models = GpuModels.models
    style_name = GpuModels.style_name


class Pipeline(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp())
        (self.tmp / "catalog.toml").write_text(CATALOG)
        (self.tmp / "categories.toml").write_text(CATEGORIES)
        self.out, self.lib = self.tmp / "gen", self.tmp / "assets"

    def gen(self, *args):
        return generate.main(["--out", str(self.out), "--library", str(self.lib), "--catalog", str(self.tmp / "catalog.toml"),
                              "--categories", str(self.tmp / "categories.toml"), *args])

    def manifest(self):
        return json.loads((self.out / "manifest.json").read_text())

    def test_jobs_come_from_catalog_and_categories(self):
        jobs = {j.key: j for j in generate.load_jobs(self.tmp / "catalog.toml", self.tmp / "categories.toml")}
        self.assertEqual(set(jobs), {"backdrop:stage", "backdrop:spicy", "sticker:trophy", "clip:stage", "clip:spicy",
                                     "category:science", "category:after-dark"})
        self.assertTrue(jobs["sticker:trophy"].prompt.startswith("house style, a trophy, on white"))
        self.assertEqual(jobs["category:science"].aliases, ["science-and-nature"])
        self.assertEqual(jobs["category:after-dark"].rating, "PG-13")
        self.assertEqual(jobs["clip:spicy"].rating, "MA", "a clip is rated like the backdrop it animates")
        self.assertEqual(jobs["clip:stage"].prompt, "lights pulse")

    def test_stub_run_makes_small_files_and_a_manifest(self):
        self.assertEqual(self.gen(), 0)
        m = self.manifest()
        self.assertEqual(len(m["files"]), 7)
        for path, f in m["files"].items():
            size = (self.out / path).stat().st_size
            self.assertEqual(size, f["bytes"])
            self.assertLessEqual(size, generate.SPEC[f["kind"]]["max_bytes"], path)
        self.assertEqual(m["categories"]["science-and-nature"], "category/science.webp")
        self.assertEqual(m["files"]["stickers/trophy.webp"]["tags"], ["winner"])
        self.assertEqual(m["files"]["bg/spicy.webp"]["rating"], "MA")
        self.assertTrue((self.out / "clips/stage.mp4").read_bytes()[4:8] == b"ftyp")

    def test_each_model_runs_once_in_batches(self):
        """Pictures are drawn in batches before any cut-out, and clips last, so a GPU loads each model once."""
        calls = []

        class Batching(FakeGpu):
            batch = 2

            def images(self, prompts, seeds, w, h):
                calls.append(("images", len(prompts)))
                return [self.image(p, s, w, h) for p, s in zip(prompts, seeds)]

            def cutout(self, img):
                calls.append(("cutout", 1))
                return super().cutout(img)

            def video(self, *a, **k):
                calls.append(("video", 1))
                return super().video(*a, **k)

        jobs = generate.load_jobs(self.tmp / "catalog.toml", self.tmp / "categories.toml")
        todo = generate.plan(jobs, self.out, "gpu")
        self.assertEqual(generate.run(jobs, todo, Batching(), self.out, Library(self.lib), "test-v1"), 0)
        kinds = [c[0] for c in calls]
        self.assertEqual(kinds, sorted(kinds, key=["images", "cutout", "video"].index), calls)
        # 5 pictures (2 backdrops, 2 cards, 1 sticker) in two sizes, 2 at a time.
        self.assertEqual(sum(n for k, n in calls if k == "images"), 5)
        self.assertLessEqual(max(n for k, n in calls if k == "images"), 2)
        self.assertEqual(kinds.count("cutout"), 1)
        self.assertEqual(kinds.count("video"), 2)

    def test_nothing_is_made_twice(self):
        self.gen()
        self.assertEqual(generate.plan(generate.load_jobs(self.tmp / "catalog.toml", self.tmp / "categories.toml"),
                                       self.out, "stub"), [])
        # Even with the TV's files gone, the originals come from the asset store, not the generator.
        stored = sorted(p.name for p in self.lib.rglob("*") if p.is_file())
        for f in self.out.rglob("*"):
            if f.is_file():
                f.unlink()
        lib = Library(self.lib)
        jobs = generate.load_jobs(self.tmp / "catalog.toml", self.tmp / "categories.toml")
        generate.run(jobs, jobs, StubBackend(), self.out, lib, "test-v1")
        self.assertEqual(lib.made, 0)
        self.assertGreater(lib.hits, 0)
        self.assertEqual(sorted(p.name for p in self.lib.rglob("*") if p.is_file()), stored)

    def test_store_layout_matches_docs_assets_md(self):
        self.gen("--only", "sticker:trophy")
        metas = [json.loads(p.read_text()) for p in self.lib.rglob("*.json")]
        cut = next(m for m in metas if m["inputs"].get("op") == "cutout")
        self.assertEqual(cut["key"], key_of(cut["inputs"]))
        folder = next(self.lib.rglob(cut["file"])).parent
        self.assertEqual((folder / "style.txt").read_text(), cut["style"])
        self.assertTrue(cut["style"].endswith(":test-v1"))
        self.assertIn("winner", cut["tags"])
        self.assertIsInstance(cut["created"], int)

    def test_changing_a_prompt_remakes_only_that(self):
        self.gen()
        (self.tmp / "catalog.toml").write_text(CATALOG.replace('prompt = "a trophy"', 'prompt = "a gold trophy"'))
        jobs = generate.load_jobs(self.tmp / "catalog.toml", self.tmp / "categories.toml")
        self.assertEqual([j.key for j in generate.plan(jobs, self.out, "stub")], ["sticker:trophy"])

    def test_a_pack_is_planned_and_listed_on_its_own(self):
        cat = CATALOG.replace('id = "trophy"', 'id = "trophy"\npack = "show"').replace(
            '[[clip]]\nid = "stage"', '[[clip]]\nid = "stage"\npack = "show"', 1)
        cat += '[[pack]]\nid = "show"\nname = "The show"\n'
        (self.tmp / "catalog.toml").write_text(cat)
        jobs = generate.load_jobs(self.tmp / "catalog.toml", self.tmp / "categories.toml")
        self.assertEqual({j.key for j in generate.plan(jobs, self.out, "stub", only={"pack:show"})},
                         {"sticker:trophy", "clip:stage"})
        self.assertEqual(self.gen("--only", "pack:show"), 0)
        listing = generate.listing(self.tmp / "catalog.toml", self.tmp / "categories.toml", {}, "bix",
                                   self.out, Library(self.lib), "stub")
        self.assertEqual([(p["id"], p["made"], p["total"]) for p in listing["packs"]], [("show", 2, 2)])
        self.assertEqual(next(i for i in listing["items"] if i["key"] == "sticker:trophy")["pack"], "show")
        (self.tmp / "catalog.toml").write_text(cat.replace('pack = "show"', 'pack = "nope"', 1))
        with self.assertRaises(SystemExit):
            generate.load_jobs(self.tmp / "catalog.toml", self.tmp / "categories.toml")

    def test_the_real_catalogs_packs_hold_pieces_in_every_theme(self):
        packs = {p["id"] for p in generate.load_packs()}
        self.assertTrue(packs)
        for theme in generate.load_themes():
            jobs = generate.load_jobs(theme=theme)
            self.assertEqual({j.pack for j in jobs if j.pack}, packs, theme)
            for j in jobs:
                if j.pack and j.kind != "clip":
                    self.assertIn(j.body, j.prompt, f"{theme} keeps what {j.key} shows")

    def test_retagging_never_rerenders(self):
        self.gen()
        (self.tmp / "catalog.toml").write_text(CATALOG.replace('tags = ["winner"]', 'tags = ["winner", "prize"]'))
        before = (self.out / "stickers/trophy.webp").stat().st_mtime_ns
        self.assertEqual(self.gen(), 0)
        self.assertEqual(self.manifest()["files"]["stickers/trophy.webp"]["tags"], ["winner", "prize"])
        self.assertEqual((self.out / "stickers/trophy.webp").stat().st_mtime_ns, before)

    def test_offline_lists_what_needs_a_gpu_then_uses_the_store(self):
        self.assertEqual(self.gen("--backend", "offline"), 3)
        self.assertEqual(len((self.out / ".needs-gpu").read_text().split()), 7)
        # A GPU run (here, a fake one on another machine) fills the store...
        remote = self.tmp / "remote"
        jobs = generate.load_jobs(self.tmp / "catalog.toml", self.tmp / "categories.toml")
        generate.run(jobs, jobs, FakeGpu(), remote / "gen", Library(remote / "assets"), "test-v1")
        # ...its results are brought home, and the offline pass finishes without a GPU.
        self.assertEqual(generate.merge(remote / "gen", self.out), 7)
        import shutil
        shutil.copytree(remote / "assets", self.lib, dirs_exist_ok=True)
        self.assertEqual(self.gen("--backend", "offline", "--force"), 0)
        self.assertFalse((self.out / ".needs-gpu").exists())

    def test_prune_drops_art_the_catalog_no_longer_lists(self):
        self.gen()
        (self.tmp / "catalog.toml").write_text(CATALOG.replace('[[sticker]]\nid = "trophy"\nprompt = "a trophy"\ntags = ["winner"]\n', ""))
        self.gen("--prune")
        self.assertNotIn("stickers/trophy.webp", self.manifest()["files"])
        self.assertFalse((self.out / "stickers/trophy.webp").exists())


class Scenes(unittest.TestCase):
    """The shipped scenes ask for art the catalog can make, and the shipped art is real."""

    def test_scene_art_requests_can_be_met(self):
        jobs = generate.load_jobs()
        index = json.loads((CUTSCENES / "index.json").read_text())
        names = {n for c in index["cues"].values() for n in c.get("scenes", [])}
        for name in names:
            scene = json.loads((CUTSCENES / f"{name}.json").read_text())
            wants = [scene.get("backdrop", {}).get("art")] + [layer.get("art") for layer in scene.get("layers", [])]
            for want in filter(None, wants):
                if isinstance(want, str):
                    kind, id = want.split(":")
                    self.assertIn(f"{kind}:{id}", {j.key for j in jobs}, f"{name} wants {want}")
                    continue
                kinds = [want["kind"]] if isinstance(want["kind"], str) else want["kind"]
                if kinds == ["category"]:
                    continue
                ok = any(j.kind in kinds and set(want.get("tags", [])) <= set(j.tags) for j in jobs)
                self.assertTrue(ok, f"{name}: nothing in art/catalog.toml is a {kinds} tagged {want.get('tags')}")

    def test_shipped_art_is_not_placeholder(self):
        manifest = generate.OUT / "manifest.json"
        if not manifest.exists():
            self.skipTest("no generated art shipped yet")
        files = json.loads(manifest.read_text())["files"]
        stubs = [p for p, f in files.items() if f.get("backend") == "stub"]
        self.assertEqual(stubs, [], "stub placeholders are in web/cutscenes/assets/gen; run with --backend gpu")



class ShipCodeTest(unittest.TestCase):
    def test_the_gpu_gets_this_servers_generator(self):
        import render_on_vast
        with tempfile.TemporaryDirectory() as d:
            render_on_vast.ship_code(Path(d))
            for rel in ("art/generate.py", "art/library.py", "art/catalog.toml", "art/backends/gpu.py",
                        "art/backends/models.py", "content/categories.toml"):
                self.assertTrue(Path(d, rel).is_file(), rel)
            self.assertFalse(Path(d, "art/test_generate.py").exists())


class SeveralThemesTest(unittest.TestCase):
    """Several themes' missing art goes to one rented GPU, each theme into its own folder."""

    def test_two_themes_share_one_gpu(self):
        import unittest.mock

        import render_on_vast
        with tempfile.TemporaryDirectory() as d:
            calls = Path(d, "calls.json")
            fake = Path(d, "vast.py")
            fake.write_text("import json, sys\n"
                            f"json.dump(sys.argv[1:], open({str(calls)!r}, 'w'))\n")
            root, lib = Path(d, "art"), Path(d, "lib")
            with unittest.mock.patch.object(render_on_vast, "VAST", fake), \
                    unittest.mock.patch.object(render_on_vast, "model_access", lambda *a: None):
                code = render_on_vast.main(["--art-root", str(root), "--library", str(lib), "--theme", "bix,comic-book",
                                            "--kinds", "backdrop", "--dry-run", "--yes"])
            self.assertEqual(code, 0)
            argv = json.loads(calls.read_text())
            self.assertEqual(argv.count("run"), 1, "one rental")
            cmd = argv[argv.index("--cmd") + 1]
            self.assertIn("--out /out/gen/bix ", cmd)
            self.assertIn("--out /out/gen/comic-book ", cmd)
            self.assertIn("--theme comic-book", cmd)
            self.assertIn("if [ $c -eq 75 ]; then exit 75; fi", cmd)
            self.assertTrue(cmd.endswith("exit $rc"))
            # Each theme's offline step ran in its own folder.
            self.assertTrue((root / ".needs-gpu").exists())
            self.assertTrue((root / "themes" / "comic-book" / ".needs-gpu").exists())

    def test_all_means_every_theme(self):
        import render_on_vast
        themes = render_on_vast.pick_themes("all", None)
        self.assertEqual(themes[0], "bix")
        self.assertGreater(len(themes), 1)
        with self.assertRaises(SystemExit):
            render_on_vast.pick_themes("bix,no-such-theme", None)


class ModelAccessTest(unittest.TestCase):
    """render_on_vast.model_access against a stand-in for the Hugging Face Hub."""

    def setUp(self):
        import threading
        from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

        class Hub(BaseHTTPRequestHandler):
            def log_message(self, *a):
                pass

            def send(self, code, body=b""):
                self.send_response(code)
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                if self.command != "HEAD":
                    self.wfile.write(body)

            def do_GET(self):
                repos = {"bfl/flux": {"gated": "manual", "siblings": [{"rfilename": "README.md"},
                                                                      {"rfilename": "model_index.json"}]},
                         "open/birefnet": {"gated": False}}
                repo = self.path.removeprefix("/api/models/")
                if repo not in repos:
                    return self.send(404)
                self.send(200, json.dumps(repos[repo]).encode())

            def do_HEAD(self):
                ok = self.path == "/bfl/flux/resolve/main/model_index.json" and \
                    self.headers.get("Authorization") == "Bearer good"
                self.send(200 if ok else 401)

        self.hub = ThreadingHTTPServer(("127.0.0.1", 0), Hub)
        threading.Thread(target=self.hub.serve_forever, daemon=True).start()
        self.old = os.environ.get("HF_ENDPOINT")
        os.environ["HF_ENDPOINT"] = f"http://127.0.0.1:{self.hub.server_address[1]}"

    def tearDown(self):
        self.hub.shutdown()
        if self.old is None:
            os.environ.pop("HF_ENDPOINT", None)
        else:
            os.environ["HF_ENDPOINT"] = self.old

    def test_a_gated_model_needs_a_token_that_can_read_it(self):
        import render_on_vast
        check = render_on_vast.model_access
        self.assertIsNone(check(["open/birefnet"], None))
        self.assertIn("accept its terms", check(["bfl/flux", "open/birefnet"], None))
        self.assertIn("can't download bfl/flux (401)", check(["bfl/flux"], "bad"))
        self.assertIsNone(check(["bfl/flux", "open/birefnet"], "good"))
        self.assertIn("doesn't exist", check(["nobody/nothing"], "good"))

if __name__ == "__main__":
    unittest.main()
