//! `triviabix-questions`: validate, inspect, export, and import trivia packs.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
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
            println!(
                "OK: {} questions in {} categories",
                bank.questions().len(),
                bank.categories().len()
            );
        }
        Command::Stats => {
            let bank = load(&cli.content)?;
            let s = bank.stats();
            println!("{} questions\n", s.total);
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
                if let Some(quip) = &q.host_quip {
                    println!("    host: {quip}");
                }
                if let Some(fact) = &q.fun_fact {
                    println!("    fun fact: {fact}");
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
