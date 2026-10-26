//! The admin panel's rendering jobs, with stand-ins for the tools (tests/fake-tools) that print
//! the same events as gpu/vast.py, art/render_on_vast.py and the line writer, but rent nothing.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};
use triviabix::{Config, render::RenderConfig, room::Timing};

const PASSWORD: &str = "pw";

struct Server {
    addr: String,
    cookie: String,
    tools: PathBuf,
    tmp: PathBuf,
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &to.join(entry.file_name()));
        } else {
            std::fs::copy(entry.path(), to.join(entry.file_name())).unwrap();
        }
    }
}

async fn start() -> Server {
    let tmp = std::env::temp_dir().join(format!("triviabix-render-{}", uuid::Uuid::new_v4()));
    let tools = tmp.join("tools");
    copy_dir(
        Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fake-tools")),
        &tools,
    );
    // A copy, so uploads and imports don't touch the repo's packs.
    let content = tmp.join("content");
    copy_dir(
        Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../content")),
        &content,
    );
    let config = Config {
        database_url: format!("sqlite://{}", tmp.join("test.db").display()),
        static_dir: concat!(env!("CARGO_MANIFEST_DIR"), "/../web").into(),
        public_url: None,
        max_players: 8,
        max_rooms: 10,
        itunes_url: "http://127.0.0.1:9".into(),
        music_country: "US".into(),
        music_warmup: false,
        library: Default::default(),
        timing: Timing {
            question: Duration::from_secs(5),
            song_question: Duration::from_secs(5),
            hangman: Duration::from_secs(5),
            reveal: Duration::from_millis(200),
            scoreboard: Duration::from_millis(200),
            round_intro: Duration::from_millis(200),
            recap: Duration::from_millis(200),
            intro: Duration::from_millis(200),
            talk_cps: 0.0,
            idle_timeout: Duration::from_secs(60),
        },
        admin_password: Some(PASSWORD.into()),
        host_script: None,
        content_dir: content,
        tts_url: None,
        voice_names: Vec::new(),
        tts_cache_dir: tmp.join("assets/voice"),
        render: RenderConfig {
            tools_dir: tools.clone(),
            python: "python3".into(),
            questions_bin: tools.join("triviabix-questions").display().to_string(),
            art_dir: tmp.join("art"),
            assets_dir: tmp.join("assets"),
            server_url: "http://127.0.0.1:1".into(),
        },
    };
    let app = triviabix::router(config).await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = format!("127.0.0.1:{}", listener.local_addr().unwrap().port());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let res = reqwest::Client::new()
        .post(format!("http://{addr}/api/admin/login"))
        .json(&json!({ "password": PASSWORD }))
        .send()
        .await
        .unwrap();
    let cookie = res.headers()["set-cookie"].to_str().unwrap();
    let cookie = cookie.split(';').next().unwrap().to_string();
    Server {
        addr,
        cookie,
        tools,
        tmp,
    }
}

impl Server {
    async fn put(&self, path: &str, body: Value) -> (u16, Value) {
        let res = reqwest::Client::new()
            .put(format!("http://{}/api/admin/{path}", self.addr))
            .header("cookie", &self.cookie)
            .json(&body)
            .send()
            .await
            .unwrap();
        (res.status().as_u16(), res.json().await.unwrap())
    }

    async fn call(&self, path: &str, body: Option<Value>) -> (u16, Value) {
        let http = reqwest::Client::new();
        let url = format!("http://{}/api/admin/{path}", self.addr);
        let req = match body {
            Some(b) => http.post(url).json(&b),
            None => http.get(url),
        };
        let res = req.header("cookie", &self.cookie).send().await.unwrap();
        (res.status().as_u16(), res.json().await.unwrap())
    }

    /// Polls the job until it isn't running.
    async fn finish(&self, id: i64) -> Value {
        for _ in 0..200 {
            let (_, job) = self.call(&format!("render/jobs/{id}"), None).await;
            if job["state"] != "running" {
                return job;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("job {id} never finished");
    }

    fn calls(&self) -> String {
        std::fs::read_to_string(self.tools.join("calls.log")).unwrap_or_default()
    }
}

#[tokio::test]
async fn voice_job_is_priced_confirmed_and_costed() {
    let s = start().await;

    // Locked without a login.
    let res = reqwest::get(format!("http://{}/api/admin/render", s.addr))
        .await
        .unwrap();
    assert_eq!(res.status(), 401);

    let (_, status) = s.call("render", None).await;
    assert_eq!(status["ready"]["gpu_tools"], true);
    assert_eq!(status["missing"]["art"], 3);
    assert_eq!(status["missing"]["lines"], 7);
    assert!(status["job"].is_null());
    // Keys are reported as set or not, never shown.
    assert!(status["ready"]["vast_key"].is_boolean());

    // Over the limit is refused before anything runs.
    let (code, res) = s
        .call(
            "render/preview",
            Some(json!({ "kind": "voice", "options": { "max_hours": 9 } })),
        )
        .await;
    assert_eq!(code, 409, "{res}");

    let (code, preview) = s
        .call(
            "render/preview",
            Some(json!({ "kind": "voice", "options": { "max_hours": 1 } })),
        )
        .await;
    assert_eq!(code, 200, "{preview}");
    assert_eq!(preview["ok"], true);
    assert_eq!(preview["gpu"], "RTX 4090 24GB");
    assert_eq!(preview["price_hr"], 0.3);
    assert_eq!(preview["max_cost"], 0.3);
    assert_eq!(preview["details"]["backlog"], 12);
    assert!(s.calls().contains("--dry-run"));
    assert!(!s.calls().contains("--yes"));
    let token = preview["token"].as_str().unwrap();

    // Not without saying yes.
    let (code, _) = s
        .call("render/start", Some(json!({ "token": token })))
        .await;
    assert_eq!(code, 400);
    let (code, job) = s
        .call(
            "render/start",
            Some(json!({ "token": token, "confirm": true })),
        )
        .await;
    assert_eq!(code, 200, "{job}");
    // A token works once.
    let (code, _) = s
        .call(
            "render/start",
            Some(json!({ "token": token, "confirm": true })),
        )
        .await;
    assert_eq!(code, 409);

    let job = s.finish(job["id"].as_i64().unwrap()).await;
    assert_eq!(job["state"], "done", "{job}");
    assert_eq!(job["cost"], 0.005);
    assert_eq!(job["max_cost"], 0.3);
    assert_eq!(job["released"], true);
    assert_eq!(job["details"]["backlog_start"], 12);
    assert_eq!(job["details"]["backlog"], 0);
    assert!(
        job["log"]
            .as_array()
            .unwrap()
            .iter()
            .any(|l| l[1] == "[vast] 0 lines to go")
    );
    // Rented at no more than a little over the quoted price.
    assert!(
        s.calls()
            .contains("voice --server http://127.0.0.1:1 --max-hours 1 --yes --max-price 0.36"),
        "{}",
        s.calls()
    );

    let (_, status) = s.call("render", None).await;
    assert_eq!(status["history"][0]["state"], "done");
    assert_eq!(status["spent"]["month"], 0.005);
}

#[tokio::test]
async fn cancelling_gives_the_gpu_back() {
    let s = start().await;
    let (_, preview) = s
        .call(
            "render/preview",
            Some(json!({ "kind": "voice", "options": { "max_hours": 0.25 } })),
        )
        .await;
    let (_, job) = s
        .call(
            "render/start",
            Some(json!({ "token": preview["token"], "confirm": true })),
        )
        .await;
    let id = job["id"].as_i64().unwrap();
    // Wait until it has rented.
    for _ in 0..100 {
        let (_, job) = s.call(&format!("render/jobs/{id}"), None).await;
        if job["instance"] == 42 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    // One job at a time.
    let (code, _) = s
        .call("render/preview", Some(json!({ "kind": "art" })))
        .await;
    assert_eq!(code, 409);

    let (code, res) = s
        .call(&format!("render/jobs/{id}/cancel"), Some(json!({})))
        .await;
    assert_eq!(code, 200, "{res}");
    let job = s.finish(id).await;
    assert_eq!(job["state"], "cancelled", "{job}");
    assert_eq!(job["released"], true);
    assert!(job["cost"].as_f64().unwrap() > 0.0);
    // It destroyed its own instance, so the backstop wasn't needed.
    assert!(!s.calls().contains("destroy 42"));
}

#[tokio::test]
async fn a_crashed_job_still_destroys_its_instance() {
    let s = start().await;
    let (_, preview) = s
        .call(
            "render/preview",
            Some(json!({ "kind": "voice", "options": { "max_hours": 0.5 } })),
        )
        .await;
    let (_, job) = s
        .call(
            "render/start",
            Some(json!({ "token": preview["token"], "confirm": true })),
        )
        .await;
    let job = s.finish(job["id"].as_i64().unwrap()).await;
    assert_eq!(job["state"], "failed");
    assert_eq!(job["released"], true);
    assert!(s.calls().contains("vast destroy 42"), "{}", s.calls());
}

#[tokio::test]
async fn art_and_lines_jobs() {
    let s = start().await;
    let (_, preview) = s
        .call(
            "render/preview",
            Some(json!({ "kind": "art", "options": { "kinds": "clip" } })),
        )
        .await;
    assert_eq!(preview["details"]["todo"]["count"], 3, "{preview}");
    assert_eq!(preview["max_cost"], 0.23);
    let (_, job) = s
        .call(
            "render/start",
            Some(json!({ "token": preview["token"], "confirm": true })),
        )
        .await;
    let job = s.finish(job["id"].as_i64().unwrap()).await;
    assert_eq!(job["state"], "done", "{job}");
    assert_eq!(job["cost"], 0.067);
    assert!(s.calls().contains("--kinds clip --yes --max-price 0.47"));
    // The TV gets the new art from appdata.
    let art = reqwest::get(format!(
        "http://{}/cutscenes/assets/gen/manifest.json",
        s.addr
    ))
    .await
    .unwrap();
    assert_eq!(art.status(), 200);
    assert!(art.text().await.unwrap().contains("bg/test.webp"));

    let (code, _) = s
        .call(
            "render/preview",
            Some(json!({ "kind": "art", "options": { "kinds": "nonsense" } })),
        )
        .await;
    assert_eq!(code, 409);

    let (_, preview) = s
        .call(
            "render/preview",
            Some(json!({ "kind": "lines", "options": { "limit": 20 } })),
        )
        .await;
    assert_eq!(preview["ok"], true, "{preview}");
    assert!(preview["max_cost"].is_null());
    let (_, job) = s
        .call(
            "render/start",
            Some(json!({ "token": preview["token"], "confirm": true })),
        )
        .await;
    let job = s.finish(job["id"].as_i64().unwrap()).await;
    assert_eq!(job["state"], "done", "{job}");
    assert_eq!(job["details"]["progress"]["output_tokens"], 120);
    // Saved where a container restart won't replace it.
    assert!(
        s.calls()
            .contains("scripts generate --into generated --limit 20 --missing either"),
        "{}",
        s.calls()
    );
}

#[tokio::test]
async fn importing_and_uploading_trivia() {
    let s = start().await;
    let (code, _) = s
        .call(
            "render/import",
            Some(json!({ "source": "opentdb", "amount": 500 })),
        )
        .await;
    assert_eq!(code, 409, "too many at once");
    let (code, _) = s
        .call(
            "render/import",
            Some(json!({ "source": "opentdb", "category": "nonsense" })),
        )
        .await;
    assert_eq!(code, 409);

    let (code, job) = s
        .call(
            "render/import",
            Some(
                json!({ "source": "opentdb", "amount": 10, "category": "9", "difficulty": "easy" }),
            ),
        )
        .await;
    assert_eq!(code, 200, "{job}");
    let job = s.finish(job["id"].as_i64().unwrap()).await;
    assert_eq!(job["state"], "done", "{job}");
    assert_eq!(job["details"]["imported"]["added"], 1);

    // Lines for just that import.
    let (_, preview) = s
        .call(
            "render/preview",
            Some(json!({ "kind": "lines", "options": { "batch": job["id"] } })),
        )
        .await;
    assert_eq!(preview["ok"], true, "{preview}");
    assert!(
        s.calls().contains("--missing either --ids opentdb-fake1"),
        "{}",
        s.calls()
    );
    let (code, _) = s
        .call(
            "render/preview",
            Some(json!({ "kind": "lines", "options": { "batch": 999 } })),
        )
        .await;
    assert_eq!(code, 409, "an import that doesn't exist");
    assert!(
        s.calls()
            .contains("import opentdb --amount 10 --category 9 --difficulty easy"),
        "{}",
        s.calls()
    );

    // A pack with a broken question is refused and not left behind.
    let (code, res) = s
        .call(
            "pipeline/pack",
            Some(json!({ "name": "Bad", "toml": "[pack]\nname = \"Bad\"\n\n[[question]]\ntype = \"multiple_choice\"\nprompt = \"Which?\"\nchoices = [\"A\", \"B\"]\nanswer = \"C\"\n" })),
        )
        .await;
    assert_eq!(code, 400, "{res}");
    assert!(
        res["error"]
            .as_str()
            .unwrap()
            .contains("not one of the choices")
    );

    let pack = "[pack]\nname = \"Office quiz\"\ncategory = \"general\"\n\n\
                [[question]]\ntype = \"true_false\"\nprompt = \"Our office has a ping-pong table.\"\nanswer = true\n\n\
                [[question]]\ntype = \"type_answer\"\nprompt = \"What floor is the kitchen on?\"\nanswers = [\"3\", \"third\"]\n";
    let (code, res) = s
        .call(
            "pipeline/pack",
            Some(json!({ "name": "Office Quiz.toml", "toml": pack })),
        )
        .await;
    assert_eq!(code, 200, "{res}");
    assert!(
        res["message"]
            .as_str()
            .unwrap()
            .contains("packs/mine/office-quiz.toml")
    );

    // Both batches are listed, newest first, with how far they've got.
    let mut batches = Value::Null;
    for _ in 0..100 {
        let (_, pipeline) = s.call("pipeline", None).await;
        batches = pipeline["batches"].clone();
        if batches[0]["in_game"] == 2 && batches[1]["in_game"] == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(batches.as_array().unwrap().len(), 2, "{batches}");
    assert_eq!(batches[0]["imported"]["added"], 2, "{batches}");
    assert_eq!(batches[0]["in_game"], 2, "{batches}");
    assert_eq!(batches[0]["with_lines"], 0);
    assert_eq!(batches[1]["in_game"], 1, "{batches}");
}

fn setting<'a>(view: &'a Value, key: &str) -> &'a Value {
    view["fields"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["key"] == key)
        .unwrap()
}

#[tokio::test]
async fn settings_reach_the_jobs_and_keys_stay_hidden() {
    let s = start().await;
    let (_, view) = s.call("settings", None).await;
    assert_eq!(setting(&view, "CLAUDE_MODEL")["default"], "claude-opus-5-5");
    assert_eq!(setting(&view, "VOICE_MAX_PRICE")["kind"]["max"], 2.0);

    let (code, view) = s
        .put(
            "settings",
            json!({ "key": "VAST_API_KEY", "value": " secret-key-123 " }),
        )
        .await;
    assert_eq!(code, 200);
    assert_eq!(setting(&view, "VAST_API_KEY")["source"], "saved");
    assert!(setting(&view, "VAST_API_KEY")["value"].is_null());
    assert!(!view.to_string().contains("secret-key-123"), "{view}");
    let (_, status) = s.call("render", None).await;
    assert_eq!(status["ready"]["vast_key"], true);
    assert!(!status.to_string().contains("secret-key-123"));

    // Bad values are refused.
    for (key, value) in [
        ("VOICE_MAX_PRICE", "5"),
        ("VOICE_MAX_PRICE", "cheap"),
        ("CLAUDE_MODEL", "gpt-4"),
        ("ART_IMAGE_MODEL", "not a model"),
        ("LIDARR_URL", "192.168.1.20:8686"),
        ("NOT_A_SETTING", "x"),
    ] {
        let (code, _) = s
            .put("settings", json!({ "key": key, "value": value }))
            .await;
        assert_eq!(code, 400, "{key} = {value}");
    }

    // The price limit, GPU and models reach the tools, with the saved key.
    for (key, value) in [
        ("VOICE_MAX_PRICE", "$1.2"),
        ("CLAUDE_MODEL", "claude-haiku-5-5"),
        ("CLAUDE_EFFORT", "low"),
        ("ART_VIDEO_MODEL", "Wan-AI/Wan2.2-T2V-A14B-Diffusers"),
    ] {
        let (code, view) = s
            .put("settings", json!({ "key": key, "value": value }))
            .await;
        assert_eq!(code, 200, "{key}: {view}");
    }
    let (_, view) = s.call("settings", None).await;
    assert_eq!(setting(&view, "VOICE_MAX_PRICE")["value"], "1.20");
    let (_, status) = s.call("render", None).await;
    assert_eq!(status["ready"]["voice_max_price"], 1.2);
    let (_, preview) = s
        .call("render/preview", Some(json!({ "kind": "voice" })))
        .await;
    assert_eq!(preview["ok"], true, "{preview}");
    assert!(
        s.calls()
            .contains("--dry-run --max-price 1.20 [key secret-key-123]"),
        "{}",
        s.calls()
    );
    let (_, preview) = s
        .call(
            "render/preview",
            Some(json!({ "kind": "lines", "options": { "limit": 5 } })),
        )
        .await;
    assert_eq!(preview["ok"], true, "{preview}");
    assert!(
        s.calls().contains("--model claude-haiku-5-5 --effort low"),
        "{}",
        s.calls()
    );

    // Clearing goes back to the container's value (none here).
    let (_, view) = s
        .put("settings", json!({ "key": "VAST_API_KEY", "value": null }))
        .await;
    assert_eq!(setting(&view, "VAST_API_KEY")["source"], "unset");
    let (_, status) = s.call("render", None).await;
    assert_eq!(status["ready"]["vast_key"], false);

    // Tests report what's wrong rather than failing the request.
    let (_, res) = s
        .call("settings/test", Some(json!({ "service": "lidarr" })))
        .await;
    assert_eq!(res["ok"], false);
    assert!(
        res["error"].as_str().unwrap().contains("Set Lidarr"),
        "{res}"
    );
    s.put(
        "settings",
        json!({ "key": "TTS_URL", "value": "http://127.0.0.1:1/" }),
    )
    .await;
    let (_, view) = s.call("settings", None).await;
    assert_eq!(setting(&view, "TTS_URL")["value"], "http://127.0.0.1:1");
    let (_, res) = s
        .call("settings/test", Some(json!({ "service": "voice" })))
        .await;
    assert_eq!(res["ok"], false);
    assert!(
        res["error"].as_str().unwrap().contains("Couldn't reach"),
        "{res}"
    );
}

#[tokio::test]
async fn designing_listening_to_and_picking_a_voice() {
    let s = start().await;
    let (_, status) = s.call("render", None).await;
    assert!(
        status["ready"]["voice_description"]
            .as_str()
            .unwrap()
            .contains("washed-up")
    );

    // Out-of-range options are refused before anything runs.
    let (code, _) = s
        .call(
            "render/preview",
            Some(json!({ "kind": "design", "options": { "count": 20 } })),
        )
        .await;
    assert_eq!(code, 409);
    let (code, res) = s
        .call(
            "render/preview",
            Some(json!({ "kind": "design", "options": { "text": "Too short." } })),
        )
        .await;
    assert_eq!(code, 409);
    assert!(
        res["error"].as_str().unwrap().contains("Too short."),
        "{res}"
    );

    let description = "A tired, gravelly game show host who has seen it all.";
    let options = json!({ "description": description, "count": 2 });
    let mine = "Hello again, everyone. Yes, it's still me. No, there's no refund.";
    let (_, preview) = s
        .call(
            "render/preview",
            Some(json!({ "kind": "design", "options": options })),
        )
        .await;
    assert_eq!(preview["ok"], true, "{preview}");
    assert_eq!(preview["details"]["todo"]["count"], 2);
    // Each candidate says a different line from the default list.
    let lines = preview["options"]["lines"].as_array().unwrap();
    assert_eq!(lines.len(), 2);
    assert_ne!(lines[0], lines[1]);
    assert_eq!(preview["max_cost"], 0.13);
    let calls = s.calls();
    assert!(calls.contains("vast design --library"), "{calls}");
    assert!(
        calls.contains("--count 2 --text ") && calls.contains(" --dry-run --max-price 0.50"),
        "{calls}"
    );
    let (code, job) = s
        .call(
            "render/start",
            Some(json!({ "token": preview["token"], "confirm": true })),
        )
        .await;
    assert_eq!(code, 200, "{job}");
    let job = s.finish(job["id"].as_i64().unwrap()).await;
    assert_eq!(job["state"], "done", "{job}");
    assert_eq!(job["details"]["made"].as_array().unwrap().len(), 2);
    assert_eq!(job["cost"], 0.033);

    // Candidates already made aren't paid for again.
    let options = json!({ "description": description, "count": 1, "text": mine });
    let (_, first) = s
        .call(
            "render/preview",
            Some(json!({ "kind": "design", "options": options })),
        )
        .await;
    let (_, job) = s
        .call(
            "render/start",
            Some(json!({ "token": first["token"], "confirm": true })),
        )
        .await;
    s.finish(job["id"].as_i64().unwrap()).await;
    let (_, again) = s
        .call(
            "render/preview",
            Some(json!({ "kind": "design", "options": options })),
        )
        .await;
    assert_eq!(again["nothing"], true, "{again}");
    assert!(again["token"].is_null());

    let (_, list) = s.call("voices", None).await;
    let voices = list["voices"].as_array().unwrap();
    assert_eq!(voices.len(), 3, "{list}");
    assert_eq!(voices[0]["description"], description);
    assert_eq!(voices[0]["current"], false);
    let key = voices[0]["key"].as_str().unwrap().to_string();

    let res = reqwest::Client::new()
        .get(format!("http://{}/api/admin/voices/{key}/clip", s.addr))
        .header("cookie", &s.cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    assert_eq!(res.headers()["content-type"], "audio/wav");
    assert!(res.bytes().await.unwrap().starts_with(b"RIFF"));
    let (code, _) = s
        .call(&format!("voices/{}/use", "f".repeat(32)), Some(json!({})))
        .await;
    assert_eq!(code, 404);

    let (code, list) = s.call(&format!("voices/{key}/use"), Some(json!({}))).await;
    assert_eq!(code, 200, "{list}");
    assert_eq!(list["bix_voice"], format!("clone:{key}"));
    assert!(
        list["voices"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["key"] == key.as_str() && v["current"] == true)
    );
    let (_, view) = s.call("settings", None).await;
    assert_eq!(setting(&view, "BIX_VOICE")["value"], format!("clone:{key}"));
    // The new voice got its own pack, in use, and the old one is kept to come back to.
    let (_, list) = s.call("packs", None).await;
    let packs = list["packs"].as_array().unwrap();
    assert_eq!(packs[0]["voice"], format!("clone:{key}"), "{list}");
    assert_eq!(packs[0]["active"], true);
    assert!(packs[0]["name"].as_str().unwrap().starts_with("Take "));
    assert!(
        packs
            .iter()
            .any(|p| p["voice"] == "ip:US_James" && p["active"] == false)
    );
}

#[tokio::test]
async fn voice_packs_switch_and_keep_their_lines() {
    let s = start().await;
    let (code, list) = s.call("packs", None).await;
    assert_eq!(code, 200, "{list}");
    let packs = list["packs"].as_array().unwrap();
    assert_eq!(packs.len(), 1, "{list}");
    let james = packs[0]["id"].as_i64().unwrap();
    assert_eq!(packs[0]["voice"], "ip:US_James");
    assert_eq!(packs[0]["active"], true);
    assert_eq!(packs[0]["made"], 0);
    assert!(packs[0]["missing"].as_u64().unwrap() > 100, "{list}");

    // A line James already has: kept in his folder of the asset store (slug of ip:US_James).
    let dir = s.tmp.join("assets/voice/8f7aee184357");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("style.txt"), "ip:US_James").unwrap();
    std::fs::write(
        dir.join("ef73ae6a8b47cd3601ab3fad1fb8099e.ogg"),
        b"OggS fake",
    )
    .unwrap();

    let (code, err) = s
        .call(
            "packs",
            Some(json!({ "name": "Nope", "voice": "ip:Nobody" })),
        )
        .await;
    assert_eq!(code, 400, "{err}");
    let (code, list) = s
        .call(
            "packs",
            Some(json!({ "name": "  Posh   Bix ", "voice": "ip:UK_George" })),
        )
        .await;
    assert_eq!(code, 200, "{list}");
    let george = list["packs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "Posh Bix")
        .unwrap()
        .clone();
    assert_eq!(george["active"], false);
    let george = george["id"].as_i64().unwrap();
    let (code, _) = s
        .call(
            "packs",
            Some(json!({ "name": "Again", "voice": "ip:UK_George" })),
        )
        .await;
    assert_eq!(code, 400);

    // James talks faster; switching away and back brings his speed back with him.
    let (code, list) = s
        .put(&format!("packs/{james}"), json!({ "speed": "1.2" }))
        .await;
    assert_eq!(code, 200, "{list}");
    let (_, view) = s.call("settings", None).await;
    assert_eq!(setting(&view, "BIX_SPEED")["value"], "1.2");

    let (code, list) = s
        .call(&format!("packs/{george}/use"), Some(json!({})))
        .await;
    assert_eq!(code, 200, "{list}");
    assert_eq!(list["packs"][0]["id"], george);
    assert_eq!(list["packs"][0]["active"], true);
    let (_, view) = s.call("settings", None).await;
    assert_eq!(setting(&view, "BIX_VOICE")["value"], "ip:UK_George");
    assert_eq!(setting(&view, "BIX_SPEED")["value"], "1.0");

    let (_, list) = s.call(&format!("packs/{james}/use"), Some(json!({}))).await;
    let back = &list["packs"][0];
    assert_eq!(back["id"], james);
    assert_eq!(back["made"], 1, "{list}");
    assert_eq!(back["speed"], 1.2);
    let (_, view) = s.call("settings", None).await;
    assert_eq!(setting(&view, "BIX_SPEED")["value"], "1.2");

    // His line is still there to hear; George has none yet.
    let res = reqwest::Client::new()
        .get(format!("http://{}/api/admin/packs/{james}/sample", s.addr))
        .header("cookie", &s.cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    assert_eq!(res.bytes().await.unwrap().as_ref(), b"OggS fake");
    let res = reqwest::Client::new()
        .get(format!("http://{}/api/admin/packs/{george}/sample", s.addr))
        .header("cookie", &s.cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 404);

    // Changing the voice on the Settings page switches packs too, making one if it's new.
    let (code, _) = s
        .put(
            "settings",
            json!({ "key": "BIX_VOICE", "value": "ip:US_Emma" }),
        )
        .await;
    assert_eq!(code, 200);
    let (_, list) = s.call("packs", None).await;
    assert_eq!(list["packs"][0]["voice"], "ip:US_Emma", "{list}");
    assert_eq!(list["packs"][0]["name"], "Emma (built-in)");
    assert_eq!(list["packs"].as_array().unwrap().len(), 3);
}

#[tokio::test]
async fn the_host_persona_reaches_the_line_writer() {
    let s = start().await;
    let (_, view) = s.call("settings", None).await;
    let persona = setting(&view, "HOST_PERSONA");
    assert_eq!(persona["kind"]["type"], "long_text");
    // The page's script, which draws long_text as a textarea, is never served stale.
    let res = reqwest::get(format!("http://{}/admin-settings.js", s.addr))
        .await
        .unwrap();
    assert_eq!(res.headers()["cache-control"], "no-cache");
    assert!(res.text().await.unwrap().contains("textarea"));
    assert!(
        persona["default"]
            .as_str()
            .unwrap()
            .contains("seen better days")
    );
    let (code, view) = s
        .put(
            "settings",
            json!({ "key": "HOST_PERSONA", "value": "Bix is a burnt-out host.\nHe sighs a lot." }),
        )
        .await;
    assert_eq!(code, 200, "{view}");
    assert_eq!(
        setting(&view, "HOST_PERSONA")["value"],
        "Bix is a burnt-out host.\nHe sighs a lot."
    );
    let (_, preview) = s
        .call(
            "render/preview",
            Some(json!({ "kind": "lines", "options": { "limit": 5 } })),
        )
        .await;
    assert_eq!(preview["ok"], true, "{preview}");
    assert!(
        s.calls()
            .contains("[persona Bix is a burnt-out host.|He sighs a lot.]"),
        "{}",
        s.calls()
    );
}

#[tokio::test]
async fn personas_keep_their_own_lines() {
    let s = start().await;
    let (code, list) = s.call("personas", None).await;
    assert_eq!(code, 200, "{list}");
    let original = &list["personas"][0];
    assert_eq!(original["name"], "Original Bix");
    assert_eq!(original["active"], true);
    assert_eq!(original["folder"], "generated");
    assert!(original["lines"].as_u64().unwrap() > 100, "{list}");
    let original = original["id"].as_i64().unwrap();

    let (code, err) = s
        .call(
            "personas",
            Some(json!({ "name": "Pirate Bix", "persona": "  " })),
        )
        .await;
    assert_eq!(code, 400, "{err}");
    let (code, list) = s
        .call(
            "personas",
            Some(json!({ "name": "Pirate Bix", "persona": "Bix is a pirate who lost his ship." })),
        )
        .await;
    assert_eq!(code, 200, "{list}");
    let pirate = list["personas"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "Pirate Bix")
        .unwrap()
        .clone();
    assert_eq!(pirate["folder"], "personas/pirate-bix");
    assert_eq!(pirate["lines"], 0);
    assert_eq!(pirate["active"], false);
    let pirate = pirate["id"].as_i64().unwrap();

    // Lines written for the pirate, as the line writer files them.
    let starter =
        std::fs::read_to_string(s.tmp.join("content/scripts/starter/science.toml")).unwrap();
    let question = starter
        .lines()
        .find_map(|l| l.strip_prefix("question = \""))
        .unwrap()
        .trim_end_matches('"')
        .to_string();
    let dir = s.tmp.join("content/scripts/personas/pirate-bix");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("science.toml"),
        format!("[[script]]\nquestion = \"{question}\"\nlead_in = \"Arr, a science one.\"\n"),
    )
    .unwrap();
    let pirate_heard = || async {
        let (_, found) = s.call("questions?q=Arr%2C%20a%20science", None).await;
        found.to_string().contains("Arr, a science one.")
    };
    assert!(!pirate_heard().await, "only the persona in use is heard");

    let (code, list) = s
        .call(&format!("personas/{pirate}/use"), Some(json!({})))
        .await;
    assert_eq!(code, 200, "{list}");
    assert_eq!(list["personas"][0]["id"], pirate);
    assert_eq!(list["personas"][0]["lines"], 1);
    assert!(pirate_heard().await);
    let (_, view) = s.call("settings", None).await;
    assert_eq!(
        setting(&view, "HOST_PERSONA")["value"],
        "Bix is a pirate who lost his ship."
    );
    // The line writer writes the pirate's missing lines into his own folder.
    let (_, preview) = s
        .call(
            "render/preview",
            Some(json!({ "kind": "lines", "options": { "limit": 5 } })),
        )
        .await;
    assert_eq!(preview["ok"], true, "{preview}");
    let calls = s.calls();
    let last = calls.lines().last().unwrap();
    assert!(last.contains("--into personas/pirate-bix"), "{calls}");
    assert!(last.contains("[scripts personas/pirate-bix]"), "{calls}");
    assert!(
        last.contains("[persona Bix is a pirate who lost his ship.]"),
        "{calls}"
    );

    // Editing who he is on the Settings page edits the persona in use.
    let (code, _) = s
        .put(
            "settings",
            json!({ "key": "HOST_PERSONA", "value": "Bix is a pirate with a parrot." }),
        )
        .await;
    assert_eq!(code, 200);
    let (_, list) = s.call("personas", None).await;
    assert_eq!(
        list["personas"][0]["persona"],
        "Bix is a pirate with a parrot."
    );

    // Back to the original: the shared lines play again, and the pirate's are kept.
    let (code, list) = s
        .call(&format!("personas/{original}/use"), Some(json!({})))
        .await;
    assert_eq!(code, 200, "{list}");
    assert!(!pirate_heard().await);
    assert!(dir.join("science.toml").is_file());
    let (_, view) = s.call("settings", None).await;
    assert_eq!(setting(&view, "HOST_PERSONA")["source"], "unset");
    let (_, preview) = s
        .call(
            "render/preview",
            Some(json!({ "kind": "lines", "options": { "limit": 5 } })),
        )
        .await;
    assert_eq!(preview["ok"], true, "{preview}");
    let calls = s.calls();
    let last = calls.lines().last().unwrap();
    assert!(last.contains("--into generated"), "{calls}");
    assert!(!last.contains("[scripts"), "{calls}");
}
