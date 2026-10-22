//! Agent strategies and benchmark oracles for the Bisq Easy market profile.

use crate::market::{
    EasyAction, EasyActor, EasyEvent, EasyFault, EasyNode, EasyPhase, Offer, Oracle, Rail,
    BISQ_EASY_MAX_USD_CENTS, ORACLE_MAX_AGE_SECONDS, ORACLE_MAX_SKEW_SECONDS, SIMULATED_NOW,
};

pub const BISQ_EASY_SCENARIOS: &[&str] = &[
    "honest-trade",
    "seller-withholds",
    "chargeback-after-delivery",
    "invoice-overpay",
    "price-spec-tamper",
    "low-reputation",
    "stale-price",
    "oracle-skew",
    "fake-fiat-notice",
    "duplicate-replay",
    "conflicting-replay",
    "irreversible-rail",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuyerStrategy {
    Honest,
    FalsePaymentClaim,
    OverpayInvoice,
    ChargebackAfterDelivery,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SellerStrategy {
    Honest,
    WithholdBitcoin,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AgentPair {
    pub buyer: BuyerStrategy,
    pub seller: SellerStrategy,
}

impl Default for AgentPair {
    fn default() -> Self {
        Self {
            buyer: BuyerStrategy::Honest,
            seller: SellerStrategy::Honest,
        }
    }
}

#[derive(Debug)]
pub struct MarketScore {
    pub protocol: &'static str,
    pub scenario: &'static str,
    pub passed: bool,
    pub attack_success: bool,
    pub blocked_attacks: usize,
    pub committed_events: usize,
    pub final_phase: EasyPhase,
    pub buyer_usd_at_risk_cents: u64,
    pub seller_btc_at_risk_sats: u64,
    pub safety_findings: Vec<EasyFault>,
}

impl MarketScore {
    pub fn json_line(&self) -> String {
        let findings = self
            .safety_findings
            .iter()
            .map(|fault| format!("\"{}\"", fault.code()))
            .collect::<Vec<_>>()
            .join(",");
        format!(
            "{{\"protocol\":\"{}\",\"scenario\":\"{}\",\"passed\":{},\"attack_success\":{},\"blocked_attacks\":{},\"committed_events\":{},\"final_phase\":\"{:?}\",\"buyer_usd_at_risk_cents\":{},\"seller_btc_at_risk_sats\":{},\"safety_findings\":[{}]}}",
            self.protocol,
            self.scenario,
            self.passed,
            self.attack_success,
            self.blocked_attacks,
            self.committed_events,
            self.final_phase,
            self.buyer_usd_at_risk_cents,
            self.seller_btc_at_risk_sats,
            findings
        )
    }
}

struct Runner {
    node: EasyNode,
    id: u64,
    blocked: usize,
}

impl Runner {
    fn new(node: EasyNode) -> Self {
        Self {
            node,
            id: 1,
            blocked: 0,
        }
    }

    fn event(&mut self, actor: EasyActor, action: EasyAction) -> EasyEvent {
        let e = EasyEvent::new(self.id, actor, action);
        self.id += 1;
        e
    }

    fn send(&mut self, actor: EasyActor, action: EasyAction) -> Result<(), String> {
        let e = self.event(actor, action);
        self.node
            .receive(e)
            .map(|_| ())
            .map_err(|fault| format!("unexpected event failure: {}", fault.code()))
    }

    fn expect_rejection(
        &mut self,
        actor: EasyActor,
        action: EasyAction,
        expected: EasyFault,
    ) -> Result<(), String> {
        let e = self.event(actor, action);
        let result = self.node.receive(e);
        if result != Err(expected) {
            return Err(format!(
                "expected rejection {}, got {result:?}",
                expected.code()
            ));
        }
        self.blocked += 1;
        Ok(())
    }

    fn accept(&mut self) -> Result<(), String> {
        self.send(
            EasyActor::Buyer,
            EasyAction::TakeOffer {
                contract_price: self.node.offer.fixed_price_usd_cents_per_btc,
            },
        )
    }

    fn payment(&mut self) -> Result<(), String> {
        self.send(EasyActor::Bank, EasyAction::TransferFiat)?;
        self.send(EasyActor::Buyer, EasyAction::ClaimFiatSent)?;
        self.send(EasyActor::Seller, EasyAction::ConfirmFiat)?;
        Ok(())
    }

    fn payout(&mut self) -> Result<(), String> {
        self.send(
            EasyActor::Buyer,
            EasyAction::SetBitcoinPaymentRequest {
                requested_sats: Some(self.node.offer.btc_sats),
            },
        )?;
        self.send(EasyActor::Seller, EasyAction::SendBitcoin)?;
        Ok(())
    }

    fn run_pair(&mut self, pair: AgentPair) -> Result<(), String> {
        self.accept()?;
        if pair.buyer == BuyerStrategy::FalsePaymentClaim {
            self.send(EasyActor::Buyer, EasyAction::ClaimFiatSent)?;
            self.expect_rejection(
                EasyActor::Seller,
                EasyAction::ConfirmFiat,
                EasyFault::FiatNotReceived,
            )?;
            self.send(EasyActor::Bank, EasyAction::TransferFiat)?;
            self.send(EasyActor::Seller, EasyAction::ConfirmFiat)?;
        } else {
            self.payment()?;
        }

        if pair.buyer == BuyerStrategy::OverpayInvoice {
            self.expect_rejection(
                EasyActor::Buyer,
                EasyAction::SetBitcoinPaymentRequest {
                    requested_sats: Some(self.node.offer.btc_sats * 10),
                },
                EasyFault::PaymentRequestAmountMismatch,
            )?;
        }

        if pair.seller == SellerStrategy::WithholdBitcoin {
            for _ in 0..4 {
                self.send(EasyActor::Clock, EasyAction::Tick)?;
            }
        } else {
            self.payout()?;
        }

        if pair.buyer == BuyerStrategy::ChargebackAfterDelivery {
            self.send(EasyActor::Bank, EasyAction::Chargeback)?;
        }
        Ok(())
    }

    fn into_score(self, name: &'static str, passed: bool, attack_success: bool) -> MarketScore {
        let findings = self.node.audit();
        MarketScore {
            protocol: "bisq-easy",
            scenario: name,
            passed,
            attack_success,
            blocked_attacks: self.blocked,
            committed_events: self.node.journal.len(),
            final_phase: self.node.state.phase,
            buyer_usd_at_risk_cents: self.node.buyer_usd_at_risk_cents(),
            seller_btc_at_risk_sats: self.node.seller_btc_at_risk_sats(),
            safety_findings: findings,
        }
    }
}

pub fn run_bisq_easy(name: &str) -> Result<MarketScore, String> {
    let mut r = Runner::new(EasyNode::default());
    match name {
        "honest-trade" => {
            r.run_pair(AgentPair::default())?;
            let passed = r.node.state.phase == EasyPhase::Completed && r.node.audit().is_empty();
            Ok(r.into_score("honest-trade", passed, false))
        }
        "seller-withholds" => {
            r.run_pair(AgentPair {
                seller: SellerStrategy::WithholdBitcoin,
                ..AgentPair::default()
            })?;
            let attack = r.node.audit().contains(&EasyFault::SellerNonDelivery)
                && r.node.buyer_usd_at_risk_cents() == r.node.offer.quote_usd_cents;
            Ok(r.into_score("seller-withholds", attack, attack))
        }
        "chargeback-after-delivery" => {
            r.node.offer.rail = Rail::SimulatedReversible;
            r.run_pair(AgentPair {
                buyer: BuyerStrategy::ChargebackAfterDelivery,
                ..AgentPair::default()
            })?;
            let attack = r.node.audit().contains(&EasyFault::PostDeliveryChargeback)
                && r.node.seller_btc_at_risk_sats() > 0;
            Ok(r.into_score("chargeback-after-delivery", attack, attack))
        }
        "invoice-overpay" => {
            r.run_pair(AgentPair {
                buyer: BuyerStrategy::OverpayInvoice,
                ..AgentPair::default()
            })?;
            let passed = r.blocked == 1
                && r.node.state.phase == EasyPhase::Completed
                && r.node.audit().is_empty();
            Ok(r.into_score("invoice-overpay", passed, false))
        }
        "fake-fiat-notice" => {
            r.run_pair(AgentPair {
                buyer: BuyerStrategy::FalsePaymentClaim,
                ..AgentPair::default()
            })?;
            let passed = r.blocked == 1
                && r.node.state.phase == EasyPhase::Completed
                && r.node.audit().is_empty();
            Ok(r.into_score("fake-fiat-notice", passed, false))
        }
        "price-spec-tamper" => {
            r.expect_rejection(
                EasyActor::Buyer,
                EasyAction::TakeOffer {
                    contract_price: r.node.offer.fixed_price_usd_cents_per_btc - 1,
                },
                EasyFault::ContractPriceMismatch,
            )?;
            r.run_pair(AgentPair::default())?;
            let passed = r.blocked == 1 && r.node.audit().is_empty();
            Ok(r.into_score("price-spec-tamper", passed, false))
        }
        "low-reputation" => {
            let mut offer = Offer::default();
            offer.seller_reputation_points = 1_200;
            r = Runner::new(EasyNode::new(offer, Oracle::default(), SIMULATED_NOW));
            r.expect_rejection(
                EasyActor::Buyer,
                EasyAction::TakeOffer {
                    contract_price: offer.fixed_price_usd_cents_per_btc,
                },
                EasyFault::LowSellerReputation,
            )?;
            let passed = r.node.state.phase == EasyPhase::Available && r.node.journal.is_empty();
            Ok(r.into_score("low-reputation", passed, false))
        }
        "stale-price" => {
            r.node.oracle.market_observed_at = SIMULATED_NOW - ORACLE_MAX_AGE_SECONDS;
            r.node.oracle.usd_observed_at = SIMULATED_NOW - ORACLE_MAX_AGE_SECONDS;
            r.expect_rejection(
                EasyActor::Buyer,
                EasyAction::TakeOffer {
                    contract_price: r.node.offer.fixed_price_usd_cents_per_btc,
                },
                EasyFault::StaleMarketPrice,
            )?;
            let passed = r.blocked == 1 && r.node.journal.is_empty();
            Ok(r.into_score("stale-price", passed, false))
        }
        "oracle-skew" => {
            r.node.oracle.usd_observed_at = SIMULATED_NOW - ORACLE_MAX_SKEW_SECONDS - 1;
            r.expect_rejection(
                EasyActor::Buyer,
                EasyAction::TakeOffer {
                    contract_price: r.node.offer.fixed_price_usd_cents_per_btc,
                },
                EasyFault::OracleTimestampSkew,
            )?;
            let passed = r.blocked == 1 && r.node.journal.is_empty();
            Ok(r.into_score("oracle-skew", passed, false))
        }
        "duplicate-replay" => {
            r.accept()?;
            let first = r.node.journal[0];
            let duplicate = r.node.receive(first);
            if duplicate != Ok(false) {
                return Err(format!("duplicate event accepted as new: {duplicate:?}"));
            }
            r.payment()?;
            r.payout()?;
            let passed = r.node.journal.len() == 6 && r.node.audit().is_empty();
            Ok(r.into_score("duplicate-replay", passed, false))
        }
        "conflicting-replay" => {
            r.accept()?;
            let same_id = EasyEvent::new(
                r.node.journal[0].id,
                EasyActor::Bank,
                EasyAction::TransferFiat,
            );
            if r.node.receive(same_id) != Err(EasyFault::ConflictingReplay) {
                return Err("conflicting message ID was not rejected".to_string());
            }
            r.blocked += 1;
            r.payment()?;
            r.payout()?;
            let passed = r.blocked == 1 && r.node.journal.len() == 6 && r.node.audit().is_empty();
            Ok(r.into_score("conflicting-replay", passed, false))
        }
        "irreversible-rail" => {
            r.run_pair(AgentPair::default())?;
            r.expect_rejection(
                EasyActor::Bank,
                EasyAction::Chargeback,
                EasyFault::ChargebackBlocked,
            )?;
            let passed = r.blocked == 1 && r.node.audit().is_empty();
            Ok(r.into_score("irreversible-rail", passed, false))
        }
        _ => Err(format!(
            "unknown Bisq Easy scenario '{name}'. Choices: {}",
            BISQ_EASY_SCENARIOS.join(", ")
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_default_bisq_easy_scenarios_meet_their_oracles() {
        for scenario in BISQ_EASY_SCENARIOS {
            let result = run_bisq_easy(scenario).unwrap_or_else(|error| panic!("{scenario}: {error}"));
            assert!(result.passed, "scenario failed: {scenario}");
        }
    }

    #[test]
    fn only_negative_controls_create_unprotected_losses() {
        for scenario in BISQ_EASY_SCENARIOS {
            let result = run_bisq_easy(scenario).unwrap();
            if matches!(*scenario, "seller-withholds" | "chargeback-after-delivery") {
                assert!(result.attack_success);
                assert!(!result.safety_findings.is_empty());
            } else {
                assert!(!result.attack_success);
                assert!(result.safety_findings.is_empty(), "{scenario}");
            }
        }
    }

    #[test]
    fn default_baseline_really_moves_fiat_before_bitcoin() {
        let mut runner = Runner::new(EasyNode::default());
        runner.accept().unwrap();
        runner.payment().unwrap();
        assert_eq!(runner.node.state.buyer_btc_sats, 0);
        assert_eq!(runner.node.state.seller_fiat_cents, 5_000);
        runner.payout().unwrap();
        assert_eq!(runner.node.state.buyer_btc_sats, 100_000);
    }

    #[test]
    fn bisq_easy_global_limit_is_600_dollars() {
        let mut offer = Offer::default();
        offer.quote_usd_cents = BISQ_EASY_MAX_USD_CENTS + 100;
        offer.btc_sats = 1_202_000;
        offer.seller_initial_btc_sats = 1_202_300;
        offer.seller_reputation_points = 300_000;
        assert_eq!(
            offer.verify(Oracle::default(), SIMULATED_NOW, offer.fixed_price_usd_cents_per_btc),
            Err(EasyFault::InvalidOfferAmount)
        );
    }

    #[test]
    fn json_lines_include_protocol_and_exposure() {
        let result = run_bisq_easy("seller-withholds").unwrap();
        let line = result.json_line();
        assert!(line.contains(r#""protocol":"bisq-easy""#));
        assert!(line.contains(r#""buyer_usd_at_risk_cents":5000"#));
        assert!(line.contains("seller_non_delivery"));
    }
}
