//! The golden corpus and the **differential runner** — the deliverable of Fase 1.
//!
//! `tests/golden/corpus.jsonl` (generated from the Python bot by `tools/recon/gen_corpus.py`)
//! records, for every turn of 2 850 sessions: the input, the reply, the winning module, the
//! candidate modules with their priorities, the memory keys and a SHA-1 digest of the full
//! memory. The runner replays the *same* sessions in the Rust bot and compares.
//!
//! # What it measures
//!
//! Per turn, five independent signals:
//!
//! | signal | meaning |
//! |---|---|
//! | `reply` | the normalised reply (`" ".join(reply.split())`, Python's own normalisation) |
//! | `module` | the winning module (the main debugging signal, GOAL.md Fase 7) |
//! | `candidates` | the proposal list: which modules replied, and at what priority |
//! | `memory_keys` | the sorted keys of the post-turn memory |
//! | `memory_digest` | SHA-1 of the full post-turn memory, canonical JSON |
//!
//! Equivalence is *not* expected in Fase 1 (GOAL.md: "0 % de equivalência é aceitável; o
//! harness é o entregável"): most modules are skeletons. Fase 1 is green because **the harness
//! runs, is deterministic, and tells the truth about the gap**.
//!
//! # What it reports
//!
//! [`GoldenReport`] aggregates by *block* (`por diretório`: the corpus was generated in named
//! blocks and session ids encode them — see [`Block`]), by *expected winning module*, by
//! *module appearing in candidates*, and lists grouped diffs — at most
//! [`MAX_GROUPED_DIFFS`] examples per group, so a 3 000-turn mismatch cannot flood the output.
//!
//! The runner never uses time, randomness or hash order: turns are compared in file order and
//! every aggregate is a sorted `Vec`/`BTreeMap`.

use std::collections::BTreeMap;

use crate::error::{Error, Result};
use crate::json::{self, Json};
use crate::limits;
use crate::pyutil;
use crate::session::Session;
use crate::types::{Priority, Turn};

/// One recorded turn of the corpus.
#[derive(Clone, Debug, PartialEq)]
pub struct GoldenTurn {
    /// Session id (`s0000`).
    pub session: String,
    /// 0-based index inside the session.
    pub turn: u32,
    /// The exact text fed to `runtime.turn`.
    pub input: String,
    /// The normalised reply.
    pub reply: String,
    /// The winning module (`None` when the fallback answered).
    pub module: Option<String>,
    /// Candidate modules in arrival order, with their priorities.
    pub candidates: Vec<(String, Priority)>,
    /// Sorted post-turn memory keys.
    pub memory_keys: Vec<String>,
    /// SHA-1 of the canonical full post-turn memory.
    pub memory_digest: String,
}

/// The corpus manifest (`tests/golden/manifest.json`).
#[derive(Clone, Debug, PartialEq)]
pub struct Manifest {
    /// Manifest schema version.
    pub version: i64,
    /// Generator name.
    pub created_by: String,
    /// Seed used per session.
    pub corpus_seed: i64,
    /// `PYTHONHASHSEED` of the Python run.
    pub pythonhashseed: String,
    /// Python version.
    pub python: String,
    /// Number of sessions.
    pub sessions: u64,
    /// Number of turns (must equal the number of lines).
    pub turns: u64,
    /// SHA-256 of `corpus.jsonl` (hex).
    pub corpus_sha256: String,
}

/// The whole corpus, in file order.
#[derive(Clone, Debug, Default)]
pub struct Corpus {
    /// Turns in file order (session order, then turn order).
    pub turns: Vec<GoldenTurn>,
    /// The manifest, when one was loaded and validated.
    pub manifest: Option<Manifest>,
}

/// Where a session belongs in the corpus' structure.
///
/// The generator emits blocks in order, and `sNNNN` ids are assigned sequentially, so the
/// block is derivable from the session number. Boundaries were measured from the corpus
/// (`tools/recon/gen_corpus.py` block sizes; `tests/golden` pins them):
///
/// | sessions | turns | block |
/// |---|---|---|
/// | s0000–s0299 | 512 | hand-written prompts (incl. edge cases), scripted multi-turn sessions, templated sessions |
/// | s0300–s1003 | 704 | fuzzed mutations of seed prompts, plus pure noise |
/// | s1004–s2849 | 1 846 | knowledge probes (one question per entry, round-robin across domains) |
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Block {
    /// Hand-written prompts, scripted multi-turn sessions, templated sessions, repetitions.
    Curated,
    /// Fuzzed mutations of seed prompts, plus pure noise.
    Fuzz,
    /// Knowledge-base probes (one question per entry, round-robin across domains).
    Knowledge,
}

impl Block {
    /// Every block, in corpus order.
    pub const ALL: [Self; 3] = [Self::Curated, Self::Fuzz, Self::Knowledge];

    /// The block's directory-like name (the report's group key).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Curated => "curated",
            Self::Fuzz => "fuzz",
            Self::Knowledge => "knowledge_probes",
        }
    }

    /// The block of a canonical session number.
    #[must_use]
    pub const fn of_session(number: u32) -> Self {
        match number {
            0..=299 => Self::Curated,
            300..=1_003 => Self::Fuzz,
            _ => Self::Knowledge,
        }
    }
}

impl GoldenTurn {
    /// The block this turn belongs to (any non-canonical id counts as curated).
    #[must_use]
    pub fn block(&self) -> Block {
        self.session_number()
            .map_or(Block::Curated, Block::of_session)
    }

    /// The session's numeric part, when canonical (`s` + exactly four digits).
    #[must_use]
    pub fn session_number(&self) -> Option<u32> {
        let digits = self.session.strip_prefix('s')?;
        if digits.len() != 4 || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        digits.parse().ok()
    }
}

impl Corpus {
    /// Parse `corpus.jsonl` contents (one JSON object per line; blank lines are ignored).
    ///
    /// Validates the schema the runner relies on and the session structure (contiguous turns
    /// inside a session, sessions in ascending id order). Errors carry the 1-based line number.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let text = std::str::from_utf8(bytes).map_err(|error| Error::InvalidUtf8 {
            valid_up_to: error.valid_up_to(),
        })?;
        let mut turns = Vec::new();
        let mut previous_session: Option<String> = None;
        let mut expected_turn: u32 = 0;
        for (index, line) in text.lines().enumerate() {
            let line_number = index + 1;
            if line.trim().is_empty() {
                continue;
            }
            if line.len() > limits::MAX_JSON_LINE_BYTES {
                return Err(Error::Corpus {
                    line: line_number,
                    message: "line exceeds MAX_JSON_LINE_BYTES",
                });
            }
            let turn = parse_turn(line.as_bytes(), line_number)?;
            match previous_session.as_deref() {
                Some(previous) if previous == turn.session => {
                    if turn.turn != expected_turn {
                        return Err(Error::Corpus {
                            line: line_number,
                            message: "turn indices are not contiguous inside a session",
                        });
                    }
                }
                Some(previous) => {
                    if previous > turn.session.as_str() {
                        return Err(Error::Corpus {
                            line: line_number,
                            message: "sessions are not in ascending id order",
                        });
                    }
                    if turn.turn != 0 {
                        return Err(Error::Corpus {
                            line: line_number,
                            message: "a new session must start at turn 0",
                        });
                    }
                    expected_turn = 0;
                }
                None => {
                    if turn.turn != 0 {
                        return Err(Error::Corpus {
                            line: line_number,
                            message: "a new session must start at turn 0",
                        });
                    }
                }
            }
            expected_turn = expected_turn.saturating_add(1);
            previous_session = Some(turn.session.clone());
            turns.push(turn);
        }
        if turns.is_empty() {
            return Err(Error::Corpus {
                line: 1,
                message: "corpus is empty",
            });
        }
        Ok(Self {
            turns,
            manifest: None,
        })
    }

    /// Parse the manifest next to a corpus.
    pub fn parse_manifest(bytes: &[u8]) -> Result<Manifest> {
        let value = json::parse(bytes)?;
        let missing = |field: &'static str| Error::Manifest { message: field };
        let string = |key: &str| value.get(key).and_then(Json::as_str);
        let number = |key: &str| value.get(key).and_then(Json::as_i64);
        let sessions = number("sessions").ok_or_else(|| missing("missing sessions"))?;
        let turns = number("turns").ok_or_else(|| missing("missing turns"))?;
        Ok(Manifest {
            version: number("version").ok_or_else(|| missing("missing version"))?,
            created_by: string("created_by")
                .ok_or_else(|| missing("missing created_by"))?
                .to_string(),
            corpus_seed: number("corpus_seed").ok_or_else(|| missing("missing corpus_seed"))?,
            pythonhashseed: string("pythonhashseed").unwrap_or("0").to_string(),
            python: string("python").unwrap_or("unknown").to_string(),
            sessions: u64::try_from(sessions).map_err(|_| missing("negative sessions"))?,
            turns: u64::try_from(turns).map_err(|_| missing("negative turns"))?,
            corpus_sha256: string("corpus_sha256")
                .ok_or_else(|| missing("missing corpus_sha256"))?
                .to_string(),
        })
    }

    /// Install a manifest parsed separately, validating it against the parsed turns.
    ///
    /// The turn/session counts alone would accept a corpus that was *edited* consistently, so
    /// the manifest's `corpus_sha256` is checked against the exact bytes the turns came from.
    /// `corpus_bytes` is `None` when the caller parsed a buffer it no longer has: the counts
    /// are still checked, and the digest is skipped rather than guessed.
    pub fn with_manifest(self, manifest: Manifest) -> Result<Self> {
        self.with_manifest_and_bytes(manifest, None)
    }

    /// [`Self::with_manifest`] plus the corpus bytes the manifest's digest covers.
    pub fn with_manifest_and_bytes(
        mut self,
        manifest: Manifest,
        corpus_bytes: Option<&[u8]>,
    ) -> Result<Self> {
        if let Some(bytes) = corpus_bytes {
            let digest = crate::hash::sha256(&[bytes]).hex();
            if digest != manifest.corpus_sha256 {
                return Err(Error::Manifest {
                    message: "manifest corpus_sha256 does not match corpus.jsonl",
                });
            }
        }
        let turns = u64::try_from(self.len()).unwrap_or(u64::MAX);
        if manifest.turns != turns {
            return Err(Error::Manifest {
                message: "manifest turn count does not match the corpus",
            });
        }
        let sessions = u64::try_from(self.session_count()).unwrap_or(u64::MAX);
        if manifest.sessions != sessions {
            return Err(Error::Manifest {
                message: "manifest session count does not match the corpus",
            });
        }
        self.manifest = Some(manifest);
        Ok(self)
    }

    /// Number of turns.
    #[must_use]
    pub fn len(&self) -> usize {
        self.turns.len()
    }

    /// True when there are no turns.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.turns.is_empty()
    }

    /// Number of distinct sessions, in file order.
    #[must_use]
    pub fn session_count(&self) -> usize {
        let mut count = 0;
        let mut previous: Option<&str> = None;
        for turn in &self.turns {
            if previous != Some(turn.session.as_str()) {
                count += 1;
                previous = Some(turn.session.as_str());
            }
        }
        count
    }

    /// Sessions as `(id, turns)` slices, in file order.
    #[must_use]
    pub fn sessions(&self) -> Vec<(&str, &[GoldenTurn])> {
        let mut sessions = Vec::new();
        let mut start = 0;
        while start < self.turns.len() {
            let id = self
                .turns
                .get(start)
                .map_or("", |turn| turn.session.as_str());
            let mut end = start + 1;
            while end < self.turns.len()
                && self.turns.get(end).map(|turn| turn.session.as_str()) == Some(id)
            {
                end += 1;
            }
            if let Some(slice) = self.turns.get(start..end) {
                sessions.push((id, slice));
            }
            start = end;
        }
        sessions
    }

    /// The number of turns per block, measured from the data.
    #[must_use]
    pub fn block_counts(&self) -> BTreeMap<&'static str, usize> {
        let mut counts: BTreeMap<&'static str, usize> =
            Block::ALL.iter().map(|block| (block.as_str(), 0)).collect();
        for turn in &self.turns {
            if let Some(slot) = counts.get_mut(turn.block().as_str()) {
                *slot += 1;
            }
        }
        counts
    }
}

/// Parse and validate one corpus line.
fn parse_turn(line: &[u8], line_number: usize) -> Result<GoldenTurn> {
    let error = |message: &'static str| Error::Corpus {
        line: line_number,
        message,
    };
    let value = json::parse(line).map_err(|failure| match failure {
        Error::Json { message, .. } => error(message),
        other => other,
    })?;
    let field = |key: &str| value.get(key).ok_or_else(|| error("missing field"));
    let session = field("session")?
        .as_str()
        .ok_or_else(|| error("session is not a string"))?;
    let turn = field("turn")?
        .as_i64()
        .ok_or_else(|| error("turn is not an int"))?;
    let input = field("input")?
        .as_str()
        .ok_or_else(|| error("input is not a string"))?;
    let reply = field("reply")?
        .as_str()
        .ok_or_else(|| error("reply is not a string"))?;
    let module = match field("module")? {
        Json::Null => None,
        Json::Str(text) => Some(text.clone()),
        _ => return Err(error("module is neither string nor null")),
    };
    let mut candidates = Vec::new();
    for candidate in field("candidates")?
        .as_array()
        .ok_or_else(|| error("candidates is not an array"))?
    {
        let name = candidate
            .get("module")
            .and_then(Json::as_str)
            .ok_or_else(|| error("candidate has no module name"))?;
        let priority = match candidate.get("priority") {
            Some(Json::Int(value)) => Priority::Int(*value),
            Some(Json::Float(value)) => {
                Priority::from_f64(*value).ok_or_else(|| error("candidate priority is NaN"))?
            }
            _ => return Err(error("candidate priority is not a number")),
        };
        candidates.push((name.to_string(), priority));
    }
    let mut memory_keys = Vec::new();
    for key in field("memory_keys")?
        .as_array()
        .ok_or_else(|| error("memory_keys is not an array"))?
    {
        memory_keys.push(
            key.as_str()
                .ok_or_else(|| error("memory_keys holds a non-string"))?
                .to_string(),
        );
    }
    let memory_digest = field("memory_digest")?
        .as_str()
        .ok_or_else(|| error("memory_digest is not a string"))?;
    if memory_digest.len() != 40 || !memory_digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(error("memory_digest is not a sha1 hex digest"));
    }
    Ok(GoldenTurn {
        session: session.to_string(),
        turn: u32::try_from(turn).map_err(|_| error("turn index out of range"))?,
        input: input.to_string(),
        reply: reply.to_string(),
        module,
        candidates,
        memory_keys,
        memory_digest: memory_digest.to_ascii_lowercase(),
    })
}

// ---------------------------------------------------------------------------- comparison

/// How one turn compares with the corpus.
///
/// Five independent signals, one bool each (reply, module, candidates, memory keys, memory
/// digest): grouping them would only hide which one disagreed.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TurnComparison {
    /// Index in the corpus file.
    pub index: usize,
    /// Session id.
    pub session: String,
    /// Turn index inside the session.
    pub turn: u32,
    /// The block the turn belongs to.
    pub block: Block,
    /// The text of the turn (truncated for storage in reports).
    pub input: String,
    /// The expected winning module.
    pub expected_module: Option<String>,
    /// The module the Rust bot picked.
    pub actual_module: Option<String>,
    /// Modules in the expected candidate list, in order.
    pub expected_candidates: Vec<String>,
    /// Modules in the actual candidate list, in order.
    pub actual_candidates: Vec<String>,
    /// The expected reply.
    pub expected_reply: String,
    /// The reply the Rust bot produced, normalised.
    pub actual_reply: String,
    /// Whether the reply matched.
    pub reply_matches: bool,
    /// Whether the winning module matched.
    pub module_matches: bool,
    /// Whether the candidate list matched (modules and priorities, in order).
    pub candidates_match: bool,
    /// First difference between the candidate lists, when they differ.
    pub candidates_diff: Option<String>,
    /// Whether the memory keys matched.
    pub memory_keys_match: bool,
    /// Whether the memory digest matched.
    pub memory_digest_matches: bool,
    /// Faults of the Rust turn (`module:hook:message`), in order.
    pub faults: Vec<String>,
}

impl TurnComparison {
    /// True when every signal matched and nothing faulted.
    #[must_use]
    pub fn is_perfect(&self) -> bool {
        self.reply_matches
            && self.module_matches
            && self.candidates_match
            && self.memory_keys_match
            && self.memory_digest_matches
            && self.faults.is_empty()
    }

    /// True when the reply or the winning module disagree (the "diff" of the report).
    #[must_use]
    pub fn differs(&self) -> bool {
        !self.reply_matches || !self.module_matches
    }
}

/// Run one corpus turn through the Rust bot and compare it with the record.
#[must_use]
pub fn compare_turn(
    index: usize,
    expected: &GoldenTurn,
    session: &mut Session,
    runtime: &crate::Runtime,
) -> TurnComparison {
    let turn = Turn::new(expected.input.as_str());
    let report = runtime.turn(&turn, session);
    let actual_reply = report.normalised_reply();
    let actual_candidates: Vec<(String, Priority)> = report
        .candidates
        .iter()
        .map(|candidate| (candidate.module.as_str().to_string(), candidate.priority))
        .collect();
    let (candidates_match, candidates_diff) = if actual_candidates == expected.candidates {
        (true, None)
    } else {
        (
            false,
            Some(describe_candidates(
                &expected.candidates,
                &actual_candidates,
            )),
        )
    };
    let expected_keys: Vec<&str> = expected.memory_keys.iter().map(String::as_str).collect();
    let faults: Vec<String> = report
        .faults
        .faults()
        .iter()
        .map(|fault| format!("{}:{}:{}", fault.module, fault.hook, fault.message))
        .collect();
    TurnComparison {
        index,
        session: expected.session.clone(),
        turn: expected.turn,
        block: expected.block(),
        input: truncate_for_report(&expected.input, 80),
        expected_module: expected.module.clone(),
        actual_module: report.module.as_ref().map(|name| name.as_str().to_string()),
        expected_candidates: expected
            .candidates
            .iter()
            .map(|(name, _)| name.clone())
            .collect(),
        actual_candidates: actual_candidates
            .into_iter()
            .map(|(name, _)| name)
            .collect(),
        reply_matches: actual_reply == expected.reply,
        module_matches: report.module.as_ref().map(|name| name.as_str().to_string())
            == expected.module,
        candidates_match,
        candidates_diff,
        memory_keys_match: session.memory_keys() == expected_keys,
        memory_digest_matches: session.memory_digest().hex() == expected.memory_digest,
        expected_reply: expected.reply.clone(),
        actual_reply,
        faults,
    }
}

/// A short, deterministic description of the first candidate-list difference.
fn describe_candidates(expected: &[(String, Priority)], actual: &[(String, Priority)]) -> String {
    let length = expected.len().max(actual.len());
    for index in 0..length {
        let left = expected.get(index);
        let right = actual.get(index);
        let left_key = left.map(|(name, priority)| (name.as_str(), *priority));
        let right_key = right.map(|(name, priority)| (name.as_str(), *priority));
        if left_key != right_key {
            let render = |pair: Option<&(String, Priority)>| match pair {
                Some((name, priority)) => format!("{name}@{priority}"),
                None => String::from("-"),
            };
            return format!(
                "index {index}: expected {}, got {}",
                render(left),
                render(right)
            );
        }
    }
    String::from("no difference")
}

/// One comparison per corpus turn, in file order. Sessions are replayed in order so multi-turn
/// memory effects are reproduced (a new [`Session`] per session id).
#[must_use]
pub fn compare_corpus(corpus: &Corpus) -> Vec<TurnComparison> {
    let runtime = crate::Runtime::new();
    let mut session = Session::new();
    let mut current: Option<&str> = None;
    let mut comparisons = Vec::with_capacity(corpus.turns.len());
    for (index, expected) in corpus.turns.iter().enumerate() {
        if current != Some(expected.session.as_str()) {
            session = Session::new();
            current = Some(expected.session.as_str());
        }
        comparisons.push(compare_turn(index, expected, &mut session, &runtime));
    }
    comparisons
}

// ---------------------------------------------------------------------------- the report

/// How many example diffs a group keeps.
pub const MAX_GROUPED_DIFFS: usize = 3;

/// Agreement counters for one group.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Agreement {
    /// Turns in the group.
    pub turns: usize,
    /// Turns whose reply matched.
    pub replies: usize,
    /// Turns whose winning module matched.
    pub modules: usize,
    /// Turns whose candidate list matched.
    pub candidates: usize,
    /// Turns whose memory keys matched.
    pub memory_keys: usize,
    /// Turns whose memory digest matched.
    pub memory_digest: usize,
    /// Turns with at least one fault.
    pub faulted: usize,
}

impl Agreement {
    /// Add one comparison.
    pub fn add(&mut self, comparison: &TurnComparison) {
        self.turns += 1;
        self.replies += usize::from(comparison.reply_matches);
        self.modules += usize::from(comparison.module_matches);
        self.candidates += usize::from(comparison.candidates_match);
        self.memory_keys += usize::from(comparison.memory_keys_match);
        self.memory_digest += usize::from(comparison.memory_digest_matches);
        self.faulted += usize::from(!comparison.faults.is_empty());
    }

    /// Percentage of turns whose reply matched (0.0 for an empty group).
    #[must_use]
    pub fn reply_percent(&self) -> f64 {
        percent(self.replies, self.turns)
    }

    /// Percentage of turns whose winning module matched.
    #[must_use]
    pub fn module_percent(&self) -> f64 {
        percent(self.modules, self.turns)
    }

    /// Percentage of turns whose candidate list matched.
    #[must_use]
    pub fn candidate_percent(&self) -> f64 {
        percent(self.candidates, self.turns)
    }

    /// Percentage of turns whose memory keys matched.
    #[must_use]
    pub fn memory_keys_percent(&self) -> f64 {
        percent(self.memory_keys, self.turns)
    }

    /// Percentage of turns whose memory digest matched.
    #[must_use]
    pub fn digest_percent(&self) -> f64 {
        percent(self.memory_digest, self.turns)
    }
}

fn percent(numerator: usize, denominator: usize) -> f64 {
    if denominator == 0 {
        0.0
    } else {
        (numerator as f64) * 100.0 / (denominator as f64)
    }
}

/// One example of a difference, for the report's grouped diffs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffExample {
    /// Session id.
    pub session: String,
    /// Turn index inside the session.
    pub turn: u32,
    /// The input, truncated.
    pub input: String,
    /// Expected module.
    pub expected_module: Option<String>,
    /// Actual module.
    pub actual_module: Option<String>,
    /// Expected reply, truncated.
    pub expected_reply: String,
    /// Actual reply, truncated.
    pub actual_reply: String,
}

/// One named group of the report.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Group {
    /// Group name (block name, module name, …).
    pub name: String,
    /// Agreement counters.
    pub agreement: Agreement,
    /// Total number of differing turns in this group.
    pub diffs: usize,
    /// Up to [`MAX_GROUPED_DIFFS`] examples of differences.
    pub examples: Vec<DiffExample>,
}

impl Group {
    /// A group that only counts (blocks, candidate modules).
    fn counted(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ..Self::default()
        }
    }

    /// A group that counts and samples diffs (expected modules).
    fn sampling(name: impl Into<String>) -> Self {
        Self::counted(name)
    }

    fn observe(&mut self, comparison: &TurnComparison) {
        self.agreement.add(comparison);
        if comparison.differs() {
            self.diffs += 1;
            if self.examples.len() < MAX_GROUPED_DIFFS {
                self.examples.push(diff_example(comparison));
            }
        }
    }

    fn observe_no_examples(&mut self, comparison: &TurnComparison) {
        self.agreement.add(comparison);
        if comparison.differs() {
            self.diffs += 1;
        }
    }
}

/// The differential report.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GoldenReport {
    /// Overall agreement.
    pub overall: Agreement,
    /// Agreement per block (`por diretório`), in block order.
    pub blocks: Vec<Group>,
    /// Agreement per expected winning module, alphabetically, with examples.
    pub by_expected_module: Vec<Group>,
    /// Turns each module appears in as a candidate (either side), alphabetically.
    pub by_candidate_module: Vec<Group>,
    /// Faults across the run, grouped by `module:hook`, sorted.
    pub faults: Vec<(String, usize)>,
    /// Number of comparisons.
    pub turns: usize,
}

impl GoldenReport {
    /// Build the report from per-turn comparisons in one pass.
    #[must_use]
    pub fn build(comparisons: &[TurnComparison]) -> Self {
        let mut overall = Agreement::default();
        let mut blocks: BTreeMap<&'static str, Group> = BTreeMap::new();
        let mut expected_modules: BTreeMap<String, Group> = BTreeMap::new();
        let mut candidate_modules: BTreeMap<String, Group> = BTreeMap::new();
        let mut faults: BTreeMap<String, usize> = BTreeMap::new();

        for comparison in comparisons {
            overall.add(comparison);
            blocks
                .entry(comparison.block.as_str())
                .or_insert_with(|| Group::counted(comparison.block.as_str()))
                .observe_no_examples(comparison);
            let expected_name = comparison
                .expected_module
                .clone()
                .unwrap_or_else(|| String::from("<none>"));
            expected_modules
                .entry(expected_name.clone())
                .or_insert_with(|| Group::sampling(expected_name))
                .observe(comparison);

            // "Turns it appears in" is about the *candidate* lists only: the winning module is
            // already covered by `by_expected_module`, and folding it in here would count a
            // module that proposed nothing as having proposed on every turn it won.
            let mut mentioned: Vec<&str> = Vec::new();
            for module in &comparison.expected_candidates {
                mentioned.push(module.as_str());
            }
            for module in &comparison.actual_candidates {
                mentioned.push(module.as_str());
            }
            mentioned.sort_unstable();
            mentioned.dedup();
            for module in mentioned {
                candidate_modules
                    .entry(module.to_string())
                    .or_insert_with(|| Group::counted(module))
                    .observe_no_examples(comparison);
            }

            for fault in &comparison.faults {
                let key = fault.split(':').take(2).collect::<Vec<_>>().join(":");
                *faults.entry(key).or_insert(0) += 1;
            }
        }

        Self {
            overall,
            blocks: Block::ALL
                .iter()
                .filter_map(|block| blocks.remove(block.as_str()))
                .collect(),
            by_expected_module: expected_modules.into_values().collect(),
            by_candidate_module: candidate_modules.into_values().collect(),
            faults: faults.into_iter().collect(),
            turns: comparisons.len(),
        }
    }

    /// The report as human-readable text (the format the CI log shows).
    #[must_use]
    pub fn render_text(&self) -> String {
        use std::fmt::Write as _;
        let mut out = String::new();
        let _ = writeln!(out, "differential report — {} turns", self.turns);
        let _ = writeln!(
            out,
            "overall: reply {:.2}% | module {:.2}% | candidates {:.2}% | memory keys {:.2}% | memory digest {:.2}% | faulted {}",
            self.overall.reply_percent(),
            self.overall.module_percent(),
            self.overall.candidate_percent(),
            self.overall.memory_keys_percent(),
            self.overall.digest_percent(),
            self.overall.faulted
        );
        let _ = writeln!(out, "\nby block (directory):");
        for group in &self.blocks {
            let _ = writeln!(
                out,
                "  {:<17}{:>5} turns | reply {:>6.2}% | module {:>6.2}% | candidates {:>6.2}% | memory keys {:>6.2}% | diffs {}",
                group.name,
                group.agreement.turns,
                group.agreement.reply_percent(),
                group.agreement.module_percent(),
                group.agreement.candidate_percent(),
                group.agreement.memory_keys_percent(),
                group.diffs
            );
        }
        let _ = writeln!(out, "\nby expected module:");
        for group in &self.by_expected_module {
            let _ = writeln!(
                out,
                "  {:<24}{:>5} turns | reply {:>6.2}% | module {:>6.2}% | candidates {:>6.2}%",
                group.name,
                group.agreement.turns,
                group.agreement.reply_percent(),
                group.agreement.module_percent(),
                group.agreement.candidate_percent()
            );
            for example in &group.examples {
                let _ = writeln!(
                    out,
                    "      e.g. {}/{} {:?}: module {} -> {}, reply {:?} -> {:?}",
                    example.session,
                    example.turn,
                    example.input,
                    example.expected_module.as_deref().unwrap_or("none"),
                    example.actual_module.as_deref().unwrap_or("none"),
                    example.expected_reply,
                    example.actual_reply
                );
            }
        }
        let _ = writeln!(out, "\nby candidate module (turns it appears in):");
        for group in &self.by_candidate_module {
            let _ = writeln!(
                out,
                "  {:<24}{:>5} turns | module {:>6.2}% | candidates {:>6.2}%",
                group.name,
                group.agreement.turns,
                group.agreement.module_percent(),
                group.agreement.candidate_percent()
            );
        }
        if self.faults.is_empty() {
            let _ = writeln!(out, "\nfaults: none");
        } else {
            let _ = writeln!(out, "\nfaults:");
            for (key, count) in &self.faults {
                let _ = writeln!(out, "  {key} x{count}");
            }
        }
        out
    }

    /// The report as a JSON document (stable key order; the format CI uploads).
    #[must_use]
    pub fn to_json(&self) -> Json {
        let agreement_json = |agreement: &Agreement| {
            let count = |value: usize| Json::Int(i64::try_from(value).unwrap_or(i64::MAX));
            json::object([
                ("turns", count(agreement.turns)),
                ("replies", count(agreement.replies)),
                ("modules", count(agreement.modules)),
                ("candidates", count(agreement.candidates)),
                ("memory_keys", count(agreement.memory_keys)),
                ("memory_digest", count(agreement.memory_digest)),
                ("faulted", count(agreement.faulted)),
            ])
        };
        let groups_json = |groups: &[Group]| {
            Json::Array(
                groups
                    .iter()
                    .map(|group| {
                        let examples = Json::Array(
                            group
                                .examples
                                .iter()
                                .map(|example| {
                                    json::object([
                                        ("session", json::string(example.session.clone())),
                                        ("turn", Json::Int(i64::from(example.turn))),
                                        ("input", json::string(example.input.clone())),
                                        (
                                            "expected_module",
                                            example
                                                .expected_module
                                                .as_ref()
                                                .map_or(Json::Null, |name| {
                                                    json::string(name.clone())
                                                }),
                                        ),
                                        (
                                            "actual_module",
                                            example
                                                .actual_module
                                                .as_ref()
                                                .map_or(Json::Null, |name| {
                                                    json::string(name.clone())
                                                }),
                                        ),
                                        (
                                            "expected_reply",
                                            json::string(example.expected_reply.clone()),
                                        ),
                                        (
                                            "actual_reply",
                                            json::string(example.actual_reply.clone()),
                                        ),
                                    ])
                                })
                                .collect(),
                        );
                        json::object([
                            ("name", json::string(group.name.clone())),
                            ("agreement", agreement_json(&group.agreement)),
                            (
                                "diffs",
                                Json::Int(i64::try_from(group.diffs).unwrap_or(i64::MAX)),
                            ),
                            ("examples", examples),
                        ])
                    })
                    .collect(),
            )
        };
        json::object([
            (
                "turns",
                Json::Int(i64::try_from(self.turns).unwrap_or(i64::MAX)),
            ),
            ("overall", agreement_json(&self.overall)),
            ("blocks", groups_json(&self.blocks)),
            ("by_expected_module", groups_json(&self.by_expected_module)),
            (
                "by_candidate_module",
                groups_json(&self.by_candidate_module),
            ),
            (
                "faults",
                Json::Object(
                    self.faults
                        .iter()
                        .map(|(key, count)| {
                            (
                                key.clone(),
                                Json::Int(i64::try_from(*count).unwrap_or(i64::MAX)),
                            )
                        })
                        .collect(),
                ),
            ),
        ])
    }
}

fn diff_example(comparison: &TurnComparison) -> DiffExample {
    DiffExample {
        session: comparison.session.clone(),
        turn: comparison.turn,
        input: comparison.input.clone(),
        expected_module: comparison.expected_module.clone(),
        actual_module: comparison.actual_module.clone(),
        expected_reply: truncate_for_report(&comparison.expected_reply, 80),
        actual_reply: truncate_for_report(&comparison.actual_reply, 80),
    }
}

/// Collapse whitespace like Python and truncate to `max_chars` with an ellipsis.
fn truncate_for_report(text: &str, max_chars: usize) -> String {
    let collapsed = pyutil::collapse_spaces(text).into_owned();
    let truncated = limits::truncate_chars(&collapsed, max_chars);
    if truncated.len() == collapsed.len() {
        truncated.to_string()
    } else {
        format!("{truncated}…")
    }
}

/// Load `corpus.jsonl` (and `manifest.json`, when present) from a directory.
pub fn load_dir(dir: &std::path::Path) -> Result<Corpus> {
    let bytes =
        std::fs::read(dir.join("corpus.jsonl")).map_err(|error| Error::io("read corpus", error))?;
    let corpus = Corpus::parse(&bytes)?;
    let manifest_path = dir.join("manifest.json");
    match std::fs::read(&manifest_path) {
        Ok(manifest) => {
            let manifest = Corpus::parse_manifest(&manifest)?;
            corpus.with_manifest_and_bytes(manifest, Some(&bytes))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(corpus),
        Err(error) => Err(Error::io("read manifest", error)),
    }
}

/// The corpus shipped with this repository (`tests/golden`), parsed and manifest-checked.
///
/// Used by the integration tests and the `differential` binary; not a runtime dependency.
pub fn load_repository_corpus() -> Result<Corpus> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden");
    load_dir(&dir)
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::panic
    )]

    use super::*;

    fn real_corpus() -> Corpus {
        load_repository_corpus().expect("the shipped corpus must load and validate")
    }

    // ------------------------------------------------------------------ corpus invariants

    #[test]
    fn the_real_corpus_has_the_shape_the_manifest_declares() {
        let corpus = real_corpus();
        assert_eq!(corpus.len(), 3_062);
        assert_eq!(corpus.session_count(), 2_850);
        let manifest = corpus.manifest.as_ref().unwrap();
        assert_eq!(manifest.version, 1);
        assert_eq!(manifest.turns, 3_062);
        assert_eq!(manifest.sessions, 2_850);
        assert_eq!(manifest.pythonhashseed, "0");
        assert_eq!(manifest.corpus_seed, 20_261_121);
        assert_eq!(manifest.corpus_sha256.len(), 64);
        assert_eq!(manifest.created_by, "tools/recon/gen_corpus.py");
    }

    #[test]
    fn sessions_keep_their_order_and_turn_indices() {
        let corpus = real_corpus();
        let sessions = corpus.sessions();
        assert_eq!(sessions.len(), 2_850);
        let mut previous: Option<&str> = None;
        for (id, turns) in sessions {
            if let Some(previous) = previous {
                assert!(previous < id, "sessions must ascend: {previous} < {id}");
            }
            previous = Some(id);
            for (index, turn) in turns.iter().enumerate() {
                assert_eq!(turn.turn as usize, index);
                assert_eq!(turn.session, id);
            }
        }
    }

    #[test]
    fn every_turn_has_a_consistent_candidate_list_and_memory_shape() {
        let corpus = real_corpus();
        let mut seen_priorities = std::collections::BTreeSet::new();
        for turn in &corpus.turns {
            let mut sorted = turn.candidates.clone();
            sorted.sort_by(|left, right| {
                (-left.1.as_f64(), left.0.as_str())
                    .partial_cmp(&(-right.1.as_f64(), right.0.as_str()))
                    .unwrap()
            });
            assert_eq!(sorted, turn.candidates, "{}", turn.session);
            for (name, priority) in &turn.candidates {
                assert!(!name.is_empty());
                seen_priorities.insert(priority.repr());
            }
            if let Some(module) = &turn.module {
                assert!(
                    turn.candidates.iter().any(|(name, _)| name == module),
                    "the winning module must be among the candidates"
                );
            }
            assert!(!turn.reply.is_empty());
            assert_eq!(turn.reply, pyutil::collapse_spaces(&turn.reply));
            assert_eq!(turn.memory_digest.len(), 40);
            assert!(turn
                .memory_digest
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit()));
            let mut keys = turn.memory_keys.clone();
            keys.sort();
            keys.dedup();
            assert_eq!(keys.len(), turn.memory_keys.len(), "keys unique");
            assert_eq!(keys, turn.memory_keys, "keys sorted");
        }
        assert!(seen_priorities.contains("300"));
        assert!(seen_priorities.contains("-1000"));
        assert!(
            seen_priorities.contains("-999.5"),
            "the float priority is preserved"
        );
        assert!(seen_priorities.contains("-999"));
    }

    #[test]
    fn blocks_partition_the_corpus_and_match_the_generator() {
        let corpus = real_corpus();
        let counts = corpus.block_counts();
        assert_eq!(counts.values().sum::<usize>(), corpus.len());
        assert_eq!(counts.get("curated").copied(), Some(512));
        assert_eq!(counts.get("fuzz").copied(), Some(704));
        assert_eq!(counts.get("knowledge_probes").copied(), Some(1_846));
        assert_eq!(corpus.turns.first().unwrap().block(), Block::Curated);
        assert_eq!(corpus.turns.last().unwrap().block(), Block::Knowledge);
        // The last curated turn is a repetition session; the first fuzz one is a mutation.
        let boundary = corpus
            .turns
            .iter()
            .position(|turn| turn.block() == Block::Fuzz)
            .unwrap();
        assert_eq!(boundary, 512, "512 curated turns precede the fuzz block");
        let turn = corpus.turns.get(boundary).unwrap();
        assert_eq!(turn.session, "s0300");
    }

    #[test]
    fn blocks_are_named_and_mapped_from_session_numbers() {
        let names: Vec<&str> = Block::ALL.iter().map(|block| block.as_str()).collect();
        assert_eq!(names, vec!["curated", "fuzz", "knowledge_probes"]);
        assert_eq!(Block::of_session(0), Block::Curated);
        assert_eq!(Block::of_session(299), Block::Curated);
        assert_eq!(Block::of_session(300), Block::Fuzz);
        assert_eq!(Block::of_session(1_003), Block::Fuzz);
        assert_eq!(Block::of_session(1_004), Block::Knowledge);
        assert_eq!(Block::of_session(2_849), Block::Knowledge);
    }

    #[test]
    fn non_canonical_session_ids_fall_back_to_curated_and_have_no_number() {
        let mut turn = real_corpus().turns.first().cloned().unwrap();
        turn.session = String::from("weird");
        assert_eq!(turn.session_number(), None);
        assert_eq!(turn.block(), Block::Curated);
        turn.session = String::from("s12");
        assert_eq!(turn.session_number(), None);
    }

    // ------------------------------------------------------------------ schema validation

    fn line(session: &str, turn: u32) -> String {
        format!(
            r#"{{"session":"{session}","turn":{turn},"input":"hi","reply":"x","module":null,"candidates":[],"memory_keys":[],"memory_digest":"0000000000000000000000000000000000000000"}}"#
        )
    }

    #[test]
    fn a_minimal_valid_corpus_parses() {
        let corpus = Corpus::parse(line("s0000", 0).as_bytes()).unwrap();
        assert_eq!(corpus.len(), 1);
        assert_eq!(corpus.session_count(), 1);
        assert!(corpus.manifest.is_none());
        let corpus =
            Corpus::parse(format!("{}\n{}", line("s0000", 0), line("s0001", 0)).as_bytes())
                .unwrap();
        assert_eq!(corpus.len(), 2);
    }

    #[test]
    fn parsing_rejects_missing_and_wrongly_typed_fields() {
        let cases: [(&[u8], &str); 6] = [
            (
                br#"{"session":"s0000","turn":0,"input":"hi","reply":"x","module":null,"candidates":[],"memory_keys":[]}"#,
                "missing field",
            ),
            (
                br#"{"session":"s0000","turn":0,"input":"hi","reply":"x","module":5,"candidates":[],"memory_keys":[],"memory_digest":"0000000000000000000000000000000000000000"}"#,
                "module is neither string nor null",
            ),
            (
                br#"{"session":"s0000","turn":"0","input":"hi","reply":"x","module":null,"candidates":[],"memory_keys":[],"memory_digest":"0000000000000000000000000000000000000000"}"#,
                "turn is not an int",
            ),
            (
                br#"{"session":"s0000","turn":0,"input":5,"reply":"x","module":null,"candidates":[],"memory_keys":[],"memory_digest":"0000000000000000000000000000000000000000"}"#,
                "input is not a string",
            ),
            (
                br#"{"session":"s0000","turn":0,"input":"hi","reply":"x","module":null,"candidates":[],"memory_keys":[],"memory_digest":"xyz"}"#,
                "memory_digest is not a sha1 hex digest",
            ),
            (
                br#"{"session":"s0000","turn":-1,"input":"hi","reply":"x","module":null,"candidates":[],"memory_keys":[],"memory_digest":"0000000000000000000000000000000000000000"}"#,
                "turn index out of range",
            ),
        ];
        for (text, expected) in cases {
            match Corpus::parse(text) {
                Err(Error::Corpus { line: 1, message }) => assert_eq!(message, expected),
                other => panic!("{expected}: {other:?}"),
            }
        }
    }

    #[test]
    fn parsing_rejects_structural_violations() {
        let cases: [(String, &str); 4] = [
            (
                format!("{}\n{}", line("s0000", 0), line("s0000", 2)),
                "turn indices are not contiguous inside a session",
            ),
            (
                format!("{}\n{}", line("s0001", 0), line("s0000", 0)),
                "sessions are not in ascending id order",
            ),
            (
                format!("{}\n{}", line("s0000", 0), line("s0001", 1)),
                "a new session must start at turn 0",
            ),
            (line("s0000", 3), "a new session must start at turn 0"),
        ];
        for (text, expected) in cases {
            match Corpus::parse(text.as_bytes()) {
                Err(Error::Corpus { message, .. }) => assert_eq!(message, expected),
                other => panic!("{expected}: {other:?}"),
            }
        }
        assert!(matches!(Corpus::parse(b""), Err(Error::Corpus { .. })));
        assert!(matches!(Corpus::parse(b"\n\n"), Err(Error::Corpus { .. })));
    }

    #[test]
    fn parsing_rejects_invalid_utf8() {
        assert!(matches!(
            Corpus::parse(&[b'{', 0xFF, b'}']),
            Err(Error::InvalidUtf8 { .. })
        ));
    }

    #[test]
    fn candidate_priorities_keep_their_kind() {
        let text = br#"{"session":"s0000","turn":0,"input":"hi","reply":"x","module":"a","candidates":[{"module":"a","priority":300},{"module":"b","priority":-999.5},{"module":"c","priority":-1000.0}],"memory_keys":[],"memory_digest":"0000000000000000000000000000000000000000"}"#;
        let corpus = Corpus::parse(text).unwrap();
        let turn = corpus.turns.first().unwrap();
        assert_eq!(turn.candidates[0].1, Priority::Int(300));
        assert_eq!(turn.candidates[1].1, Priority::Float(-999.5));
        assert_eq!(turn.candidates[2].1, Priority::Float(-1000.0));
        assert_eq!(turn.candidates[2].1.repr(), "-1000.0");
        let bad = br#"{"session":"s0000","turn":0,"input":"hi","reply":"x","module":null,"candidates":[{"module":"a","priority":"x"}],"memory_keys":[],"memory_digest":"0000000000000000000000000000000000000000"}"#;
        assert!(Corpus::parse(bad).is_err());
    }

    // ------------------------------------------------------------------ manifest

    #[test]
    fn the_manifest_parses_and_validates_against_the_turns() {
        let manifest_json = br#"{"version":1,"created_by":"tools/recon/gen_corpus.py","corpus_seed":20261121,"pythonhashseed":"0","python":"3.11.2","sessions":1,"turns":1,"corpus_sha256":"abc"}"#;
        let manifest = Corpus::parse_manifest(manifest_json).unwrap();
        assert_eq!(manifest.python, "3.11.2");
        let corpus = Corpus::parse(line("s0000", 0).as_bytes()).unwrap();
        let corpus = corpus.with_manifest(manifest.clone()).unwrap();
        assert_eq!(corpus.manifest.as_ref().map(|m| m.turns), Some(1));
        assert!(matches!(
            corpus.with_manifest(Manifest {
                turns: 99,
                ..manifest
            }),
            Err(Error::Manifest { .. })
        ));
        let wrong_sessions = Corpus::parse_manifest(manifest_json).unwrap();
        let corpus = Corpus::parse(line("s0000", 0).as_bytes()).unwrap();
        assert!(matches!(
            corpus.with_manifest(Manifest {
                sessions: 7,
                ..wrong_sessions
            }),
            Err(Error::Manifest { .. })
        ));
    }

    #[test]
    fn a_manifest_missing_any_required_field_is_rejected() {
        let full = r#"{"version":1,"created_by":"x","corpus_seed":1,"pythonhashseed":"0","python":"3","sessions":1,"turns":1,"corpus_sha256":"a"}"#;
        assert!(Corpus::parse_manifest(full.as_bytes()).is_ok());
        for key in [
            "version",
            "created_by",
            "corpus_seed",
            "sessions",
            "turns",
            "corpus_sha256",
        ] {
            let mut value = json::parse_str(full).unwrap();
            if let Json::Object(fields) = &mut value {
                fields.retain(|(name, _)| name != key);
            }
            let text = json::to_string(&value).expect("finite test data");
            assert!(
                matches!(
                    Corpus::parse_manifest(text.as_bytes()),
                    Err(Error::Manifest { .. })
                ),
                "missing {key} must be rejected"
            );
        }
        let negative = r#"{"version":1,"created_by":"x","corpus_seed":1,"sessions":-1,"turns":1,"corpus_sha256":"a"}"#;
        assert!(matches!(
            Corpus::parse_manifest(negative.as_bytes()),
            Err(Error::Manifest { .. })
        ));
    }

    #[test]
    fn a_manifest_whose_digest_disagrees_with_the_bytes_is_rejected() {
        let directory = temp_dir("tampered-digest");
        std::fs::create_dir_all(&directory).unwrap();
        let corpus_bytes = format!("{}\n", line("s0000", 0));
        std::fs::write(directory.join("corpus.jsonl"), &corpus_bytes).unwrap();
        let good = corpus_bytes.trim_end().as_bytes();
        let real_digest = crate::hash::sha256(&[corpus_bytes.as_bytes()]).hex();
        // A manifest with the right counts but the digest of *different* bytes must be refused:
        // counts alone cannot tell an edited corpus from the captured one.
        let manifest = format!(
            r#"{{"version":1,"created_by":"x","corpus_seed":1,"sessions":1,"turns":1,"corpus_sha256":"{}"}}"#,
            crate::hash::sha256(&[good]).hex()
        );
        assert_ne!(crate::hash::sha256(&[good]).hex(), real_digest);
        std::fs::write(directory.join("manifest.json"), manifest).unwrap();
        assert!(
            matches!(
                load_dir(&directory),
                Err(Error::Manifest { message }) if message.contains("sha256")
            ),
            "a digest mismatch must be a manifest error, not silently accepted"
        );

        // The honest manifest still loads, and the counts are still enforced.
        let honest = format!(
            r#"{{"version":1,"created_by":"x","corpus_seed":1,"sessions":1,"turns":1,"corpus_sha256":"{real_digest}"}}"#
        );
        std::fs::write(directory.join("manifest.json"), honest).unwrap();
        let corpus = load_dir(&directory).expect("the honest manifest loads");
        assert!(corpus.manifest.is_some());
    }

    #[test]
    fn candidate_groups_ignore_the_winning_module() {
        // A comparison where `chat` wins but only `greeting` proposed: `chat` must not appear
        // in the candidate table (it proposed nothing), and `greeting` must be counted once
        // even though it is both an expected and an actual candidate.
        let comparison = TurnComparison {
            index: 0,
            session: "s0000".to_string(),
            turn: 0,
            block: Block::Curated,
            input: "hello".to_string(),
            expected_module: Some("chat".to_string()),
            actual_module: Some("chat".to_string()),
            expected_candidates: vec!["greeting".to_string(), "chat".to_string()],
            actual_candidates: vec!["greeting".to_string(), "chat".to_string()],
            reply_matches: true,
            module_matches: true,
            candidates_match: true,
            candidates_diff: None,
            memory_keys_match: true,
            memory_digest_matches: true,
            expected_reply: "hi".to_string(),
            actual_reply: "hi".to_string(),
            faults: Vec::new(),
        };
        let report = GoldenReport::build(&[comparison]);
        let names: Vec<&str> = report
            .by_candidate_module
            .iter()
            .map(|group| group.name.as_str())
            .collect();
        assert_eq!(
            names,
            vec!["chat", "greeting"],
            "deduplicated, alphabetical"
        );
        assert_eq!(
            report
                .by_candidate_module
                .iter()
                .find(|group| group.name == "greeting")
                .map(|group| group.agreement.turns),
            Some(1),
            "a module in both candidate lists counts once"
        );

        // A winner that proposed nothing is absent; only `chat` (present in both lists here)
        // and `greeting` are candidates, and a winning-only module never enters the table.
        let mut only_one = report.by_candidate_module.clone();
        only_one.retain(|group| group.name == "chat");
        assert_eq!(only_one.len(), 1);
    }

    #[test]
    fn the_manifest_is_optional_for_a_directory() {
        let directory = temp_dir("no-manifest");
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(
            directory.join("corpus.jsonl"),
            format!("{}\n", line("s0000", 0)),
        )
        .unwrap();
        let corpus = load_dir(&directory).unwrap();
        assert!(corpus.manifest.is_none());
        std::fs::remove_dir_all(&directory).ok();
    }

    #[test]
    fn a_directory_without_a_corpus_is_an_io_error() {
        let directory = temp_dir("missing-corpus");
        std::fs::create_dir_all(&directory).unwrap();
        assert!(matches!(load_dir(&directory), Err(Error::Io { .. })));
        std::fs::remove_dir_all(&directory).ok();
    }

    fn temp_dir(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("pybot-golden-{name}-{}", std::process::id()))
    }

    // ------------------------------------------------------------------ comparison

    /// An expectation that matches what the Fase 1 skeleton does for `hello`.
    ///
    /// The reply and the name of the winning module are already the ones Python gives; the
    /// candidate list is not, because Python's `chat` module also proposes its greeting line
    /// (priority 300) and the skeleton's `chat` only runs after the turn. The memory digest is
    /// left empty on purpose: `understood` is the one part of memory the skeleton does not
    /// reproduce, so no digest can match yet.
    fn hello_expectation() -> GoldenTurn {
        GoldenTurn {
            session: String::from("s0000"),
            turn: 0,
            input: String::from("hello"),
            reply: String::from("Hi there — what do you need?"),
            module: Some(String::from("greeting")),
            candidates: vec![(String::from("greeting"), Priority::Int(-1_000))],
            memory_keys: vec![
                String::from("frequency_answer"),
                String::from("frequency_reply"),
                String::from("intent_wants"),
                String::from("said_kind"),
                String::from("turns"),
                String::from("understood"),
                String::from("user"),
            ],
            memory_digest: String::new(),
        }
    }

    #[test]
    fn a_matching_turn_is_recognised_and_its_candidates_agree() {
        let expected = hello_expectation();
        let mut session = Session::new();
        let comparison = compare_turn(0, &expected, &mut session, &crate::Runtime::new());
        assert!(comparison.reply_matches);
        assert!(comparison.module_matches);
        assert!(comparison.candidates_match);
        assert!(comparison.memory_keys_match, "same seven keys as Python");
        assert!(
            !comparison.memory_digest_matches,
            "the skeleton memory differs"
        );
        assert!(comparison.faults.is_empty());
        assert_eq!(comparison.block, Block::Curated);
        assert_eq!(comparison.actual_candidates, vec!["greeting"]);
        assert_eq!(comparison.expected_module.as_deref(), Some("greeting"));
        assert_eq!(comparison.actual_module.as_deref(), Some("greeting"));
        assert!(!comparison.is_perfect());
        assert!(
            !comparison.differs(),
            "nothing in the measured signals differs"
        );
    }

    #[test]
    fn a_wrong_expectation_reports_the_first_candidate_diff() {
        let mut expected = real_corpus().turns.first().cloned().unwrap();
        expected.input = String::from("hello");
        expected.reply = String::from("Something else");
        expected.module = Some(String::from("chat"));
        expected.candidates = vec![
            (String::from("chat"), Priority::Int(300)),
            (String::from("greeting"), Priority::Int(-1000)),
        ];
        let mut session = Session::new();
        let comparison = compare_turn(0, &expected, &mut session, &crate::Runtime::new());
        assert!(!comparison.reply_matches);
        assert!(!comparison.module_matches);
        assert!(!comparison.candidates_match);
        assert!(comparison.differs());
        assert_eq!(
            comparison.candidates_diff.as_deref(),
            Some("index 0: expected chat@300, got greeting@-1000")
        );
    }

    #[test]
    fn longer_and_shorter_candidate_lists_are_described() {
        let expected = vec![(String::from("a"), Priority::Int(1))];
        assert_eq!(
            describe_candidates(&expected, &[]),
            "index 0: expected a@1, got -"
        );
        let actual = vec![(String::from("b"), Priority::Int(2))];
        assert_eq!(
            describe_candidates(&[], &actual),
            "index 0: expected -, got b@2"
        );
        assert_eq!(describe_candidates(&expected, &expected), "no difference");
        let expected = vec![
            (String::from("a"), Priority::Int(1)),
            (String::from("b"), Priority::Int(0)),
        ];
        let actual = vec![(String::from("a"), Priority::Int(1))];
        assert_eq!(
            describe_candidates(&expected, &actual),
            "index 1: expected b@0, got -"
        );
    }

    #[test]
    fn comparison_replays_sessions_so_memory_is_reproduced_per_session() {
        let corpus = real_corpus();
        let comparisons = compare_corpus(&corpus);
        assert_eq!(comparisons.len(), corpus.len());
        // The first session is `hello`: a fresh session has no memory keys at all before the
        // turn, and the skeleton's key set differs from Python's — the harness says so.
        let first = comparisons.first().unwrap();
        assert_eq!(first.session, "s0000");
        assert_eq!(first.index, 0);
        assert!(comparisons
            .iter()
            .all(|comparison| comparison.faults.is_empty()));
    }

    // ------------------------------------------------------------------ report

    #[test]
    fn the_report_groups_everything_and_is_valid_json() {
        let corpus = real_corpus();
        let report = GoldenReport::build(&compare_corpus(&corpus));
        assert_eq!(report.turns, corpus.len());
        assert_eq!(report.overall.turns, corpus.len());
        assert_eq!(
            report
                .blocks
                .iter()
                .map(|group| group.name.as_str())
                .collect::<Vec<_>>(),
            vec!["curated", "fuzz", "knowledge_probes"],
        );
        assert_eq!(
            report
                .blocks
                .iter()
                .map(|group| group.agreement.turns)
                .sum::<usize>(),
            corpus.len()
        );
        assert!(report
            .by_expected_module
            .iter()
            .any(|group| group.name == "answers"));
        assert!(report
            .by_expected_module
            .iter()
            .any(|group| group.name == "greeting"));
        assert!(report
            .by_candidate_module
            .iter()
            .any(|group| group.name == "greeting"));
        assert!(report.faults.is_empty());

        let text = report.render_text();
        assert!(text.contains("differential report — 3062 turns"));
        assert!(text.contains("by block (directory)"));
        assert!(text.contains("faults: none"));

        let rendered = json::to_string(&report.to_json()).unwrap();
        let parsed = json::parse_str(&rendered).unwrap();
        assert_eq!(parsed.get("turns").and_then(Json::as_i64), Some(3_062));
        assert_eq!(
            parsed
                .get("overall")
                .and_then(|overall| overall.get("turns"))
                .and_then(Json::as_i64),
            Some(3_062)
        );
        let blocks = parsed.get("blocks").and_then(Json::as_array).unwrap();
        assert_eq!(blocks.len(), 3);
        for block in blocks {
            assert!(block.get("name").and_then(Json::as_str).is_some());
            assert!(block
                .get("agreement")
                .and_then(|agreement| agreement.get("turns"))
                .and_then(Json::as_i64)
                .is_some());
        }
    }

    #[test]
    fn examples_are_capped_per_group() {
        let corpus = real_corpus();
        let report = GoldenReport::build(&compare_corpus(&corpus));
        for group in &report.by_expected_module {
            assert!(group.examples.len() <= MAX_GROUPED_DIFFS, "{}", group.name);
            assert!(group.diffs >= group.examples.len());
        }
    }

    #[test]
    fn the_report_is_deterministic_across_runs() {
        let corpus = real_corpus();
        let first =
            json::to_string(&GoldenReport::build(&compare_corpus(&corpus)).to_json()).unwrap();
        let second =
            json::to_string(&GoldenReport::build(&compare_corpus(&corpus)).to_json()).unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn an_empty_comparison_set_yields_an_empty_but_valid_report() {
        let report = GoldenReport::build(&[]);
        assert_eq!(report.turns, 0);
        assert_eq!(report.overall.turns, 0);
        assert!(report.overall.reply_percent().abs() < f64::EPSILON);
        let text = report.render_text();
        assert!(text.contains("0 turns"));
        let parsed = json::parse_str(&json::to_string(&report.to_json()).unwrap()).unwrap();
        assert_eq!(parsed.get("turns").and_then(Json::as_i64), Some(0));
    }

    #[test]
    fn report_truncation_collapses_whitespace_and_marks_cuts() {
        assert_eq!(truncate_for_report("a  b\nc", 10), "a b c");
        assert_eq!(truncate_for_report("abcdef", 3), "abc…");
        assert_eq!(truncate_for_report("abc", 3), "abc");
        assert_eq!(truncate_for_report("😀😀😀", 2), "😀😀…");
    }

    #[test]
    fn the_harness_measures_the_phase_one_gap_honestly() {
        // The report must show a real gap (skeleton modules) and real agreement (the greeting
        // skeleton) — if either became 100 %, one of the two is broken.
        let corpus = real_corpus();
        let report = GoldenReport::build(&compare_corpus(&corpus));
        assert!(report.overall.reply_percent() < 100.0);
        assert!(report.overall.module_percent() < 100.0);
        assert!(
            report.overall.reply_percent() > 1.0,
            "some replies do agree: {:.2}%",
            report.overall.reply_percent()
        );
        assert_eq!(report.overall.faulted, 0);
    }
}
