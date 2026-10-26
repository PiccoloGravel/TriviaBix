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
    }
}

impl Server {
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
