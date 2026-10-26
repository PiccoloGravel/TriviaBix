"""Tests the service around the model with the stub backend: python3 -m unittest tts/test_server.py"""

import json
import os
import threading
import time
import unittest
import urllib.error
import urllib.request
from http.server import ThreadingHTTPServer

os.environ["TTS_BACKEND"] = "stub"
import server  # noqa: E402


class ServiceTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.service = server.Service()
        cls.service.load()
        cls.httpd = ThreadingHTTPServer(("127.0.0.1", 0), server.make_handler(cls.service))
        cls.url = f"http://127.0.0.1:{cls.httpd.server_address[1]}"
        threading.Thread(target=cls.httpd.serve_forever, daemon=True).start()

    @classmethod
    def tearDownClass(cls):
        cls.httpd.shutdown()

    def post(self, body):
        req = urllib.request.Request(self.url + "/v1/speech", json.dumps(body).encode(),
                                     {"Content-Type": "application/json"})
        return urllib.request.urlopen(req)

    def test_health(self):
        h = json.load(urllib.request.urlopen(self.url + "/healthz"))
        self.assertEqual((h["ready"], h["backend"], h["voice"], h["format"]), (True, "stub", "ip:US_James", "wav"))

    def test_speech_is_a_wav(self):
        r = self.post({"text": "Hello, I'm Bix."})
        self.assertEqual(r.headers["Content-Type"], "audio/wav")
        self.assertEqual(r.headers["X-Voice"], "ip:US_James")
        self.assertEqual(r.read()[:4], b"RIFF")

    def test_bad_requests(self):
        for body in ({}, {"text": ""}, {"text": "x" * 2000}):
            with self.assertRaises(urllib.error.HTTPError) as e:
                self.post(body)
            self.assertEqual(e.exception.code, 400)

    def test_live_requests_jump_the_queue(self):
        gate = server.Gate()
        order = []

        def run(name, live, hold=0.0):
            with gate(live):
                order.append(name)
                time.sleep(hold)

        first = threading.Thread(target=run, args=("warm-1", False, 0.2))
        first.start()
        time.sleep(0.05)
        rest = [threading.Thread(target=run, args=("warm-2", False))]
        rest[0].start()
        time.sleep(0.05)
        rest.append(threading.Thread(target=run, args=("live", True)))
        rest[1].start()
        for t in [first, *rest]:
            t.join()
        self.assertEqual(order, ["warm-1", "live", "warm-2"])

    def test_token(self):
        self.service.token = "s3cret"
        try:
            with self.assertRaises(urllib.error.HTTPError) as e:
                self.post({"text": "Hi"})
            self.assertEqual(e.exception.code, 401)
            req = urllib.request.Request(self.url + "/v1/speech", json.dumps({"text": "Hi"}).encode(),
                                         {"Content-Type": "application/json", "Authorization": "Bearer s3cret"})
            self.assertEqual(urllib.request.urlopen(req).status, 200)
        finally:
            self.service.token = ""

    def test_voices(self):
        self.assertEqual(server.Voice("ip:UK_George").kwargs("Hi [Laughter] there")["text"], "Hi there")
        self.assertEqual(server.Voice("freeform:Be cheeky").kwargs("x")["task"], "FREEFORM")
        with self.assertRaises(SystemExit):
            server.Voice("clone:/nope.wav")
        with self.assertRaises(SystemExit):
            server.Voice("robot")


if __name__ == "__main__":
    unittest.main()
