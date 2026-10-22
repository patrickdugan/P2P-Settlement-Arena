//! Repeatable attacker/defender benchmark scenarios and machine-readable scoring.

use crate::{AckPolicy, Action, Actor, CrashPoint, Event, Fault, Node, Phase, TRADE_SATS};

pub const SCENARIOS: &[&str] = &[
    "baseline",
    "duplicate-replay",
    "out-of-order",
    "crash-before-commit",
    "crash-after-commit",
    "ack-before-commit",
    "conflicting-message-id",
    "unauthorized-release",
    "double-settlement",
    "dispute-resolution",
    "watchtower-success",
    "watchtower-timeout",
    "fault-sweep",
    "peer-reconciliation",
];

#[derive(Debug)]
pub struct Score {
    pub scenario: &'static str,
    pub passed: bool,
    pub attack_success: bool,
    pub recovered: bool,
    pub trials: usize,
    pub safety_findings: Vec<Fault>,
    pub committed_events: usize,
    pub acknowledgements: usize,
    pub final_phase: Phase,
}

impl Score {
    /// One JSON object per line, for ingestion by an external evaluation harness.
    pub fn json_line(&self) -> String {
        let findings = self
            .safety_findings
            .iter()
            .map(|fault| format!("\"{}\"", fault.code()))
            .collect::<Vec<_>>()
            .join(",");
        format!(
            "{{\"protocol\":\"bisq-musig-model\",\"scenario\":\"{}\",\"passed\":{},\"attack_success\":{},\"recovered\":{},\"trials\":{},\"committed_events\":{},\"acknowledgements\":{},\"final_phase\":\"{:?}\",\"safety_findings\":[{}]}}",
            self.scenario,
            self.passed,
            self.attack_success,
            self.recovered,
            self.trials,
            self.committed_events,
            self.acknowledgements,
            self.final_phase,
            findings
        )
    }
}

fn event(id: u64, actor: Actor, action: Action) -> Event {
    Event::new(id, actor, action)
}

fn standard_events() -> Vec<Event> {
    vec![
        event(1, Actor::Seller, Action::Lock { sats: TRADE_SATS }),
        event(2, Actor::Buyer, Action::MarkFiatSent),
        event(3, Actor::Seller, Action::ConfirmFiat),
        event(4, Actor::Seller, Action::Release),
    ]
}

fn receive(node: &mut Node, e: Event, point: CrashPoint) -> Result<(), String> {
    node.receive(e, point)
        .map(|_| ())
        .map_err(|fault| format!("unexpected protocol error: {}", fault.code()))
}

fn happy_path(node: &mut Node) -> Result<(), String> {
    for e in standard_events() {
        receive(node, e, CrashPoint::None)?;
    }
    Ok(())
}

fn score(
    scenario: &'static str,
    node: &Node,
    passed: bool,
    attack_success: bool,
    recovered: bool,
    trials: usize,
) -> Score {
    Score {
        scenario,
        passed,
        attack_success,
        recovered,
        trials,
        safety_findings: node.audit(),
        committed_events: node.journal.len(),
        acknowledgements: node.acks_issued.len(),
        final_phase: node.live.phase,
    }
}

pub fn run_scenario(name: &str) -> Result<Score, String> {
    let mut node = Node::new(AckPolicy::AfterCommit);
    match name {
        "baseline" => {
            happy_path(&mut node)?;
            let passed = node.live.phase == Phase::Settled && node.audit().is_empty();
            Ok(score("baseline", &node, passed, false, false, 1))
        }
        "duplicate-replay" => {
            let first = standard_events().remove(0);
            receive(&mut node, first.clone(), CrashPoint::None)?;
            let duplicate = node
                .receive(first, CrashPoint::None)
                .map_err(|f| f.code().to_string())?
                .duplicate;
            for e in standard_events().into_iter().skip(1) {
                receive(&mut node, e, CrashPoint::None)?;
            }
            let passed = duplicate && node.journal.len() == 4 && node.audit().is_empty();
            Ok(score("duplicate-replay", &node, passed, false, false, 1))
        }
        "out-of-order" => {
            let rejected = node.receive(standard_events()[1].clone(), CrashPoint::None)
                == Err(Fault::InvalidTransition);
            happy_path(&mut node)?;
            let passed = rejected && node.live.phase == Phase::Settled && node.audit().is_empty();
            Ok(score("out-of-order", &node, passed, false, false, 1))
        }
        "crash-before-commit" | "crash-after-commit" => {
            let first = standard_events().remove(0);
            let point = if name == "crash-before-commit" {
                CrashPoint::BeforeCommit
            } else {
                CrashPoint::AfterCommitBeforeAck
            };
            let receipt = node
                .receive(first.clone(), point)
                .map_err(|f| f.code().to_string())?;
            node.restart().map_err(|f| f.code().to_string())?;
            receive(&mut node, first, CrashPoint::None)?;
            for e in standard_events().into_iter().skip(1) {
                receive(&mut node, e, CrashPoint::None)?;
            }
            let passed = !receipt.acknowledged
                && receipt.crashed
                && node.live.phase == Phase::Settled
                && node.journal.len() == 4
                && node.audit().is_empty();
            let scenario = if name == "crash-before-commit" {
                "crash-before-commit"
            } else {
                "crash-after-commit"
            };
            Ok(score(scenario, &node, passed, false, true, 1))
        }
        "ack-before-commit" => {
            node = Node::new(AckPolicy::UnsafeBeforeCommit);
            let receipt = node
                .receive(standard_events().remove(0), CrashPoint::BeforeCommit)
                .map_err(|f| f.code().to_string())?;
            node.restart().map_err(|f| f.code().to_string())?;
            let attack = receipt.acknowledged
                && !receipt.committed
                && node.audit().contains(&Fault::AckWithoutDurableCommit);
            Ok(score("ack-before-commit", &node, attack, attack, true, 1))
        }
        "conflicting-message-id" => {
            receive(&mut node, standard_events().remove(0), CrashPoint::None)?;
            let rejected = node.receive(
                event(1, Actor::Buyer, Action::MarkFiatSent),
                CrashPoint::None,
            ) == Err(Fault::ConflictingReplay);
            for e in standard_events().into_iter().skip(1) {
                receive(&mut node, e, CrashPoint::None)?;
            }
            let passed = rejected && node.audit().is_empty() && node.journal.len() == 4;
            Ok(score(
                "conflicting-message-id",
                &node,
                passed,
                false,
                false,
                1,
            ))
        }
        "unauthorized-release" => {
            for e in standard_events().into_iter().take(3) {
                receive(&mut node, e, CrashPoint::None)?;
            }
            let rejected = node.receive(event(5, Actor::Buyer, Action::Release), CrashPoint::None)
                == Err(Fault::Unauthorized);
            receive(&mut node, standard_events()[3].clone(), CrashPoint::None)?;
            let passed = rejected && node.audit().is_empty() && node.live.phase == Phase::Settled;
            Ok(score(
                "unauthorized-release",
                &node,
                passed,
                false,
                false,
                1,
            ))
        }
        "double-settlement" => {
            happy_path(&mut node)?;
            let rejected = node.receive(
                event(5, Actor::Mediator, Action::ResolveToSeller),
                CrashPoint::None,
            ) == Err(Fault::InvalidTransition);
            let passed = rejected && node.audit().is_empty() && node.live.balances.escrow == 0;
            Ok(score("double-settlement", &node, passed, false, false, 1))
        }
        "dispute-resolution" => {
            receive(&mut node, standard_events()[0].clone(), CrashPoint::None)?;
            receive(&mut node, standard_events()[1].clone(), CrashPoint::None)?;
            receive(
                &mut node,
                event(3, Actor::Buyer, Action::OpenDispute),
                CrashPoint::None,
            )?;
            receive(
                &mut node,
                event(4, Actor::Mediator, Action::ResolveToBuyer),
                CrashPoint::None,
            )?;
            let passed = node.live.phase == Phase::Settled && node.audit().is_empty();
            Ok(score("dispute-resolution", &node, passed, false, false, 1))
        }
        "watchtower-success" | "watchtower-timeout" => {
            happy_path(&mut node)?;
            receive(
                &mut node,
                event(5, Actor::Seller, Action::BroadcastWarning),
                CrashPoint::None,
            )?;
            if name == "watchtower-success" {
                receive(
                    &mut node,
                    event(6, Actor::Mediator, Action::Tick),
                    CrashPoint::None,
                )?;
                receive(
                    &mut node,
                    event(7, Actor::Watchtower, Action::CounterWarning),
                    CrashPoint::None,
                )?;
                let passed = node.live.blocked_warnings == 1 && node.audit().is_empty();
                Ok(score("watchtower-success", &node, passed, false, false, 1))
            } else {
                for id in 6..=8 {
                    receive(
                        &mut node,
                        event(id, Actor::Mediator, Action::Tick),
                        CrashPoint::None,
                    )?;
                }
                let attack = node.audit().contains(&Fault::MissedWatchtowerWindow);
                Ok(score("watchtower-timeout", &node, attack, attack, false, 1))
            }
        }
        "fault-sweep" => {
            let mut passed = true;
            for fail_at in 0..4 {
                for point in [CrashPoint::BeforeCommit, CrashPoint::AfterCommitBeforeAck] {
                    let mut probe = Node::new(AckPolicy::AfterCommit);
                    for (index, e) in standard_events().into_iter().enumerate() {
                        if index == fail_at {
                            let receipt = probe
                                .receive(e.clone(), point)
                                .map_err(|f| f.code().to_string())?;
                            if receipt.acknowledged {
                                passed = false;
                            }
                            probe.restart().map_err(|f| f.code().to_string())?;
                            receive(&mut probe, e, CrashPoint::None)?;
                        } else {
                            receive(&mut probe, e, CrashPoint::None)?;
                        }
                    }
                    if !probe.audit().is_empty()
                        || probe.live.phase != Phase::Settled
                        || probe.journal.len() != 4
                    {
                        passed = false;
                    }
                    node = probe;
                }
            }
            Ok(score("fault-sweep", &node, passed, false, true, 8))
        }
        "peer-reconciliation" => {
            let mut other = Node::new(AckPolicy::AfterCommit);
            receive(&mut node, standard_events()[0].clone(), CrashPoint::None)?;
            receive(&mut other, standard_events()[0].clone(), CrashPoint::None)?;
            receive(&mut node, standard_events()[1].clone(), CrashPoint::None)?;
            let temporarily_diverged = node.live != other.live;
            receive(&mut other, standard_events()[1].clone(), CrashPoint::None)?;
            for e in standard_events().into_iter().skip(2) {
                receive(&mut node, e.clone(), CrashPoint::None)?;
                receive(&mut other, e, CrashPoint::None)?;
            }
            let passed = temporarily_diverged
                && node.live == other.live
                && node.journal == other.journal
                && node.audit().is_empty()
                && other.audit().is_empty();
            Ok(score("peer-reconciliation", &node, passed, false, true, 1))
        }
        _ => Err(format!(
            "unknown scenario '{name}'. Choices: {}",
            SCENARIOS.join(", ")
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_named_scenario_matches_its_oracle() {
        for scenario in SCENARIOS {
            let result = run_scenario(scenario).unwrap_or_else(|err| panic!("{scenario}: {err}"));
            assert!(result.passed, "scenario failed: {scenario}");
        }
    }

    #[test]
    fn only_intentional_vulnerabilities_have_findings() {
        for scenario in SCENARIOS {
            let result = run_scenario(scenario).unwrap();
            if matches!(*scenario, "ack-before-commit" | "watchtower-timeout") {
                assert!(result.attack_success);
                assert!(!result.safety_findings.is_empty());
            } else {
                assert!(!result.attack_success);
                assert!(
                    result.safety_findings.is_empty(),
                    "unexpected finding: {scenario}"
                );
            }
        }
    }

    #[test]
    fn machine_output_contains_integrity_verdict() {
        let json = run_scenario("ack-before-commit").unwrap().json_line();
        assert!(json.contains(r#""attack_success":true"#));
        assert!(json.contains("ack_without_durable_commit"));
    }
}
