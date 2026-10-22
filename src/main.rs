use p2p_settlement_arena::scenarios::{run_scenario, SCENARIOS};

fn usage() {
    eprintln!(
        "P2P Settlement Arena\n\
         Usage:\n\
           settlement-arena list\n\
           settlement-arena run <scenario>\n\
           settlement-arena all\n\
         Output: one JSON record per executed scenario"
    );
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("list") => {
            for scenario in SCENARIOS {
                println!("{scenario}");
            }
        }
        Some("run") => {
            if let Some(name) = args.get(2) {
                match run_scenario(name) {
                    Ok(score) => {
                        println!("{}", score.json_line());
                        if !score.passed {
                            std::process::exit(1);
                        }
                    }
                    Err(error) => {
                        eprintln!("{error}");
                        std::process::exit(2);
                    }
                }
            } else {
                usage();
                std::process::exit(2);
            }
        }
        Some("all") => {
            let mut failures = 0;
            for name in SCENARIOS {
                match run_scenario(name) {
                    Ok(score) => {
                        println!("{}", score.json_line());
                        if !score.passed {
                            failures += 1;
                        }
                    }
                    Err(error) => {
                        eprintln!("{name}: {error}");
                        failures += 1;
                    }
                }
            }
            if failures > 0 {
                std::process::exit(1);
            }
        }
        _ => {
            usage();
            std::process::exit(2);
        }
    }
}
