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
pub mod mapfile;
pub mod names;
pub mod provider;
pub mod suggest;

use crate::ai_model::ChatModel;
use crate::util;
use clap::Args;
use names::{NameMap, World, MAP_VERSION};
use provider::{CachedModel, LiveModel, ProviderConfig, ProviderOverrides};
use std::collections::{BTreeMap, BTreeSet};
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
    /// decompiled source tree to show the model (e.g. OUT/merged, or the
    /// `decompiled/merged` of a previous run, which also shows local labels); without
    /// it the model sees signatures only
    #[arg(long = "src")]
    pub src: Option<String>,
    /// output directory (renamed jar, label table, report; the name map by default)
    #[arg(short = 'o', long = "out", default_value = "ai_rename_out")]
    pub out: String,
    /// name map: a `.map` file or a directory of them (Sporeflower format; default
    /// OUT/mappings).  A `.json` file is read as an older map and its names are
    /// carried into OUT/mappings
    #[arg(long = "names")]
    pub names: Option<String>,
    /// do not ask the model; just validate and apply the name map as it stands
    #[arg(long = "apply-only")]
    pub apply_only: bool,
    /// set the existing name map aside (renamed to *.bak-N) and start a new one
    #[arg(long = "fresh")]
    pub fresh: bool,
    /// write the name map and report but do not rewrite the jar
    #[arg(long = "no-apply")]
    pub no_apply: bool,
    /// leave unnamed locals to the decompilers (`var4`) instead of writing their
    /// labels (`v4`) into the jar; the next round then cannot name locals
    #[arg(long = "no-labels")]
    pub no_labels: bool,
    /// after applying, run the full `decompile` pipeline on the renamed jar
    #[arg(long = "decompile")]
    pub decompile: bool,
    /// extra argument for that `decompile` run (repeatable), e.g.
    /// --decompile-arg=--config --decompile-arg=base8.toml
    #[arg(long = "decompile-arg", allow_hyphen_values = true)]
    pub decompile_args: Vec<String>,
    /// do not resolve JDK supertypes with javap (they then count as unknown)
    #[arg(long = "no-javap")]
    pub no_javap: bool,
    #[command(flatten)]
    pub model: ModelArgs,
}

/// `apply-names`: a name map written by a person, applied without any model.
#[derive(Args, Debug, Clone)]
pub struct ApplyNamesArgs {
    /// the jar the map was written for (OUT/_rename/<name>.renamed.jar after `decompile`)
    pub jar: String,
    /// the name map: a `.map` file or a directory of them
    #[arg(long = "names", short = 'n')]
    pub names: String,
    /// output directory (renamed jar, label table, report)
    #[arg(short = 'o', long = "out", default_value = "names_out")]
    pub out: String,
    /// see `ai-rename --no-labels`
    #[arg(long = "no-labels")]
    pub no_labels: bool,
    /// add declarations for every symbol the map does not list yet (unnamed), so the
    /// file becomes a complete worksheet
    #[arg(long = "complete")]
    pub complete: bool,
    /// after applying, run the full `decompile` pipeline on the renamed jar
    #[arg(long = "decompile")]
    pub decompile: bool,
    /// extra argument for that `decompile` run (repeatable)
    #[arg(long = "decompile-arg", allow_hyphen_values = true)]
    pub decompile_args: Vec<String>,
    /// do not resolve JDK supertypes with javap
    #[arg(long = "no-javap")]
    pub no_javap: bool,
}

pub fn cmd_apply_names(a: &ApplyNamesArgs) -> i32 {
    let r = AiRenameArgs {
        jar: a.jar.clone(),
        src: None,
        out: a.out.clone(),
        names: Some(a.names.clone()),
        apply_only: true,
        fresh: false,
        no_apply: false,
        no_labels: a.no_labels,
        decompile: a.decompile,
        decompile_args: a.decompile_args.clone(),
        no_javap: a.no_javap,
        model: ModelArgs::default(),
    };
    run_rename(&r, a.complete)
}

pub fn cmd_ai_rename(a: &AiRenameArgs) -> i32 {
    run_rename(a, true)
}

/// Readable-name coverage of a jar under a plan.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Coverage {
    /// (readable, total)
    pub classes: (usize, usize),
    pub fields: (usize, usize),
    pub methods: (usize, usize),
    pub locals: (usize, usize),
}

fn pct(x: (usize, usize)) -> String {
    if x.1 == 0 {
        "—".into()
    } else {
        format!("{}/{} ({:.0} %)", x.0, x.1, 100.0 * x.0 as f64 / x.1 as f64)
    }
}

/// A name is readable when the plan gave it one, or it never looked machine-made,
/// or it is fixed by a framework method it overrides (`run`, `paint`).
pub fn coverage(world: &World, plan: Option<&names::Plan>, labels: &[mapfile::Label]) -> Coverage {
    let mut c = Coverage::default();
    let overrides_external = |owner: &str, n: &str, d: &str| {
        world.ancestors(owner).iter().filter(|a| !world.in_jar(a)).any(|a| world.ext.known.get(a).map(|ci| ci.methods.iter().any(|m| m.name == n && m.desc == d)).unwrap_or(false))
    };
    for (cn, ci) in world.classes {
        let simple = names::simple_name(cn);
        if !simple.chars().all(|ch| ch.is_ascii_digit()) {
            let named = plan.map(|p| p.classes.contains_key(cn)).unwrap_or(false);
            c.classes.1 += 1;
            if named || !names::looks_obfuscated(simple) {
                c.classes.0 += 1;
            }
        }
        for f in ci.fields.iter().filter(|f| !f.is_synthetic()) {
            c.fields.1 += 1;
            let named = plan.map(|p| p.fields.contains_key(&(cn.clone(), f.name.clone()))).unwrap_or(false);
            if named || !names::looks_obfuscated(&f.name) {
                c.fields.0 += 1;
            }
        }
        for m in ci.methods.iter().filter(|m| !m.is_synthetic() && m.name != "<init>" && m.name != "<clinit>") {
            c.methods.1 += 1;
            let named = plan.map(|p| p.methods.contains_key(&(cn.clone(), m.name.clone(), m.desc.clone()))).unwrap_or(false);
            if named || !names::looks_obfuscated(&m.name) || overrides_external(cn, &m.name, &m.desc) {
                c.methods.0 += 1;
            }
        }
    }
    for l in labels {
        c.locals.1 += 1;
        if plan.map(|p| p.locals.contains_key(&(l.owner.clone(), l.method.clone(), l.desc.clone(), l.label.clone()))).unwrap_or(false) {
            c.locals.0 += 1;
        }
    }
    c
}

fn set_aside(path: &Path) -> Option<PathBuf> {
    if !path.exists() {
        return None;
    }
    for i in 1.. {
        let bak = PathBuf::from(format!("{}.bak-{i}", path.display()));
        if !bak.exists() {
            return std::fs::rename(path, &bak).ok().map(|_| bak);
        }
    }
    None
}

fn run_rename(a: &AiRenameArgs, complete: bool) -> i32 {
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

    // ── the name map ────────────────────────────────────────────────────────
    let jar_sha = crate::diffrun::sha256_hex(&std::fs::read(&jar).unwrap_or_default());
    let names_path = a.names.as_ref().map(|p| util::absolutize(Path::new(p))).unwrap_or_else(|| out.join("mappings"));
    let legacy = names_path.extension().map(|e| e == "json").unwrap_or(false);
    let map_target = if legacy { out.join("mappings") } else { names_path.clone() };
    if a.fresh {
        if let Some(b) = set_aside(&map_target) {
            println!("   --fresh: previous map set aside as {}", b.display());
        }
    }
    let mut map_errors: Vec<String> = Vec::new();
    let mut map = NameMap { version: MAP_VERSION, jar_sha256: jar_sha.clone(), entries: vec![] };
    if legacy && !a.fresh && names_path.exists() {
        match NameMap::load(&names_path) {
            Ok(m) => {
                if !m.jar_sha256.is_empty() && m.jar_sha256 != jar_sha {
                    println!("   warning: {} was made for a different jar; entries that do not match are rejected", names_path.display());
                }
                map.entries = m.entries;
            }
            Err(e) => {
                println!("ai-rename: {e}");
                return 2;
            }
        }
    }
    let files = match mapfile::load(&map_target) {
        Ok(f) => f,
        Err(e) => {
            println!("ai-rename: {e}");
            return 2;
        }
    };
    let semantic: usize = files.iter().map(|f| f.semantic).sum();
    {
        let (entries, errs) = mapfile::entries(&world, &files);
        map_errors.extend(errs);
        map.merge_new(entries);
    }
    if !files.is_empty() {
        println!("   name map: {} ({} file(s), {} names, {} semantic annotation(s))", map_target.display(), files.len(), map.entries.len(), semantic);
    } else if a.names.is_some() && !legacy {
        println!("ai-rename: no .map files at {}", map_target.display());
        if a.apply_only {
            return 2;
        }
    }
    for e in map_errors.iter().take(20) {
        println!("   map: {e}");
    }
    if map_errors.len() > 20 {
        println!("   map: … {} more (see the report)", map_errors.len() - 20);
    }
    let labels_path = out.join("labels.tsv");
    let mut labels = mapfile::read_labels(&labels_path);
    let before = coverage(&world, None, &labels);

    // ── the model ───────────────────────────────────────────────────────────
    let mut suggest_report = None;
    let mut usage = None;
    let mut cache_stats = (0, 0);
    let mut cfg_id = String::from("(none)");
    let mut auto: BTreeSet<String> = BTreeSet::new();
    if !a.apply_only {
        let cfg = match a.model.resolve() {
            Ok(c) => c,
            Err(e) => {
                println!("ai-rename: {e}");
                return 2;
            }
        };
        cfg_id = cfg.id();
        let pre = names::plan(&world, &map);
        // a source tree decompiled from the renamed jar shows the new names
        let renamed_src = src.as_ref().map(|s| pre.classes.values().any(|c| s.join(format!("{c}.java")).exists())).unwrap_or(false);
        let ctx = suggest::Context {
            plan: if renamed_src { Some(&pre) } else { None },
            labels: &labels,
            named: map.entries.iter().map(|e| e.key()).collect(),
        };
        let jobs = suggest::collect_jobs_with(&world, src.as_deref(), &ctx);
        let opt = suggest::Options { batch_tokens: a.model.batch_tokens, reply_tokens: cfg.max_tokens.min(16_000), ..Default::default() };
        let nsym: usize = jobs.iter().map(|j| j.symbols.len()).sum();
        let nloc: usize = jobs.iter().map(|j| j.symbols.iter().filter(|s| s.kind == names::Kind::Local).count()).sum();
        let nbatches = suggest::batches(&jobs, &world, &opt).len();
        println!("   to name: {nsym} symbols ({nloc} parameters/locals) in {} files -> {nbatches} request(s)", jobs.len());
        if src.is_none() {
            println!("   note: no --src — the model sees signatures only (names will be weaker)");
        } else if nloc == 0 && !labels.is_empty() {
            println!("   note: the source shows no local labels — pass the `decompiled/merged` of the previous run to name locals");
        }
        if let Some(code) = gate("ai-rename", "the decompiled source of the jar's obfuscated classes", &cfg, &a.model) {
            return code;
        }
        let mut glossary: BTreeMap<String, String> = pre.classes.iter().map(|(o, n)| (o.clone(), names::simple_name(n).to_string())).collect();
        if a.model.dry_run {
            save_prompts(&out.join("ai-prompts"), &jobs, &world, &opt, &glossary, ctx.plan);
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
        let (entries, rep) = suggest::run_with(model.get(), &world, &jobs, &opt, &mut glossary, ctx.plan);
        cache_stats = model.get().stats();
        usage = live.as_ref().map(|m| m.usage());
        for e in &entries {
            match e.kind {
                names::Kind::Field => auto.insert(format!("f {} {}", e.owner, e.name)),
                names::Kind::Method => auto.insert(format!("m {} {}{}", e.owner, e.name, e.desc)),
                _ => false,
            };
        }
        let added = map.merge_new(entries);
        println!("   model proposed {added} new names ({} requests, {} cache hits)", rep.requests, cache_stats.0);
        suggest_report = Some(rep);
    }

    let plan = names::plan(&world, &map);
    if let Err(e) = plan.map.save(&out.join("ai-names.json")) {
        println!("ai-rename: {e}");
        return 2;
    }
    let pg = out.join("ai-names.mapping.txt");
    let _ = std::fs::write(&pg, names::proguard_mapping(&world, &plan));
    println!("   plan: {} applied, {} rejected", plan.applied, plan.rejected);

    // ── the jar ─────────────────────────────────────────────────────────────
    let mut applied_jar = None;
    let mut apply_msg;
    if a.no_apply || a.model.dry_run {
        apply_msg = "not applied (--no-apply / --dry-run)".to_string();
    } else if plan.is_empty() && a.no_labels {
        apply_msg = "nothing to apply".into();
    } else {
        let stem = jar.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let out_jar = out.join(format!("{stem}.ai-renamed.jar"));
        let lo = apply::LocalOptions { label_all: !a.no_labels, labels_out: Some(labels_path.clone()) };
        match apply::apply_with(&plan, &jar, &out_jar, None, &lo) {
            Ok(msg) => {
                let mut lines = msg.lines();
                let head = lines.next().unwrap_or("").to_string();
                println!("   applied: {head} -> {}", out_jar.display());
                apply_msg = format!("{head} -> `{}`", out_jar.display());
                let warns: Vec<&str> = lines.collect();
                if !warns.is_empty() {
                    println!("   {} applier warning(s), see the report", warns.len());
                    apply_msg.push_str("\n\n```\n");
                    apply_msg.push_str(&warns.join("\n"));
                    apply_msg.push_str("\n```");
                }
                applied_jar = Some(out_jar);
                labels = mapfile::read_labels(&labels_path);
            }
            Err(e) => {
                println!("   apply FAILED: {e}");
                apply_msg = format!("FAILED: {e}");
            }
        }
    }

    // ── the map, brought up to date without touching what a person wrote ───
    let mut want = mapfile::wanted(&world, &plan, &labels);
    if !complete {
        // only what the files or the plan name — no worksheet lines for the rest
        want.classes.retain(|c, n| plan.classes.contains_key(c) || n != names::simple_name(c));
        want.fields.retain(|(o, f, _), _| plan.fields.contains_key(&(o.clone(), f.clone())));
        want.methods.retain(|k, _| plan.methods.contains_key(k) || plan.locals.keys().any(|(o, m, d, _)| (o, m, d) == (&k.0, &k.1, &k.2)));
        for (k, lw) in want.locals.iter_mut() {
            lw.retain(|l, _| plan.locals.contains_key(&(k.0.clone(), k.1.clone(), k.2.clone(), l.clone())));
        }
        want.locals.retain(|_, lw| !lw.is_empty());
        let keep: BTreeSet<String> = want.methods.keys().map(|k| k.0.clone()).chain(want.fields.keys().map(|k| k.0.clone())).chain(want.locals.keys().map(|k| k.0.clone())).collect();
        want.classes.retain(|c, _| plan.classes.contains_key(c) || keep.contains(c));
    }
    let changes = mapfile::patch(&world, &plan, &files, &want, &map_target, &auto);
    if !a.model.dry_run {
        if !map_target.extension().map(|e| e == "map").unwrap_or(false) {
            let _ = std::fs::create_dir_all(&map_target);
        }
        for (p, text) in &changes {
            if let Err(e) = std::fs::write(p, text) {
                println!("ai-rename: {}: {e}", p.display());
                return 2;
            }
        }
    }
    println!("   name map: {} file(s) updated -> {}", changes.len(), map_target.display());
    let after = coverage(&world, Some(&plan), &labels);
    let before = Coverage { locals: (0, after.locals.1.max(before.locals.1)), ..before };
    println!(
        "   readable names before -> after: classes {} -> {}, methods {} -> {}, fields {} -> {}, parameters/locals {} -> {}",
        pct(before.classes),
        pct(after.classes),
        pct(before.methods),
        pct(after.methods),
        pct(before.fields),
        pct(after.fields),
        pct(before.locals),
        pct(after.locals)
    );
    if semantic > 0 {
        println!("   note: {semantic} semantic annotation(s) (domains/flags) in the map — applied by Sporeflower (`sporeflower {} OUT --mappings {}`), not by the bytecode applier", jar.display(), map_target.display());
    }

    let report = render_rename_report(&jar, &cfg_id, &plan, suggest_report.as_ref(), usage, cache_stats, &apply_msg, &map_target, &map_errors, (before, after), semantic);
    let _ = std::fs::write(out.join("ai-rename-report.md"), report);
    println!("   report: {}", out.join("ai-rename-report.md").display());
    if a.model.dry_run {
        println!("   dry run: prompts in {}", out.join("ai-prompts").display());
    }

    if a.decompile {
        if let Some(j) = &applied_jar {
            println!("\n== decompiling the renamed jar ==");
            let mut argv = vec![
                "decompile".to_string(),
                j.to_string_lossy().into_owned(),
                "-o".to_string(),
                out.join("decompiled").to_string_lossy().into_owned(),
            ];
            argv.extend(a.decompile_args.iter().cloned());
            return crate::cli::main(Some(argv));
        }
    }
    if applied_jar.is_none() && !a.no_apply && !a.model.dry_run && !plan.is_empty() {
        return 1;
    }
    0
}

fn save_prompts(dir: &Path, jobs: &[suggest::Job], world: &World, opt: &suggest::Options, glossary: &BTreeMap<String, String>, plan: Option<&names::Plan>) {
    let _ = std::fs::create_dir_all(dir);
    for (i, b) in suggest::batches(jobs, world, opt).iter().enumerate() {
        let batch: Vec<&suggest::Job> = b.iter().map(|&k| &jobs[k]).collect();
        let (req, _) = suggest::build_request_with(&batch, world, glossary, opt, plan);
        let text = format!("=== SYSTEM ===\n{}\n\n=== USER ===\n{}", req.system, req.messages[0].content);
        let _ = std::fs::write(dir.join(format!("batch-{:03}.txt", i + 1)), text);
    }
}

#[allow(clippy::too_many_arguments)]
fn render_rename_report(
    jar: &Path,
    model: &str,
    plan: &names::Plan,
    rep: Option<&suggest::Report>,
    usage: Option<provider::Usage>,
    cache: (u64, u64),
    apply_msg: &str,
    map_target: &Path,
    map_errors: &[String],
    cov: (Coverage, Coverage),
    semantic: usize,
) -> String {
    let mut s = format!("# ai-rename report\n\n- jar: `{}`\n- model: `{model}`\n- name map: `{}`\n", jar.display(), map_target.display());
    if let Some(r) = rep {
        s.push_str(&format!("- requests: {} (cache hits {}, misses {})\n", r.requests, cache.0, cache.1));
    }
    if let Some(u) = usage {
        s.push_str(&format!("- tokens spent: {} in / {} out over {} API calls\n", u.input_tokens, u.output_tokens, u.calls));
    }
    s.push_str(&format!(
        "- names: {} applied, {} rejected\n- remapped: {} classes, {} method refs, {} field refs, {} parameters/locals\n- jar: {apply_msg}\n",
        plan.applied,
        plan.rejected,
        plan.classes.len(),
        plan.methods.len(),
        plan.fields.len(),
        plan.locals.len()
    ));
    let (b, a) = cov;
    s.push_str(&format!(
        "\n## Readable names\n\n| | before | after |\n|---|---|---|\n| classes | {} | {} |\n| methods | {} | {} |\n| fields | {} | {} |\n| parameters/locals | {} | {} |\n",
        pct(b.classes),
        pct(a.classes),
        pct(b.methods),
        pct(a.methods),
        pct(b.fields),
        pct(a.fields),
        pct(b.locals),
        pct(a.locals)
    ));
    if semantic > 0 {
        s.push_str(&format!(
            "\nThe map carries {semantic} semantic annotation(s) (domains, flags). The bytecode applier keeps \
             literals as they are; Sporeflower presents them: `sporeflower <jar> <out> --mappings {}`.\n",
            map_target.display()
        ));
    }
    if !map_errors.is_empty() {
        s.push_str("\n## Map declarations that do not match the jar\n\n");
        for e in map_errors {
            s.push_str(&format!("- {e}\n"));
        }
    }
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
            names::Kind::Local => format!("{}.{}{} {}", e.owner, e.name, e.desc, e.local),
            _ => format!("{}.{}{}", e.owner, e.name, if e.kind == names::Kind::Method { e.desc.as_str() } else { "" }),
        };
        s.push_str(&format!("| {:?} | `{sym}` | `{}` | {} | {} |\n", e.kind, e.new, e.origin, e.status.replace('|', "/")));
    }
    s.push_str(
        "\nThe name map is the source of truth: edit any name in it (a name that differs from its \
         `was` is never overwritten), then apply it again with `arlecchino apply-names <jar> --names <map>` \
         or `ai-rename --apply-only`; every entry is re-validated against the jar.\n",
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
