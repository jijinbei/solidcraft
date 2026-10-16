//! Seeded goldens: what every robustness seed builds (`tests/robustness/goldens.json`).
//!
//! Each seed's random design (`fuzz::run`) is recorded: its steps, how it ended (ok, or the
//! kind and start of the error it was refused with) and, for what it left, the bodies' total
//! volume and area (within a relative 1e-6) and their face, edge and shell counts (exact). Any
//! change fails, the better and the worse alike: a seed that used to be refused and now builds
//! is recorded on purpose, with the reason. To record: `ROBUSTNESS_GOLDENS_BLESS="why" cargo
//! test --profile ci -p solidcraft-engine --lib robustness_goldens:: -- --include-ignored`
//! (docs/robustness.md). The reason goes into the file's `notes` with the seeds it changed.
//!
//! Pull requests that touch `crates/kernel` run a sample (`sample_0` … `sample_3`, every 8th
//! seed, split so the shards share them); pushes to main and the nightly run every seed
//! (`all_0` … `all_95`).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use serde_json::{Value, json};

use crate::fuzz;

/// Seeds 0..SEEDS, each with up to this many steps (as `fixed_seeds_run_clean`).
const SEEDS: u64 = 96;
const MAX_STEPS: usize = 8;
const REL: f64 = 1e-6;
const FULL_PARTS: u64 = SEEDS;

// A local libtest run includes both the sample and the full set. Calculate a seed once;
// separate nextest processes each calculate their own seed normally.
static RECORDS: [OnceLock<Value>; SEEDS as usize] = [const { OnceLock::new() }; SEEDS as usize];

/// Tests share the file when they record.
static FILE: Mutex<()> = Mutex::new(());

fn path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/robustness/goldens.json")
}

fn load() -> Value {
    std::fs::read(path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_else(|| json!({}))
}

/// How `got` differs from `want` (None when it matches within the tolerances).
fn differs(want: &Value, got: &Value) -> Option<String> {
    let mut out = Vec::new();
    for (k, g) in got.as_object().into_iter().flatten() {
        let w = &want[k];
        let same = match (w.as_f64(), g.as_f64()) {
            (Some(a), Some(b)) if matches!(k.as_str(), "volume" | "area") => (a - b).abs() <= REL * a.abs().max(b.abs()).max(1.0),
            _ => w == g,
        };
        if !same {
            out.push(format!("{k}: {w} -> {g}"));
        }
    }
    (!out.is_empty()).then(|| out.join("; "))
}

fn check(part: u64, parts: u64, every: u64) {
    let seeds = assigned_seeds(part, parts, every);
    let got: BTreeMap<u64, Value> = seeds.iter().map(|s| (*s, RECORDS[*s as usize].get_or_init(|| fuzz::golden(*s, MAX_STEPS)).clone())).collect();
    let _lock = FILE.lock().unwrap_or_else(|e| e.into_inner());
    let mut file = load();
    let changed: Vec<(u64, String)> = got
        .iter()
        .filter_map(|(seed, g)| {
            let w = &file["seeds"][seed.to_string()];
            if w.is_null() { Some((*seed, "new".to_string())) } else { differs(w, g).map(|d| (*seed, d)) }
        })
        .collect();
    if let Ok(reason) = std::env::var("ROBUSTNESS_GOLDENS_BLESS") {
        assert!(!reason.trim().is_empty(), "ROBUSTNESS_GOLDENS_BLESS needs the reason for the change");
        if changed.is_empty() {
            return;
        }
        if file.get("about").is_none() {
            file["about"] = json!(
                "What each robustness seed builds (crates/engine/src/tests/robustness_goldens.rs, docs/robustness.md). Record changes with ROBUSTNESS_GOLDENS_BLESS=\"why\"."
            );
        }
        for (seed, g) in &got {
            file["seeds"][seed.to_string()] = g.clone();
        }
        let mut notes = file["notes"].as_array().cloned().unwrap_or_default();
        let seeds: Vec<u64> = changed.iter().map(|(s, _)| *s).collect();
        // The parts of one record (the tests run side by side) share one note.
        match notes.last_mut() {
            Some(last) if last["reason"] == json!(reason) => {
                let mut all: Vec<u64> = last["seeds"].as_array().into_iter().flatten().filter_map(Value::as_u64).chain(seeds).collect();
                all.sort_unstable();
                all.dedup();
                last["seeds"] = json!(all);
            }
            _ => notes.push(json!({"reason": reason, "seeds": seeds})),
        }
        file["notes"] = Value::Array(notes);
        // Seeds in order, one per line, so a change is a small diff.
        let seeds: BTreeMap<u64, Value> =
            file["seeds"].as_object().into_iter().flatten().filter_map(|(k, v)| Some((k.parse::<u64>().ok()?, v.clone()))).collect();
        let mut s = format!("{{\n \"about\": {},\n \"seeds\": {{\n", file["about"]);
        let n = seeds.len();
        for (i, (k, v)) in seeds.iter().enumerate() {
            s += &format!("  \"{k}\": {}{}\n", v, if i + 1 < n { "," } else { "" });
        }
        let notes: Vec<String> = file["notes"].as_array().into_iter().flatten().map(|n| format!("  {n}")).collect();
        s += &format!(" }},\n \"notes\": [\n{}\n ]\n}}\n", notes.join(",\n"));
        std::fs::write(path(), s).unwrap_or_else(|e| panic!("{}: {e}", path().display()));
        return;
    }
    assert!(
        changed.is_empty(),
        "{} seed(s) build differently from tests/robustness/goldens.json (if that is intended, record it with ROBUSTNESS_GOLDENS_BLESS=\"why\" and say why in the pull request):\n{}",
        changed.len(),
        changed.iter().map(|(s, d)| format!("seed {s}: {d}")).collect::<Vec<_>>().join("\n")
    );
}

#[test]
fn sample_0() {
    check(0, 4, 8);
}

#[test]
fn sample_1() {
    check(1, 4, 8);
}

#[test]
fn sample_2() {
    check(2, 4, 8);
}

#[test]
fn sample_3() {
    check(3, 4, 8);
}

fn assigned_seeds(part: u64, parts: u64, every: u64) -> Vec<u64> {
    (0..SEEDS).filter(|s| s % every == 0 && (s / every) % parts == part).collect()
}

macro_rules! full_parts {
    ($($name:ident => $part:literal),+ $(,)?) => {
        $(
            #[test]
            fn $name() {
                check($part, FULL_PARTS, 1);
            }
        )+

        #[test]
        fn partitions_cover_every_seed_once() {
            let mut all: Vec<_> = [$($part),+].into_iter().flat_map(|part| assigned_seeds(part, FULL_PARTS, 1)).collect();
            all.sort_unstable();
            assert_eq!(all, (0..SEEDS).collect::<Vec<_>>());
            let mut samples: Vec<_> = (0..4).flat_map(|part| assigned_seeds(part, 4, 8)).collect();
            samples.sort_unstable();
            assert_eq!(samples, (0..SEEDS).step_by(8).collect::<Vec<_>>());
        }
    };
}

full_parts! {
    all_0 => 0,
    all_1 => 1,
    all_2 => 2,
    all_3 => 3,
    all_4 => 4,
    all_5 => 5,
    all_6 => 6,
    all_7 => 7,
    all_8 => 8,
    all_9 => 9,
    all_10 => 10,
    all_11 => 11,
    all_12 => 12,
    all_13 => 13,
    all_14 => 14,
    all_15 => 15,
    all_16 => 16,
    all_17 => 17,
    all_18 => 18,
    all_19 => 19,
    all_20 => 20,
    all_21 => 21,
    all_22 => 22,
    all_23 => 23,
    all_24 => 24,
    all_25 => 25,
    all_26 => 26,
    all_27 => 27,
    all_28 => 28,
    all_29 => 29,
    all_30 => 30,
    all_31 => 31,
    all_32 => 32,
    all_33 => 33,
    all_34 => 34,
    all_35 => 35,
    all_36 => 36,
    all_37 => 37,
    all_38 => 38,
    all_39 => 39,
    all_40 => 40,
    all_41 => 41,
    all_42 => 42,
    all_43 => 43,
    all_44 => 44,
    all_45 => 45,
    all_46 => 46,
    all_47 => 47,
    all_48 => 48,
    all_49 => 49,
    all_50 => 50,
    all_51 => 51,
    all_52 => 52,
    all_53 => 53,
    all_54 => 54,
    all_55 => 55,
    all_56 => 56,
    all_57 => 57,
    all_58 => 58,
    all_59 => 59,
    all_60 => 60,
    all_61 => 61,
    all_62 => 62,
    all_63 => 63,
    all_64 => 64,
    all_65 => 65,
    all_66 => 66,
    all_67 => 67,
    all_68 => 68,
    all_69 => 69,
    all_70 => 70,
    all_71 => 71,
    all_72 => 72,
    all_73 => 73,
    all_74 => 74,
    all_75 => 75,
    all_76 => 76,
    all_77 => 77,
    all_78 => 78,
    all_79 => 79,
    all_80 => 80,
    all_81 => 81,
    all_82 => 82,
    all_83 => 83,
    all_84 => 84,
    all_85 => 85,
    all_86 => 86,
    all_87 => 87,
    all_88 => 88,
    all_89 => 89,
    all_90 => 90,
    all_91 => 91,
    all_92 => 92,
    all_93 => 93,
    all_94 => 94,
    all_95 => 95,
}
