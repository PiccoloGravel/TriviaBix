"""Tests for the art pipeline, using the stub backend (no GPU): python3 -m unittest discover art"""

import json
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


if __name__ == "__main__":
    unittest.main()
