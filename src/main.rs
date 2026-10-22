use p2p_settlement_arena::agents::{run_bisq_easy, BISQ_EASY_SCENARIOS};
use p2p_settlement_arena::market::ProtocolProfile;
use p2p_settlement_arena::scenarios::{run_scenario, SCENARIOS};

fn usage() {
    eprintln!(
        "P2P Settlement Arena (default: Bisq Easy)\n\
         Usage:\n\
           settlement-arena                                 # Bisq Easy honest trade\n\
           settlement-arena list\n\
           settlement-arena all\n\
           settlement-arena simulate [scenario|all|list] [--profile bisq-easy|bisq-musig]\n\
           settlement-arena profiles\n\
         Output: JSONL for scenarios, nonzero exit for failed test or invalid input."
    );
}

fn run_one(profile: ProtocolProfile, name: &str) -> Result<(String, bool), String> {
    match profile {
        ProtocolProfile::BisqEasy => {
            let score = run_bisq_easy(name)?;
            Ok((score.json_line(), score.passed))
        }
        ProtocolProfile::BisqMuSigModel => {
            let score = run_scenario(name)?;
            Ok((score.json_line(), score.passed))
        }
    }
}

fn names(profile: ProtocolProfile) -> &'static [&'static str] {
    match profile {
        ProtocolProfile::BisqEasy => BISQ_EASY_SCENARIOS,
        ProtocolProfile::BisqMuSigModel => SCENARIOS,
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if matches!(
        args.get(1).map(String::as_str),
        Some("--help" | "-h" | "help")
    ) {
        usage();
        return;
    }
    if args.get(1).map(String::as_str) == Some("profiles") {
        println!("bisq-easy (default: payment-first, reputation-backed, no escrow)");
        println!("bisq-musig (experimental abstract escrow, journal, watchtower)");
        return;
    }

    let mut profile = ProtocolProfile::default();
    let mut selection: Option<&str> = None;
    let mut args_iter = args.iter().skip(1);
    while let Some(arg) = args_iter.next() {
        match arg.as_str() {
            "simulate" | "run" => {}
            "--profile" => {
                let name = match args_iter.next() {
                    Some(name) => name,
                    None => {
                        usage();
                        std::process::exit(2);
                    }
                };
                profile = match ProtocolProfile::parse(name) {
                    Some(profile) => profile,
                    None => {
                        eprintln!("unknown profile: {name}");
                        usage();
                        std::process::exit(2);
                    }
                };
            }
            token => {
                if selection.replace(token).is_some() {
                    eprintln!("only one scenario can be selected");
                    usage();
                    std::process::exit(2);
                }
            }
        }
    }
    let selection = selection.unwrap_or(profile.default_scenario());
    if selection == "list" {
        for name in names(profile) {
            println!("{name}");
        }
        return;
    }
    let scenarios = if selection == "all" {
        names(profile).to_vec()
    } else {
        vec![selection]
    };

    let mut failures = 0;
    for name in scenarios {
        match run_one(profile, name) {
            Ok((line, passed)) => {
                println!("{line}");
                if !passed {
                    failures += 1;
                }
            }
            Err(error) => {
                eprintln!("{error}");
                failures += 1;
            }
        }
    }
    if failures > 0 {
        std::process::exit(1);
    }
}
