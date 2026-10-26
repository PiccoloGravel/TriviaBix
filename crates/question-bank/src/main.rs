//! `triviabix-questions`: validate, inspect, export, and import trivia packs.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use triviabix_questions::generate::{self, GenerateError, Generator, Usage, Wanted};
use triviabix_questions::import::{self, OpenTdbQuery, TriviaApiQuery};
use triviabix_questions::*;

#[derive(Parser)]
#[command(about = "Manage the TriviaBix question bank")]
struct Cli {
    /// The content folder holding categories.toml and packs/.
    #[arg(long, global = true, default_value = "content")]
    content: PathBuf,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Check every pack for mistakes. Exits non-zero if anything is wrong.
    Validate,
    /// Count questions by category, type, difficulty, and tag.
    Stats,
    /// Print a few random questions with answers, for playtesting.
    Sample {
        #[arg(long, default_value_t = 5)]
        count: usize,
        /// Category slugs or tags, such as 80s or music.
        #[arg(long = "topic")]
        topics: Vec<String>,
        /// Highest rating to include: PG, PG-13, MA, or NC-17.
        #[arg(long, default_value = "PG", value_parser = parse_rating)]
        rating: Rating,
    },
    /// Write the whole bank as one JSON array, for seeding a database.
    Export {
        #[arg(long, default_value = "questions.json")]
        out: PathBuf,
    },
    /// Pull questions from a trivia site into a new pack file.
    Import {
        #[command(subcommand)]
        source: ImportSource,
    },
    /// Write and load the host's per-question lines (lead-ins and reveal jokes).
    Scripts {
        #[command(subcommand)]
        action: ScriptsAction,
    },
}

#[derive(Subcommand)]
enum ScriptsAction {
    /// List questions that still need lines, as a JSON brief to fill in.
    Todo {
        /// Which lines count as missing: lead-in, reveal, or either.
        #[arg(long, default_value = "either", value_parser = ["lead-in", "reveal", "either"])]
        missing: String,
        /// Category slugs or tags.
        #[arg(long = "topic")]
        topics: Vec<String>,
        #[arg(long, default_value_t = 100)]
        limit: usize,
        /// Where to write the brief. Prints to the terminal without it.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Load a filled-in brief (JSON or TOML) into content/scripts, next to each question's pack.
    Add { file: PathBuf },
    /// Have Claude write the missing lines and save them to content/scripts. Uses the paid
    /// Anthropic API, so it needs ANTHROPIC_API_KEY.
    Generate {
        /// Which lines to write: lead-in, reveal, or either (both, where missing).
        #[arg(long, default_value = "either", value_parser = ["lead-in", "reveal", "either"])]
        missing: String,
        /// Category slugs or tags.
        #[arg(long = "topic")]
        topics: Vec<String>,
        /// The most questions to write lines for in this run.
        #[arg(long, default_value_t = 50)]
        limit: usize,
        /// Questions per API call.
        #[arg(long, default_value_t = 10)]
        batch: usize,
        #[arg(long, default_value = generate::DEFAULT_MODEL)]
        model: String,
        /// How hard Claude thinks: low, medium, high, xhigh, or max. Higher costs more.
        #[arg(long, default_value = "medium", value_parser = ["low", "medium", "high", "xhigh", "max"])]
        effort: String,
        /// Show what would be sent without calling the API.
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Subcommand)]
enum ImportSource {
    /// Open Trivia DB (opentdb.com), CC BY-SA 4.0.
    Opentdb {
        #[arg(long, default_value_t = 50)]
        amount: usize,
        /// Open Trivia DB category id; see --list-categories.
        #[arg(long)]
        category: Option<u32>,
        #[arg(long, value_parser = parse_difficulty)]
        difficulty: Option<Difficulty>,
        /// multiple or boolean.
        #[arg(long = "type")]
        kind: Option<String>,
        #[arg(long)]
        list_categories: bool,
        /// Defaults to content/packs/imported/opentdb-<category>.toml.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// The Trivia API (the-trivia-api.com), free for non-commercial use.
    TriviaApi {
        #[arg(long, default_value_t = 50)]
        amount: usize,
        /// Upstream categories, such as music, film_and_tv, food_and_drink.
        #[arg(long = "category")]
        categories: Vec<String>,
        /// Upstream tags, such as 1980's or pop_music.
        #[arg(long = "tag")]
        tags: Vec<String>,
        #[arg(long, value_parser = parse_difficulty)]
        difficulty: Option<Difficulty>,
        /// Defaults to content/packs/imported/trivia-api-<tags or categories>.toml.
        #[arg(long)]
        out: Option<PathBuf>,
    },
}

fn parse_difficulty(s: &str) -> Result<Difficulty, String> {
    match s {
        "easy" => Ok(Difficulty::Easy),
        "medium" => Ok(Difficulty::Medium),
        "hard" => Ok(Difficulty::Hard),
        _ => Err("use easy, medium, or hard".into()),
    }
}

fn parse_rating(s: &str) -> Result<Rating, String> {
    Rating::parse(s).ok_or_else(|| "use PG, PG-13, MA, or NC-17".into())
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(&cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: &Cli) -> Result<(), String> {
    match &cli.command {
        Command::Validate => {
            let bank = load(&cli.content)?;
            for w in bank.warnings() {
                println!("warning: {w}");
            }
            println!(
                "OK: {} questions in {} categories",
                bank.questions().len(),
                bank.categories().len()
            );
        }
        Command::Stats => {
            let bank = load(&cli.content)?;
            let s = bank.stats();
            println!("{} questions", s.total);
            println!(
                "{} with a host lead-in, {} with a reveal line\n",
                s.with_lead_in, s.with_reveal
            );
            println!("By category:");
            for c in bank.categories() {
                let n = s.by_category.get(&c.slug).copied().unwrap_or(0);
                println!("  {:<3} {:<28} {n:>4}", c.emoji, c.name);
            }
            println!("\nBy type:");
            for (k, v) in &s.by_type {
                println!("  {k:<32} {v:>4}");
            }
            println!("\nBy rating:");
            for r in Rating::ALL {
                println!(
                    "  {:<32} {:>4}",
                    r.as_str(),
                    s.by_rating.get(r.as_str()).copied().unwrap_or(0)
                );
            }
            println!("\nBy difficulty:");
            for (k, v) in &s.by_difficulty {
                println!("  {k:<32} {v:>4}");
            }
            let mut tags: Vec<_> = s.by_tag.iter().collect();
            tags.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
            println!("\nTop tags:");
            for (k, v) in tags.into_iter().take(20) {
                println!("  {k:<32} {v:>4}");
            }
        }
        Command::Sample {
            count,
            topics,
            rating,
        } => {
            let bank = load(&cli.content)?;
            let filter = Filter {
                topics: topics.clone(),
                max_rating: *rating,
                ..Default::default()
            };
            let mut rng = rand::thread_rng();
            for q in bank.pick(&filter, *count, &mut rng) {
                let view = q.public_view(&mut rng);
                println!(
                    "[{} / {} / {} / {}] {}",
                    q.category,
                    q.question_type().as_str(),
                    q.difficulty.as_str(),
                    q.rating.as_str(),
                    q.prompt
                );
                match &view.kind {
                    PublicKind::MultipleChoice { choices } => {
                        println!("    choices: {}", choices.join(" | "))
                    }
                    PublicKind::PutInOrder { items, .. } => {
                        println!("    items: {}", items.join(" | "))
                    }
                    _ => {}
                }
                println!("    answer: {}", q.answer_text());
                for line in q.host_lines() {
                    println!(
                        "    host {} [{}]: {}",
                        line.slot.as_str(),
                        line.rating.as_str(),
                        line.text
                    );
                }
                println!();
            }
        }
        Command::Export { out } => {
            let bank = load(&cli.content)?;
            let json = serde_json::to_string_pretty(bank.questions()).map_err(|e| e.to_string())?;
            std::fs::write(out, json).map_err(|e| e.to_string())?;
            println!(
                "Wrote {} questions to {}",
                bank.questions().len(),
                out.display()
            );
        }
        Command::Import { source } => import_cmd(&cli.content, source)?,
        Command::Scripts { action } => scripts_cmd(&cli.content, action)?,
    }
    Ok(())
}

fn load(dir: &Path) -> Result<QuestionBank, String> {
    QuestionBank::load_dir(dir).map_err(|e| e.to_string())
}

fn import_cmd(content: &Path, source: &ImportSource) -> Result<(), String> {
    let (questions, pack_name, default_out) = match source {
        ImportSource::Opentdb {
            list_categories: true,
            ..
        } => {
            for (id, name) in import::OPENTDB_CATEGORIES {
                println!("{id:>3}  {name}");
            }
            return Ok(());
        }
        ImportSource::Opentdb {
            amount,
            category,
            difficulty,
            kind,
            out,
            ..
        } => {
            let q = OpenTdbQuery {
                amount: *amount,
                category: *category,
                difficulty: *difficulty,
                kind: kind.clone(),
            };
            eprintln!("Fetching {amount} questions from Open Trivia DB...");
            let qs = import::fetch_opentdb(&q).map_err(|e| e.to_string())?;
            let label = category
                .map(|c| c.to_string())
                .unwrap_or_else(|| "mixed".into());
            (
                qs,
                format!("Open Trivia DB ({label})"),
                out.clone().unwrap_or_else(|| {
                    content
                        .join("packs/imported")
                        .join(format!("opentdb-{label}.toml"))
                }),
            )
        }
        ImportSource::TriviaApi {
            amount,
            categories,
            tags,
            difficulty,
            out,
        } => {
            let q = TriviaApiQuery {
                amount: *amount,
                categories: categories.clone(),
                tags: tags.clone(),
                difficulty: *difficulty,
            };
            eprintln!("Fetching {amount} questions from The Trivia API...");
            let qs = import::fetch_trivia_api(&q).map_err(|e| e.to_string())?;
            let mut label: Vec<String> =
                tags.iter().chain(categories).map(|s| slugify(s)).collect();
            if label.is_empty() {
                label.push("mixed".into());
            }
            let label = label.join("-");
            (
                qs,
                format!("The Trivia API ({label})"),
                out.clone().unwrap_or_else(|| {
                    content
                        .join("packs/imported")
                        .join(format!("trivia-api-{label}.toml"))
                }),
            )
        }
    };

    // Skip anything already in the bank or in the file being added to.
    let mut existing: Vec<Question> = QuestionBank::load_dir(content)
        .map(|b| b.questions().to_vec())
        .unwrap_or_default();
    let mut pack = if default_out.exists() {
        let text = std::fs::read_to_string(&default_out).map_err(|e| e.to_string())?;
        PackFile::from_toml(&text)?
    } else {
        PackFile {
            pack: PackMeta {
                name: pack_name,
                ..Default::default()
            },
            questions: vec![],
        }
    };
    existing.extend(pack.resolve("existing"));
    let mut seen: HashSet<String> = existing.iter().map(|q| normalize(&q.prompt)).collect();
    let fetched = questions.len();
    let fresh = import::dedupe(questions, &mut seen);
    let fresh: Vec<Question> = fresh
        .into_iter()
        .filter(|q| validate_question(q).is_empty())
        .collect();
    let added = fresh.len();
    pack.questions.extend(fresh.iter().map(PackQuestion::from));

    if let Some(dir) = default_out.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(&default_out, pack.to_toml()?).map_err(|e| e.to_string())?;
    println!(
        "Fetched {fetched}, added {added} new questions to {} ({} skipped as duplicates or invalid)",
        default_out.display(),
        fetched - added
    );
    println!("Review them, then run `triviabix-questions validate`.");
    Ok(())
}

/// One question in a `scripts todo` brief. Everything but `question`, `lead_in`, and `reveal`
/// is context for the writer; `scripts add` ignores it.
#[derive(serde::Serialize)]
struct Brief<'a> {
    question: &'a str,
    prompt: &'a str,
    category: &'a str,
    #[serde(rename = "type")]
    kind: &'static str,
    answer: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    choices: Option<&'a [String]>,
    rating: &'static str,
    difficulty: &'static str,
    /// Lines the question already has, so new ones don't repeat them.
    existing: Vec<String>,
    lead_in: Option<()>,
    reveal: Option<()>,
}

fn scripts_cmd(content: &Path, action: &ScriptsAction) -> Result<(), String> {
    let bank = load(content)?;
    match action {
        ScriptsAction::Todo {
            missing,
            topics,
            limit,
            out,
        } => {
            let todo: Vec<&Question> = needing_lines(&bank, wanted(missing), topics)
                .into_iter()
                .map(|(q, _)| q)
                .collect();
            let briefs: Vec<Brief> = todo
                .iter()
                .take(*limit)
                .map(|q| Brief {
                    question: &q.id,
                    prompt: &q.prompt,
                    category: &q.category,
                    kind: q.question_type().as_str(),
                    answer: q.answer_text(),
                    choices: match &q.kind {
                        QuestionKind::MultipleChoice { choices, .. } => Some(choices),
                        _ => None,
                    },
                    rating: q.rating.as_str(),
                    difficulty: q.difficulty.as_str(),
                    existing: q.host_lines().into_iter().map(|l| l.text).collect(),
                    lead_in: None,
                    reveal: None,
                })
                .collect();
            let json = serde_json::to_string_pretty(&briefs).map_err(|e| e.to_string())?;
            match out {
                Some(path) => {
                    std::fs::write(path, json).map_err(|e| e.to_string())?;
                    eprintln!(
                        "Wrote {} of {} questions needing lines to {}",
                        briefs.len(),
                        todo.len(),
                        path.display()
                    );
                }
                None => println!("{json}"),
            }
        }
        ScriptsAction::Add { file } => {
            let incoming =
                ScriptFile::read(file).map_err(|e| format!("{}: {e}", file.display()))?;
            let (added, updated) = add_scripts(content, &bank, incoming.scripts)?;
            println!("Added scripts for {added} questions and replaced {updated}.");
            println!("Check them with `triviabix-questions sample`, then restart the server to load them.");
        }
        ScriptsAction::Generate {
            missing,
            topics,
            limit,
            batch,
            model,
            effort,
            dry_run,
        } => {
            let todo = needing_lines(&bank, wanted(missing), topics);
            let total = todo.len();
            let todo: Vec<_> = todo.into_iter().take(*limit).collect();
            if todo.is_empty() {
                println!("Every matching question already has lines.");
                return Ok(());
            }
            let batches: Vec<_> = todo.chunks((*batch).max(1)).collect();
            if *dry_run {
                let gen = Generator::new("", model, effort);
                let json = serde_json::to_string_pretty(&gen.request(batches[0]))
                    .map_err(|e| e.to_string())?;
                println!("{json}");
                eprintln!(
                    "Would write lines for {} of {total} questions with {model}, {} per request. Above is the first request.",
                    todo.len(),
                    batch.max(&1)
                );
                return Ok(());
            }
            let key = std::env::var("ANTHROPIC_API_KEY")
                .ok()
                .filter(|k| !k.trim().is_empty())
                .ok_or("set ANTHROPIC_API_KEY to an Anthropic API key (console.anthropic.com) first. Calls are billed to that account.")?;
            let gen = Generator::new(key.trim(), model, effort);
            eprintln!(
                "Writing lines for {} of {total} questions with {model}, {} per request...",
                todo.len(),
                batch.max(&1)
            );
            let (mut written, mut skipped) = (0, 0);
            let mut usage = Usage::default();
            for (i, chunk) in batches.iter().enumerate() {
                match gen.write(chunk) {
                    Ok((entries, used)) => {
                        usage.input_tokens += used.input_tokens;
                        usage.output_tokens += used.output_tokens;
                        skipped += chunk.len() - entries.len();
                        // Save as we go, so a failure later doesn't waste what's paid for.
                        let (added, updated) = add_scripts(content, &bank, entries)?;
                        written += added + updated;
                        eprintln!(
                            "  batch {}/{}: {} questions",
                            i + 1,
                            batches.len(),
                            added + updated
                        );
                    }
                    Err(e @ (GenerateError::Api { .. } | GenerateError::Http(_))) => {
                        return Err(format!(
                            "{e}. Saved lines for {written} questions before this."
                        ));
                    }
                    Err(e) => {
                        skipped += chunk.len();
                        eprintln!("  batch {}/{}: skipped, {e}", i + 1, batches.len());
                    }
                }
            }
            println!(
                "Wrote lines for {written} questions ({skipped} skipped). Used {} input and {} output tokens.",
                usage.input_tokens, usage.output_tokens
            );
            println!("Skim them in content/scripts or with `triviabix-questions sample`, then hit Reload packs in the admin panel.");
        }
    }
    Ok(())
}

fn wanted(missing: &str) -> Wanted {
    match missing {
        "lead-in" => Wanted::LeadIn,
        "reveal" => Wanted::Reveal,
        _ => Wanted::Either,
    }
}

/// Questions matching `topics` that still need lines, with the slots each one needs.
fn needing_lines<'a>(
    bank: &'a QuestionBank,
    wanted: Wanted,
    topics: &[String],
) -> Vec<(&'a Question, Vec<ScriptSlot>)> {
    let filter = Filter {
        topics: topics.to_vec(),
        max_rating: Rating::Nc17,
        ..Default::default()
    };
    bank.questions()
        .iter()
        .filter(|q| filter.matches(q))
        .map(|q| (q, wanted.slots_for(q)))
        .filter(|(_, slots)| !slots.is_empty())
        .collect()
}

/// Files script entries under `content/scripts`, in a file named after each question's pack.
/// Returns how many entries were new and how many replaced an older script.
fn add_scripts(
    content: &Path,
    bank: &QuestionBank,
    entries: Vec<ScriptEntry>,
) -> Result<(usize, usize), String> {
    let mut by_file: std::collections::BTreeMap<PathBuf, Vec<ScriptEntry>> = Default::default();
    let mut problems = Vec::new();
    for mut e in entries {
        if e.lead_in.is_none() && e.reveal.is_none() {
            continue;
        }
        let Some(q) = bank.get(&e.question) else {
            problems.push(format!("{}: no question has this id", e.question));
            continue;
        };
        for line in e.lines(q.rating) {
            problems.extend(
                validate_line(&line)
                    .into_iter()
                    .map(|p| format!("{}: {p}", e.question)),
            );
        }
        e.prompt = Some(q.prompt.clone());
        let pack = bank
            .pack_of(&e.question)
            .map(|p| p.with_extension("toml"))
            .unwrap_or_else(|| PathBuf::from("misc.toml"));
        by_file
            .entry(content.join("scripts").join(pack))
            .or_default()
            .push(e);
    }
    if !problems.is_empty() {
        return Err(format!("nothing was saved:\n  {}", problems.join("\n  ")));
    }
    let (mut added, mut total) = (0, 0);
    for (path, entries) in by_file {
        let mut file = if path.exists() {
            ScriptFile::read(&path).map_err(|e| format!("{}: {e}", path.display()))?
        } else {
            ScriptFile::default()
        };
        total += entries.len();
        added += file.merge(entries);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        std::fs::write(&path, file.to_toml()?).map_err(|e| e.to_string())?;
    }
    Ok((added, total - added))
}
