//! Optional AI stages over a decompiled tree.  OFF by default: nothing here runs
//! unless one of the explicit subcommands is used, and every subcommand warns
//! before code leaves the machine.
//!
//! * `ai-rename`   — meaningful names for obfuscated classes, fields and methods,
//!   validated against the class hierarchy and applied to the BYTECODE (ASM), so
//!   every reference changes consistently and the jar can be decompiled again;
//! * `ai-annotate` — short Javadoc explanations inserted into a copy of the source,
//!   without touching a single line of code.
//!
//! [`provider`] holds the transports (Anthropic Messages API, OpenAI-compatible
//! local servers), retries and the response cache, implementing the shared
//! [`crate::ai_model::ChatModel`] contract also used by `ai-fix`.

pub mod annotate;
pub mod apply;
pub mod classfile;
pub mod external;
pub mod names;
pub mod provider;
pub mod suggest;

use crate::ai_model::ChatModel;
use crate::util;
use clap::Args;
use names::{NameMap, World, MAP_VERSION};
use provider::{CachedModel, LiveModel, ProviderConfig, ProviderOverrides};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Model selection flags shared by the AI subcommands.
#[derive(Args, Debug, Clone, Default)]
pub struct ModelArgs {
    /// anthropic (default) | openai (any OpenAI-compatible endpoint, e.g. Ollama) |
    /// claude-code (an installed, logged-in Claude Code CLI)
    #[arg(long = "provider")]
    pub provider: Option<String>,
    /// model id (default: claude-sonnet-5-5 for anthropic)
    #[arg(long = "model")]
    pub model: Option<String>,
    /// endpoint base URL (default: https://api.anthropic.com, or
    /// http://localhost:11434/v1 for openai)
    #[arg(long = "base-url")]
    pub base_url: Option<String>,
    /// name of the environment variable that holds the API key
    #[arg(long = "api-key-env")]
    pub api_key_env: Option<String>,
    /// TOML file with an `[ai]` table (provider, model, base_url, effort, ...)
    #[arg(long = "ai-config")]
    pub ai_config: Option<String>,
    /// Anthropic `output_config.effort` (low | medium | high | xhigh | max)
    #[arg(long = "effort")]
    pub effort: Option<String>,
    /// approximate input tokens per request (classes are batched under it)
    #[arg(long = "batch-tokens", default_value_t = 24_000)]
    pub batch_tokens: usize,
    /// response cache directory (default: ~/.arlecchino/ai-cache)
    #[arg(long = "cache-dir")]
    pub cache_dir: Option<String>,
    /// send nothing: print and save the prompts; answers come from the cache only
    #[arg(long = "dry-run")]
    pub dry_run: bool,
    /// do not ask before sending code to a remote endpoint
    #[arg(long = "yes", short = 'y')]
    pub yes: bool,
}

impl ModelArgs {
    fn resolve(&self) -> Result<ProviderConfig, String> {
        ProviderConfig::resolve(&ProviderOverrides {
            provider: self.provider.clone(),
            model: self.model.clone(),
            base_url: self.base_url.clone(),
            api_key_env: self.api_key_env.clone(),
            max_tokens: None,
            effort: self.effort.clone(),
            config: self.ai_config.as_ref().map(PathBuf::from),
        })
    }
    fn cache_dir(&self) -> PathBuf {
        self.cache_dir
            .as_ref()
            .map(|p| util::absolutize(Path::new(p)))
            .unwrap_or_else(|| crate::midp_stub::arlecchino_home().join("ai-cache"))
    }
}

fn confirm_remote(base_url: &str) -> bool {
    use std::io::{BufRead, IsTerminal, Write};
    if !std::io::stdin().is_terminal() {
        return false;
    }
    print!("Send decompiled code to {base_url}? [y/N] ");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    let _ = std::io::stdin().lock().read_line(&mut line);
    matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

/// Print the warning and get consent.  `None` = proceed, `Some(code)` = stop.
fn gate(stage: &str, what: &str, cfg: &ProviderConfig, m: &ModelArgs) -> Option<i32> {
    println!("   model: {} @ {}", cfg.id(), cfg.endpoint());
    if m.dry_run || !cfg.is_remote() {
        return None;
    }
    println!(
        "\n   WARNING: {stage} sends {what} to {} — a third-party service.\n   \
         Only use it on code you are allowed to share with it. --dry-run shows exactly\n   \
         what would be sent; --provider openai --base-url http://localhost:... keeps\n   \
         the code on this machine.\n",
        cfg.endpoint()
    );
    if !m.yes && !confirm_remote(&cfg.endpoint()) {
        println!("{stage}: not confirmed (pass --yes to send without asking) — nothing sent");
        return Some(1);
    }
    None
}

enum Model<'a> {
    Offline(CachedModel<'a>),
    Live(CachedModel<'a>),
}

impl<'a> Model<'a> {
    fn get(&self) -> &CachedModel<'a> {
        match self {
            Model::Offline(c) | Model::Live(c) => c,
        }
    }
}

// ───────────────────────────── ai-rename ─────────────────────────────────────

#[derive(Args, Debug, Clone)]
pub struct AiRenameArgs {
    /// the jar whose classes the source tree was decompiled from (after `decompile`
    /// ran its member renamer, that is OUT/_rename/<name>.renamed.jar)
    pub jar: String,
    /// decompiled source tree to show the model (e.g. OUT/merged); without it the
    /// model sees signatures only
    #[arg(long = "src")]
    pub src: Option<String>,
    /// output directory (name map, report, renamed jar)
    #[arg(short = 'o', long = "out", default_value = "ai_rename_out")]
    pub out: String,
    /// name map to start from / re-apply (default: OUT/ai-names.json if present)
    #[arg(long = "names")]
    pub names: Option<String>,
    /// do not ask the model; just validate and apply the name map as it stands
    #[arg(long = "apply-only")]
    pub apply_only: bool,
    /// ignore an existing name map (hand edits in it are lost)
    #[arg(long = "fresh")]
    pub fresh: bool,
    /// write the name map and report but do not rewrite the jar
    #[arg(long = "no-apply")]
    pub no_apply: bool,
    /// after applying, run the full `decompile` pipeline on the renamed jar
    #[arg(long = "decompile")]
    pub decompile: bool,
    /// do not resolve JDK supertypes with javap (they then count as unknown)
    #[arg(long = "no-javap")]
    pub no_javap: bool,
    #[command(flatten)]
    pub model: ModelArgs,
}

pub fn cmd_ai_rename(a: &AiRenameArgs) -> i32 {
    let jar = util::absolutize(Path::new(&a.jar));
    let out = util::absolutize(Path::new(&a.out));
    if !jar.exists() {
        println!("ai-rename: jar not found: {}", jar.display());
        return 2;
    }
    let src = a.src.as_ref().map(|s| util::absolutize(Path::new(s)));
    if let Some(s) = &src {
        if !s.is_dir() {
            println!("ai-rename: --src {} is not a directory", s.display());
            return 2;
        }
    }
    let _ = std::fs::create_dir_all(&out);
    println!("== Arlecchino ai-rename :: {} ==", jar.display());

    let classes = match classfile::read_jar(&jar) {
        Ok(c) => c,
        Err(e) => {
            println!("ai-rename: {e}");
            return 2;
        }
    };
    let ext = external::load(&classes, true, !a.no_javap);
    let manifest = classfile::manifest_classes(&jar);
    let world = World::new(&classes, &ext, &manifest);
    let unknown = external::unknown_supertypes(&classes, &ext);
    println!("   classes: {}   external types known: {}   unknown: {}", classes.len(), ext.known.len(), unknown.len());
    if !unknown.is_empty() {
        println!(
            "   note: unknown external supertypes ({}) — inheritable members of their subclasses stay locked",
            unknown.iter().take(5).cloned().collect::<Vec<_>>().join(", ")
        );
    }

    let jar_sha = crate::diffrun::sha256_hex(&std::fs::read(&jar).unwrap_or_default());
    let map_path = a.names.as_ref().map(|p| util::absolutize(Path::new(p))).unwrap_or_else(|| out.join("ai-names.json"));
    let mut map = if !a.fresh && map_path.exists() {
        match NameMap::load(&map_path) {
            Ok(m) => {
                if !m.jar_sha256.is_empty() && m.jar_sha256 != jar_sha {
                    println!("   warning: {} was made for a different jar; entries that do not match are rejected", map_path.display());
                }
                println!("   name map: {} ({} entries)", map_path.display(), m.entries.len());
                m
            }
            Err(e) => {
                println!("ai-rename: {e}");
                return 2;
            }
        }
    } else {
        NameMap { version: MAP_VERSION, jar_sha256: jar_sha.clone(), entries: vec![] }
    };
    map.jar_sha256 = jar_sha;

    let mut suggest_report = None;
    let mut usage = None;
    let mut cache_stats = (0, 0);
    let mut cfg_id = String::from("(none)");
    if !a.apply_only {
        let cfg = match a.model.resolve() {
            Ok(c) => c,
            Err(e) => {
                println!("ai-rename: {e}");
                return 2;
            }
        };
        cfg_id = cfg.id();
        let jobs = suggest::collect_jobs(&world, src.as_deref());
        let opt = suggest::Options { batch_tokens: a.model.batch_tokens, reply_tokens: cfg.max_tokens.min(8_000), ..Default::default() };
        let nsym: usize = jobs.iter().map(|j| j.symbols.len()).sum();
        let nbatches = suggest::batches(&jobs, &world, &opt).len();
        println!("   to name: {nsym} symbols in {} files -> {nbatches} request(s)", jobs.len());
        if src.is_none() {
            println!("   note: no --src — the model sees signatures only (names will be weaker)");
        }
        if let Some(code) = gate("ai-rename", "the decompiled source of the jar's obfuscated classes", &cfg, &a.model) {
            return code;
        }
        let mut glossary: BTreeMap<String, String> = map
            .entries
            .iter()
            .filter(|e| e.kind == names::Kind::Class)
            .map(|e| (e.owner.clone(), e.new.clone()))
            .collect();
        if a.model.dry_run {
            save_prompts(&out.join("ai-prompts"), &jobs, &world, &opt, &glossary);
        }
        let live: Option<LiveModel> = if a.model.dry_run || jobs.is_empty() {
            None
        } else {
            match cfg.build() {
                Ok(m) => Some(m),
                Err(e) => {
                    println!("ai-rename: {e}");
                    return 2;
                }
            }
        };
        let model = match &live {
            Some(m) => Model::Live(CachedModel::new(m as &dyn ChatModel, &a.model.cache_dir())),
            None => Model::Offline(CachedModel::offline(&cfg.id(), &a.model.cache_dir())),
        };
        let (entries, rep) = suggest::run(model.get(), &world, &jobs, &opt, &mut glossary);
        cache_stats = model.get().stats();
        usage = live.as_ref().map(|m| m.usage());
        let added = map.merge_new(entries);
        println!("   model proposed {added} new names ({} requests, {} cache hits)", rep.requests, cache_stats.0);
        suggest_report = Some(rep);
    }

    let plan = names::plan(&world, &map);
    if let Err(e) = plan.map.save(&map_path) {
        println!("ai-rename: {e}");
        return 2;
    }
    let pg = out.join("ai-names.mapping.txt");
    let _ = std::fs::write(&pg, names::proguard_mapping(&world, &plan));
    println!("   plan: {} applied, {} rejected -> {}", plan.applied, plan.rejected, map_path.display());

    let mut applied_jar = None;
    let apply_msg;
    if a.no_apply || a.model.dry_run {
        apply_msg = "not applied (--no-apply / --dry-run)".into();
    } else if plan.is_empty() {
        apply_msg = "nothing to apply".into();
    } else {
        let stem = jar.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let out_jar = out.join(format!("{stem}.ai-renamed.jar"));
        match apply::apply(&plan, &jar, &out_jar, None) {
            Ok(msg) => {
                println!("   applied: {msg} -> {}", out_jar.display());
                apply_msg = format!("{msg} -> `{}`", out_jar.display());
                applied_jar = Some(out_jar);
            }
            Err(e) => {
                println!("   apply FAILED: {e}");
                apply_msg = format!("FAILED: {e}");
            }
        }
    }

    let report = render_rename_report(&jar, &cfg_id, &plan, suggest_report.as_ref(), usage, cache_stats, &apply_msg);
    let _ = std::fs::write(out.join("ai-rename-report.md"), report);
    println!("   report: {}", out.join("ai-rename-report.md").display());
    if a.model.dry_run {
        println!("   dry run: prompts in {}", out.join("ai-prompts").display());
    }

    if a.decompile {
        if let Some(j) = &applied_jar {
            println!("\n== decompiling the renamed jar ==");
            let argv = vec![
                "decompile".to_string(),
                j.to_string_lossy().into_owned(),
                "-o".to_string(),
                out.join("decompiled").to_string_lossy().into_owned(),
            ];
            return crate::cli::main(Some(argv));
        }
    }
    if applied_jar.is_none() && !a.no_apply && !a.model.dry_run && !plan.is_empty() {
        return 1;
    }
    0
}

fn save_prompts(dir: &Path, jobs: &[suggest::Job], world: &World, opt: &suggest::Options, glossary: &BTreeMap<String, String>) {
    let _ = std::fs::create_dir_all(dir);
    for (i, b) in suggest::batches(jobs, world, opt).iter().enumerate() {
        let batch: Vec<&suggest::Job> = b.iter().map(|&k| &jobs[k]).collect();
        let (req, _) = suggest::build_request(&batch, world, glossary, opt);
        let text = format!("=== SYSTEM ===\n{}\n\n=== USER ===\n{}", req.system, req.messages[0].content);
        let _ = std::fs::write(dir.join(format!("batch-{:03}.txt", i + 1)), text);
    }
}

fn render_rename_report(
    jar: &Path,
    model: &str,
    plan: &names::Plan,
    rep: Option<&suggest::Report>,
    usage: Option<provider::Usage>,
    cache: (u64, u64),
    apply_msg: &str,
) -> String {
    let mut s = format!("# ai-rename report\n\n- jar: `{}`\n- model: `{model}`\n", jar.display());
    if let Some(r) = rep {
        s.push_str(&format!("- requests: {} (cache hits {}, misses {})\n", r.requests, cache.0, cache.1));
    }
    if let Some(u) = usage {
        s.push_str(&format!("- tokens spent: {} in / {} out over {} API calls\n", u.input_tokens, u.output_tokens, u.calls));
    }
    s.push_str(&format!(
        "- names: {} applied, {} rejected\n- remapped: {} classes, {} method refs, {} field refs\n- jar: {apply_msg}\n",
        plan.applied,
        plan.rejected,
        plan.classes.len(),
        plan.methods.len(),
        plan.fields.len()
    ));
    if let Some(r) = rep {
        if !r.truncated.is_empty() {
            s.push_str(&format!("\nSource shown CUT (too large for one request): {}\n", r.truncated.join(", ")));
        }
        let bad: Vec<String> = r
            .outcomes
            .iter()
            .filter_map(|(k, o)| match o {
                suggest::Outcome::Failed(e) => Some(format!("- `{k}`: {e}")),
                suggest::Outcome::NotCached => Some(format!("- `{k}`: not in cache (dry run)")),
                _ => None,
            })
            .collect();
        if !bad.is_empty() {
            s.push_str("\n## Classes without an answer\n\n");
            s.push_str(&bad.join("\n"));
            s.push('\n');
        }
    }
    s.push_str("\n## Names\n\n| kind | symbol | new | origin | status |\n|---|---|---|---|---|\n");
    let mut rows: Vec<&names::NameEntry> = plan.map.entries.iter().collect();
    rows.sort_by(|x, y| x.key().cmp(&y.key()));
    for e in rows {
        let sym = match e.kind {
            names::Kind::Class => e.owner.clone(),
            _ => format!("{}.{}{}", e.owner, e.name, if e.kind == names::Kind::Method { e.desc.as_str() } else { "" }),
        };
        s.push_str(&format!("| {:?} | `{sym}` | `{}` | {} | {} |\n", e.kind, e.new, e.origin, e.status.replace('|', "/")));
    }
    s.push_str(
        "\nEdit the name map (change `new`, set `origin` to `manual`, or delete an entry) and \
         re-run with `--names <map> --apply-only` to apply it again; every entry is re-validated.\n",
    );
    s
}

// ───────────────────────────── ai-annotate ───────────────────────────────────

#[derive(Args, Debug, Clone)]
pub struct AiAnnotateArgs {
    /// decompiled source tree (e.g. OUT/merged or an ai-rename decompile)
    pub src: String,
    /// where the annotated copy goes (the input tree is never modified)
    #[arg(short = 'o', long = "out", default_value = "ai_annotated")]
    pub out: String,
    #[command(flatten)]
    pub model: ModelArgs,
}

pub fn cmd_ai_annotate(a: &AiAnnotateArgs) -> i32 {
    let src = util::absolutize(Path::new(&a.src));
    let out = util::absolutize(Path::new(&a.out));
    if !src.is_dir() {
        println!("ai-annotate: {} is not a directory", src.display());
        return 2;
    }
    if out == src || out.starts_with(&src) {
        println!("ai-annotate: --out must be outside the source tree");
        return 2;
    }
    println!("== Arlecchino ai-annotate :: {} ==", src.display());
    let cfg = match a.model.resolve() {
        Ok(c) => c,
        Err(e) => {
            println!("ai-annotate: {e}");
            return 2;
        }
    };
    let files = annotate::java_files(&src);
    let budget = a.model.batch_tokens;
    let nreq: usize = files
        .iter()
        .map(|f| annotate::windows(&util::read_text_lossy(&src.join(f)).unwrap_or_default(), budget).len())
        .sum();
    println!("   files: {}   requests: {nreq}", files.len());
    if let Some(code) = gate("ai-annotate", "every source file of the tree", &cfg, &a.model) {
        return code;
    }
    let live: Option<LiveModel> = if a.model.dry_run {
        None
    } else {
        match cfg.build() {
            Ok(m) => Some(m),
            Err(e) => {
                println!("ai-annotate: {e}");
                return 2;
            }
        }
    };
    let model = match &live {
        Some(m) => Model::Live(CachedModel::new(m as &dyn ChatModel, &a.model.cache_dir())),
        None => Model::Offline(CachedModel::offline(&cfg.id(), &a.model.cache_dir())),
    };
    let reply_tokens = cfg.max_tokens.min(8_000);
    let mut report = String::from("# ai-annotate report\n\n");
    let (mut docs, mut failed, mut uncached) = (0usize, 0usize, 0usize);
    let _ = std::fs::create_dir_all(&out);
    if a.model.dry_run {
        let pdir = out.join("_ai-prompts");
        let _ = std::fs::create_dir_all(&pdir);
        for f in &files {
            let text = util::read_text_lossy(&src.join(f)).unwrap_or_default();
            let total = text.lines().count();
            for (i, w) in annotate::windows(&text, budget).into_iter().enumerate() {
                let rel = f.to_string_lossy();
                let req = annotate::build_request(&rel, &text, w, total, reply_tokens);
                let name = format!("{}.{}.txt", rel.replace(['/', '\\'], "_"), i + 1);
                let _ = std::fs::write(pdir.join(name), format!("=== SYSTEM ===\n{}\n\n=== USER ===\n{}", req.system, req.messages[0].content));
            }
        }
    }
    for (i, f) in files.iter().enumerate() {
        let rel = f.to_string_lossy().into_owned();
        let text = match util::read_text_lossy(&src.join(f)) {
            Ok(t) => t,
            Err(e) => {
                report.push_str(&format!("- `{rel}`: unreadable ({e})\n"));
                continue;
            }
        };
        eprintln!("   ai-annotate: {}/{} {rel}", i + 1, files.len());
        let (new_text, res) = annotate::annotate_file(model.get(), &rel, &text, budget, reply_tokens);
        let dst = out.join(f);
        if let Some(p) = dst.parent() {
            let _ = std::fs::create_dir_all(p);
        }
        let _ = std::fs::write(&dst, &new_text);
        docs += res.inserted;
        if res.not_cached {
            uncached += 1;
        }
        if let Some(e) = &res.error {
            failed += 1;
            report.push_str(&format!("- `{rel}`: model error: {e}\n"));
        }
        if res.inserted > 0 || !res.rejected.is_empty() {
            report.push_str(&format!("- `{rel}`: {} comments", res.inserted));
            if !res.rejected.is_empty() {
                report.push_str(&format!("; skipped: {}", res.rejected.join("; ")));
            }
            report.push('\n');
        }
    }
    let (hits, misses) = model.get().stats();
    let mut head = format!(
        "- source: `{}`\n- model: `{}`\n- files: {}, comments inserted: {docs}, files with errors: {failed}\n- cache hits {hits}, misses {misses}\n",
        src.display(),
        cfg.id(),
        files.len()
    );
    if let Some(m) = &live {
        let u = m.usage();
        head.push_str(&format!("- tokens spent: {} in / {} out over {} API calls\n", u.input_tokens, u.output_tokens, u.calls));
    }
    if uncached > 0 {
        head.push_str(&format!("- dry run: {uncached} files had no cached answer (prompts in `_ai-prompts/`)\n"));
    }
    head.push_str("- code lines are never changed: only whole comment lines are inserted\n\n");
    report.insert_str("# ai-annotate report\n\n".len(), &head);
    let _ = std::fs::write(out.join("ai-annotate-report.md"), report);
    println!("   comments inserted: {docs} in {} files -> {}", files.len(), out.display());
    println!("   report: {}", out.join("ai-annotate-report.md").display());
    if failed > 0 {
        1
    } else {
        0
    }
}
