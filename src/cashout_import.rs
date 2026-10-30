//! Strict import of BAN `ArenaScenarioV1` lines (contract `ban-arena-cashout/1`).
//!
//! A dependency-free JSON reader that accepts what BAN's TL-Canon export
//! emits: objects, strings, non-negative integers and booleans. It rejects
//! floats, duplicate keys, and unknown or missing fields. An imported scenario
//! runs in the matching arena profile on its own amounts, and the arena's
//! oracle must equal BAN's `expected`.

use std::collections::BTreeSet;

use crate::cashout::{
    expected, run_cashout_with, CashoutProfile, CashoutQuote, CashoutScore, Currency, Expected,
    CONTRACT_VERSION, UNMODELED_BEHAVIORS,
};
use crate::market::{BISQ_EASY_MAX_USD_CENTS, BISQ_EASY_MIN_USD_CENTS};

const MAX_DEPTH: usize = 8;
const MAX_LINE_BYTES: usize = 16_384;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Json {
    Null,
    Bool(bool),
    Int(u64),
    Str(String),
    Array(Vec<Json>),
    Object(Vec<(String, Json)>),
}

pub fn parse_json(text: &str) -> Result<Json, String> {
    if text.len() > MAX_LINE_BYTES {
        return Err("line too long".to_string());
    }
    let mut parser = Parser {
        bytes: text.as_bytes(),
        pos: 0,
    };
    parser.ws();
    let value = parser.value(0)?;
    parser.ws();
    if parser.pos != parser.bytes.len() {
        return Err(format!("trailing characters at byte {}", parser.pos));
    }
    Ok(value)
}

struct Parser<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.pos += 1;
        }
    }

    fn expect(&mut self, byte: u8) -> Result<(), String> {
        if self.peek() != Some(byte) {
            return Err(format!("expected '{}' at byte {}", byte as char, self.pos));
        }
        self.pos += 1;
        Ok(())
    }

    fn value(&mut self, depth: usize) -> Result<Json, String> {
        if depth > MAX_DEPTH {
            return Err("nesting too deep".to_string());
        }
        match self.peek() {
            Some(b'{') => self.object(depth),
            Some(b'[') => self.array(depth),
            Some(b'"') => self.string().map(Json::Str),
            Some(b't') => self.literal("true", Json::Bool(true)),
            Some(b'f') => self.literal("false", Json::Bool(false)),
            Some(b'n') => self.literal("null", Json::Null),
            Some(b'0'..=b'9') => self.integer(),
            Some(b'-') => Err("negative numbers are not allowed".to_string()),
            _ => Err(format!("unexpected input at byte {}", self.pos)),
        }
    }

    fn literal(&mut self, word: &str, value: Json) -> Result<Json, String> {
        if !self.bytes[self.pos..].starts_with(word.as_bytes()) {
            return Err(format!("invalid literal at byte {}", self.pos));
        }
        self.pos += word.len();
        Ok(value)
    }

    fn integer(&mut self) -> Result<Json, String> {
        let start = self.pos;
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.pos += 1;
        }
        if matches!(self.peek(), Some(b'.' | b'e' | b'E')) {
            return Err("non-integer number".to_string());
        }
        let digits = &self.bytes[start..self.pos];
        if digits.len() > 1 && digits[0] == b'0' {
            return Err("leading zero".to_string());
        }
        std::str::from_utf8(digits)
            .ok()
            .and_then(|text| text.parse().ok())
            .map(Json::Int)
            .ok_or_else(|| "integer out of range".to_string())
    }

    fn string(&mut self) -> Result<String, String> {
        self.expect(b'"')?;
        let mut out = String::new();
        loop {
            let start = self.pos;
            while matches!(self.peek(), Some(b) if b != b'"' && b != b'\\' && b >= 0x20) {
                self.pos += 1;
            }
            // Runs end on ASCII bytes, so they are whole UTF-8 sequences.
            out.push_str(
                std::str::from_utf8(&self.bytes[start..self.pos])
                    .map_err(|_| "invalid UTF-8".to_string())?,
            );
            match self.peek() {
                Some(b'"') => {
                    self.pos += 1;
                    return Ok(out);
                }
                Some(b'\\') => {
                    self.pos += 1;
                    let escaped = match self.peek() {
                        Some(b'"') => '"',
                        Some(b'\\') => '\\',
                        Some(b'/') => '/',
                        Some(b'b') => '\u{8}',
                        Some(b'f') => '\u{c}',
                        Some(b'n') => '\n',
                        Some(b'r') => '\r',
                        Some(b't') => '\t',
                        Some(b'u') => {
                            let hex = self
                                .bytes
                                .get(self.pos + 1..self.pos + 5)
                                .filter(|hex| hex.iter().all(u8::is_ascii_hexdigit))
                                .ok_or_else(|| "invalid \\u escape".to_string())?;
                            let code = hex.iter().fold(0u32, |acc, digit| {
                                acc * 16 + char::from(*digit).to_digit(16).unwrap_or(0)
                            });
                            self.pos += 4;
                            char::from_u32(code).ok_or_else(|| {
                                "surrogate \\u escapes are not supported".to_string()
                            })?
                        }
                        _ => return Err(format!("invalid escape at byte {}", self.pos)),
                    };
                    self.pos += 1;
                    out.push(escaped);
                }
                _ => return Err("unterminated string or raw control character".to_string()),
            }
        }
    }

    fn array(&mut self, depth: usize) -> Result<Json, String> {
        self.expect(b'[')?;
        let mut items = Vec::new();
        self.ws();
        if self.peek() == Some(b']') {
            self.pos += 1;
            return Ok(Json::Array(items));
        }
        loop {
            self.ws();
            items.push(self.value(depth + 1)?);
            self.ws();
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b']') => {
                    self.pos += 1;
                    return Ok(Json::Array(items));
                }
                _ => return Err(format!("expected ',' or ']' at byte {}", self.pos)),
            }
        }
    }

    fn object(&mut self, depth: usize) -> Result<Json, String> {
        self.expect(b'{')?;
        let mut fields: Vec<(String, Json)> = Vec::new();
        self.ws();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            return Ok(Json::Object(fields));
        }
        loop {
            self.ws();
            let key = self.string()?;
            if fields.iter().any(|(existing, _)| *existing == key) {
                return Err(format!("duplicate key: {key}"));
            }
            self.ws();
            self.expect(b':')?;
            self.ws();
            let value = self.value(depth + 1)?;
            fields.push((key, value));
            self.ws();
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b'}') => {
                    self.pos += 1;
                    return Ok(Json::Object(fields));
                }
                _ => return Err(format!("expected ',' or '}}' at byte {}", self.pos)),
            }
        }
    }
}

fn field<'a>(fields: &'a [(String, Json)], key: &str) -> Option<&'a Json> {
    fields
        .iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value)
}

const SCENARIO_KEYS: [&str; 15] = [
    "v",
    "contract",
    "profile",
    "scenario",
    "seed",
    "amount_sats",
    "net_payout_minor",
    "currency",
    "exponent",
    "quote_expiry_height",
    "min_conf",
    "behavior",
    "expected",
    "arena_supported",
    "source",
];

pub const BAN_ACTORS: [&str; 4] = ["provider", "sender", "recipient", "environment"];

/// BAN's acting party for each behavior (`arena_contract.BEHAVIORS`).
pub fn acting_party(behavior: &str) -> Option<&'static str> {
    Some(match behavior {
        "honest" | "provider_withholds" | "duplicate_payout" | "claim_without_credit" => "provider",
        "sender_withholds" | "replay_funding" | "wrong_output" | "late_funding" => "sender",
        "stale_chain" | "rail_timeout" | "substituted_provider_key" => "environment",
        "alias_equivocation" => "recipient",
        _ => return None,
    })
}

/// BAN's `arena_supported`: the arena's Bisq Easy scope, USD $6-$600.
pub fn ban_arena_supported(currency: Currency, exponent: u8, net_payout_minor: u64) -> bool {
    currency == Currency::USD
        && exponent == 2
        && (BISQ_EASY_MIN_USD_CENTS..=BISQ_EASY_MAX_USD_CENTS).contains(&net_payout_minor)
}

/// One synthetic cashout case exported by BAN.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArenaScenarioV1 {
    pub profile: CashoutProfile,
    pub scenario: String,
    pub seed: u64,
    pub amount_sats: u64,
    pub net_payout_minor: u64,
    pub currency: Currency,
    pub exponent: u8,
    pub quote_expiry_height: u64,
    pub min_conf: u64,
    pub expected: Expected,
    pub arena_supported: bool,
}

pub fn parse_scenario(line: &str) -> Result<ArenaScenarioV1, String> {
    let Json::Object(fields) = parse_json(line.trim())? else {
        return Err("a scenario is a JSON object".to_string());
    };
    let keys: BTreeSet<&str> = fields.iter().map(|(key, _)| key.as_str()).collect();
    if keys != SCENARIO_KEYS.into_iter().collect() {
        return Err("scenario must have exactly the ArenaScenarioV1 fields".to_string());
    }
    let int = |key: &str| match field(&fields, key) {
        Some(Json::Int(value)) => Ok(*value),
        _ => Err(format!("{key} must be a non-negative integer")),
    };
    let text = |key: &str| match field(&fields, key) {
        Some(Json::Str(value)) => Ok(value.as_str()),
        _ => Err(format!("{key} must be a string")),
    };
    if int("v")? != 1
        || text("contract")? != CONTRACT_VERSION
        || text("source")? != "SYNTHETIC_FIXTURE"
    {
        return Err("unsupported scenario version, contract or source".to_string());
    }
    let profile = CashoutProfile::parse(text("profile")?).ok_or("unknown profile")?;
    let scenario = text("scenario")?;
    let acting = acting_party(scenario).ok_or_else(|| format!("unknown behavior: {scenario}"))?;
    let Some(Json::Object(behavior)) = field(&fields, "behavior") else {
        return Err("behavior must be an object".to_string());
    };
    // Exactly the four BAN actors: the acting party deviates, everyone else is honest.
    if behavior.len() != BAN_ACTORS.len() {
        return Err("behavior must name exactly the four BAN actors".to_string());
    }
    for actor in BAN_ACTORS {
        let want = if actor == acting { scenario } else { "honest" };
        if field(behavior, actor) != Some(&Json::Str(want.to_string())) {
            return Err(format!("behavior.{actor} must be {want}"));
        }
    }
    let currency =
        Currency::parse(text("currency")?).ok_or("currency must be an ISO 4217 alpha code")?;
    let exponent = u8::try_from(int("exponent")?)
        .ok()
        .filter(|exponent| *exponent <= 8)
        .ok_or("exponent must be 0..8")?;
    let net_payout_minor = int("net_payout_minor")?;
    let Some(Json::Bool(arena_supported)) = field(&fields, "arena_supported") else {
        return Err("arena_supported must be a boolean".to_string());
    };
    if *arena_supported != ban_arena_supported(currency, exponent, net_payout_minor) {
        return Err("arena_supported disagrees with the USD $6-$600 rule".to_string());
    }
    Ok(ArenaScenarioV1 {
        profile,
        scenario: scenario.to_string(),
        seed: int("seed")?,
        amount_sats: int("amount_sats")?,
        net_payout_minor,
        currency,
        exponent,
        quote_expiry_height: int("quote_expiry_height")?,
        min_conf: int("min_conf")?,
        expected: Expected::parse(text("expected")?).ok_or("unknown expected oracle")?,
        arena_supported: *arena_supported,
    })
}

#[derive(Debug)]
pub enum ImportOutcome {
    /// The arena ran the scenario. `agrees` compares its oracle with BAN's.
    Ran {
        score: CashoutScore,
        agrees: bool,
    },
    Skipped(&'static str),
}

/// Runs an imported scenario on its own amounts. `cashout-btc-first` never
/// converts currency, so it runs any ISO currency; `cashout-bisq-easy` reuses
/// the USD-only Bisq Easy machine and honors BAN's `arena_supported`.
pub fn run_imported(scenario: &ArenaScenarioV1) -> Result<ImportOutcome, String> {
    if UNMODELED_BEHAVIORS.contains(&scenario.scenario.as_str()) {
        return Ok(ImportOutcome::Skipped(
            "a BAN-side check the arena does not model",
        ));
    }
    if scenario.profile == CashoutProfile::BisqEasy && !scenario.arena_supported {
        return Ok(ImportOutcome::Skipped(
            "cashout-bisq-easy reuses the USD-only Bisq Easy machine",
        ));
    }
    let quote = CashoutQuote::new(
        scenario.amount_sats,
        scenario.net_payout_minor,
        scenario.currency,
        scenario.exponent,
        scenario.quote_expiry_height,
        scenario.min_conf,
    )
    .map_err(|fault| format!("invalid quote: {}", fault.code()))?;
    let score = run_cashout_with(scenario.profile, &scenario.scenario, quote)?;
    let agrees = expected(scenario.profile, &scenario.scenario) == Some(scenario.expected);
    Ok(ImportOutcome::Ran { score, agrees })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cashout::CASHOUT_SCENARIOS;

    /// BAN `fixtures/arena/cashout_scenarios.jsonl` at `BAN_CONTRACT_PIN`.
    const BAN_FIXTURE: &str = include_str!("../fixtures/ban/cashout_scenarios.jsonl");

    fn fixture() -> Vec<ArenaScenarioV1> {
        BAN_FIXTURE
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| parse_scenario(line).unwrap_or_else(|e| panic!("{e}: {line}")))
            .collect()
    }

    fn with_usd(line: &str) -> String {
        line.replace("\"arena_supported\":false", "\"arena_supported\":true")
            .replace("\"currency\":\"ILS\"", "\"currency\":\"USD\"")
    }

    #[test]
    fn every_ban_fixture_line_imports_and_its_oracle_matches_the_arena() {
        let scenarios = fixture();
        let pairs: BTreeSet<(&str, &str)> = scenarios
            .iter()
            .map(|s| (s.profile.id(), s.scenario.as_str()))
            .collect();
        assert_eq!(pairs.len(), 24, "12 behaviors under both profiles");
        for s in &scenarios {
            match expected(s.profile, &s.scenario) {
                Some(arena) => assert_eq!(arena, s.expected, "{}/{}", s.profile.id(), s.scenario),
                None => assert!(UNMODELED_BEHAVIORS.contains(&s.scenario.as_str())),
            }
        }
        for profile in CashoutProfile::ALL {
            for behavior in CASHOUT_SCENARIOS {
                assert!(pairs.contains(&(profile.id(), *behavior)), "{behavior}");
            }
        }
    }

    #[test]
    fn ban_btc_first_scenarios_run_on_bans_own_amounts() {
        let mut ran = 0;
        for s in fixture() {
            let outcome = run_imported(&s).unwrap();
            match (s.profile, outcome) {
                (CashoutProfile::BtcFirst, ImportOutcome::Ran { score, agrees }) => {
                    assert!(agrees && score.passed, "{}", score.json_line());
                    assert!(score
                        .json_line()
                        .contains("\"currency\":\"ILS\",\"exponent\":2"));
                    ran += 1;
                }
                (CashoutProfile::BisqEasy, ImportOutcome::Skipped(reason)) => {
                    assert!(
                        reason.contains("USD-only")
                            || UNMODELED_BEHAVIORS.contains(&s.scenario.as_str())
                    );
                }
                (_, ImportOutcome::Skipped(_)) => {
                    assert!(UNMODELED_BEHAVIORS.contains(&s.scenario.as_str()))
                }
                (profile, ImportOutcome::Ran { .. }) => panic!("{} ran ILS", profile.id()),
            }
        }
        assert_eq!(ran, CASHOUT_SCENARIOS.len());
    }

    #[test]
    fn usd_versions_of_the_ban_fixture_run_in_both_profiles() {
        let mut ran = 0;
        for line in BAN_FIXTURE.lines().filter(|line| !line.trim().is_empty()) {
            let s = parse_scenario(&with_usd(line)).unwrap();
            if let ImportOutcome::Ran { score, agrees } = run_imported(&s).unwrap() {
                assert!(agrees && score.passed, "{}", score.json_line());
                ran += 1;
            }
        }
        assert_eq!(ran, 2 * CASHOUT_SCENARIOS.len());
    }

    #[test]
    fn malformed_or_inconsistent_scenarios_are_rejected() {
        let line = BAN_FIXTURE.lines().nth(1).unwrap().trim();
        assert!(parse_scenario(line).is_ok());
        let bad = [
            line.replace("\"seed\":7", "\"seed\":7.0"),
            line.replace("\"seed\":7", "\"seed\":-7"),
            line.replace("\"seed\":7", "\"seed\":7,\"seed\":8"),
            line.replace("\"seed\":7", "\"seed\":7,\"extra\":1"),
            line.replace("\"seed\":7,", ""),
            line.replace("ban-arena-cashout/1", "ban-arena-cashout/2"),
            line.replace("SYNTHETIC_FIXTURE", "LIVE"),
            line.replace("\"exponent\":2", "\"exponent\":9"),
            line.replace("\"currency\":\"ILS\"", "\"currency\":\"ils\""),
            line.replace("\"arena_supported\":false", "\"arena_supported\":true"),
            // The withholding moved to the wrong actor.
            line.replace(
                "\"provider\":\"provider_withholds\"",
                "\"provider\":\"honest\"",
            )
            .replace("\"sender\":\"honest\"", "\"sender\":\"provider_withholds\""),
            line.replace(
                "\"expected\":\"exposure_observed\"",
                "\"expected\":\"maybe\"",
            ),
            format!("{line} x"),
        ];
        for text in bad {
            assert!(parse_scenario(&text).is_err(), "accepted: {text}");
        }
    }

    #[test]
    fn json_reader_handles_escapes_and_rejects_floats() {
        assert_eq!(
            parse_json(r#"{"a":"x\"\u0041\n","b":[true,null,0]}"#),
            Ok(Json::Object(vec![
                ("a".to_string(), Json::Str("x\"A\n".to_string())),
                (
                    "b".to_string(),
                    Json::Array(vec![Json::Bool(true), Json::Null, Json::Int(0)])
                ),
            ]))
        );
        for bad in [
            "1.5",
            "1e3",
            "01",
            "\"\\ud800\"",
            "{\"a\":1,}",
            "[1 2]",
            "\"a\u{1}\"",
        ] {
            assert!(parse_json(bad).is_err(), "accepted {bad}");
        }
    }
}
