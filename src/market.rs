//! Bisq Easy-inspired, deterministic P2P marketplace model.
//!
//! This is independent MIT-licensed modeling code, not Bisq's implementation.
//! Only USD fixed-price offers and abstracted payment data are supported.
//! Fiat movement is an external event, never inferred from a chat message.

use std::collections::BTreeSet;

pub const SATS_PER_BTC: u128 = 100_000_000;
pub const SIMULATED_NOW: u64 = 60_000;
pub const ORACLE_MAX_AGE_SECONDS: u64 = 43_200;
pub const ORACLE_MAX_SKEW_SECONDS: u64 = 720;
pub const BISQ_EASY_MIN_USD_CENTS: u64 = 600;
pub const BISQ_EASY_MAX_USD_CENTS: u64 = 60_000;
pub const REPUTATION_POINTS_PER_USD: u64 = 200;
pub const MIN_SELL_OFFER_REPUTATION: u64 = 1_200;
pub const SELLER_DELIVERY_DEADLINE_ROUNDS: u64 = 3;
pub const SIMULATED_CHAIN_FEE_SATS: u64 = 300;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ProtocolProfile {
    #[default]
    BisqEasy,
    BisqMuSigModel,
}

impl ProtocolProfile {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "bisq" | "bisq-easy" => Some(Self::BisqEasy),
            "bisq-musig" | "musig" => Some(Self::BisqMuSigModel),
            _ => None,
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Self::BisqEasy => "bisq-easy",
            Self::BisqMuSigModel => "bisq-musig-model",
        }
    }

    pub fn default_scenario(self) -> &'static str {
        match self {
            Self::BisqEasy => "honest-trade",
            Self::BisqMuSigModel => "baseline",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rail {
    /// An intentionally simplified non-reversible simulated fiat rail.
    SimulatedIrreversible,
    /// Adversarial rail used to test post-delivery fiat reversal exposure.
    SimulatedReversible,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Offer {
    pub quote_usd_cents: u64,
    pub btc_sats: u64,
    pub fixed_price_usd_cents_per_btc: u64,
    pub seller_reputation_points: u64,
    pub seller_initial_btc_sats: u64,
    pub rail: Rail,
}

impl Default for Offer {
    fn default() -> Self {
        Self {
            quote_usd_cents: 5_000, // $50
            btc_sats: 100_000, // 0.001 BTC
            fixed_price_usd_cents_per_btc: 5_000_000, // $50,000 / BTC
            seller_reputation_points: 10_000,
            seller_initial_btc_sats: 200_300,
            rail: Rail::SimulatedIrreversible,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Oracle {
    pub btc_usd_cents: u64,
    pub market_observed_at: u64,
    pub usd_observed_at: u64,
}

impl Default for Oracle {
    fn default() -> Self {
        Self {
            btc_usd_cents: 5_000_000,
            market_observed_at: SIMULATED_NOW,
            usd_observed_at: SIMULATED_NOW,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EasyFault {
    Unauthorized,
    InvalidTransition,
    ContractPriceMismatch,
    InvalidOfferAmount,
    LowSellerReputation,
    StaleMarketPrice,
    OracleTimestampSkew,
    MissingMarketPrice,
    FiatNotReceived,
    InsufficientBalance,
    UnapprovedBitcoinRequest,
    PaymentRequestAmountMismatch,
    ChargebackBlocked,
    ConflictingReplay,
    ArithmeticOverflow,
    ValueNotConserved,
    JournalMismatch,
    SellerNonDelivery,
    PostDeliveryChargeback,
}

impl EasyFault {
    pub fn code(self) -> &'static str {
        match self {
            Self::Unauthorized => "unauthorized",
            Self::InvalidTransition => "invalid_transition",
            Self::ContractPriceMismatch => "contract_price_mismatch",
            Self::InvalidOfferAmount => "invalid_offer_amount",
            Self::LowSellerReputation => "seller_reputation_too_low",
            Self::StaleMarketPrice => "stale_market_price",
            Self::OracleTimestampSkew => "oracle_timestamp_skew",
            Self::MissingMarketPrice => "missing_market_price",
            Self::FiatNotReceived => "fiat_not_received",
            Self::InsufficientBalance => "insufficient_balance",
            Self::UnapprovedBitcoinRequest => "unapproved_bitcoin_request",
            Self::PaymentRequestAmountMismatch => "payment_request_amount_mismatch",
            Self::ChargebackBlocked => "chargeback_blocked",
            Self::ConflictingReplay => "conflicting_replay",
            Self::ArithmeticOverflow => "arithmetic_overflow",
            Self::ValueNotConserved => "value_not_conserved",
            Self::JournalMismatch => "journal_mismatch",
            Self::SellerNonDelivery => "seller_non_delivery",
            Self::PostDeliveryChargeback => "post_delivery_chargeback",
        }
    }
}

impl Oracle {
    pub fn verify(self, now: u64) -> Result<(), EasyFault> {
        if self.btc_usd_cents == 0 {
            return Err(EasyFault::MissingMarketPrice);
        }
        if self.market_observed_at > now || self.usd_observed_at > now {
            return Err(EasyFault::StaleMarketPrice);
        }
        let newest_age = now - self.market_observed_at;
        let usd_age = now - self.usd_observed_at;
        if newest_age >= ORACLE_MAX_AGE_SECONDS || usd_age >= ORACLE_MAX_AGE_SECONDS {
            return Err(EasyFault::StaleMarketPrice);
        }
        if self.market_observed_at.abs_diff(self.usd_observed_at) > ORACLE_MAX_SKEW_SECONDS {
            return Err(EasyFault::OracleTimestampSkew);
        }
        Ok(())
    }
}

impl Offer {
    /// Checks a fixed USD-price offer and the independent market oracle.
    /// For USD, market USD value of a quoted fiat amount is that amount.
    /// This does not implement Bisq's cross-currency price conversion.
    pub fn verify(self, oracle: Oracle, now: u64, contract_price: u64) -> Result<(), EasyFault> {
        if contract_price != self.fixed_price_usd_cents_per_btc {
            return Err(EasyFault::ContractPriceMismatch);
        }
        oracle.verify(now)?;
        if !(BISQ_EASY_MIN_USD_CENTS..=BISQ_EASY_MAX_USD_CENTS)
            .contains(&self.quote_usd_cents)
            || self.fixed_price_usd_cents_per_btc == 0
            || self.btc_sats == 0
        {
            return Err(EasyFault::InvalidOfferAmount);
        }
        let expected = u128::from(self.btc_sats)
            .checked_mul(u128::from(self.fixed_price_usd_cents_per_btc))
            .and_then(|v| v.checked_add(SATS_PER_BTC / 2))
            .ok_or(EasyFault::ArithmeticOverflow)?
            / SATS_PER_BTC;
        if expected != u128::from(self.quote_usd_cents) {
            return Err(EasyFault::InvalidOfferAmount);
        }
        if self.seller_initial_btc_sats
            < self
                .btc_sats
                .checked_add(SIMULATED_CHAIN_FEE_SATS)
                .ok_or(EasyFault::ArithmeticOverflow)?
        {
            return Err(EasyFault::InsufficientBalance);
        }
        if self.seller_reputation_points < MIN_SELL_OFFER_REPUTATION {
            return Err(EasyFault::LowSellerReputation);
        }
        // Bisq Easy reputational score: 200 points per rounded USD.
        // Tolerance is max(round(score * 1.05), score + 200).
        let rounded_usd = u128::from(self.quote_usd_cents)
            .checked_add(50)
            .ok_or(EasyFault::ArithmeticOverflow)?
            / 100;
        let required = rounded_usd
            .checked_mul(u128::from(REPUTATION_POINTS_PER_USD))
            .ok_or(EasyFault::ArithmeticOverflow)?;
        let score = u128::from(self.seller_reputation_points);
        let tolerance = ((score * 105 + 50) / 100).max(score + 200);
        if required > tolerance {
            return Err(EasyFault::LowSellerReputation);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EasyActor {
    Buyer,
    Seller,
    Bank,
    Clock,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EasyAction {
    TakeOffer { contract_price: u64 },
    TransferFiat,
    ClaimFiatSent,
    ConfirmFiat,
    SetBitcoinPaymentRequest { requested_sats: Option<u64> },
    SendBitcoin,
    OpenDispute,
    Chargeback,
    Tick,
}

impl EasyAction {
    pub fn authorized(self, actor: EasyActor) -> bool {
        matches!(
            (self, actor),
            (Self::TakeOffer { .. }, EasyActor::Buyer)
                | (Self::TransferFiat, EasyActor::Bank)
                | (Self::ClaimFiatSent, EasyActor::Buyer)
                | (Self::ConfirmFiat, EasyActor::Seller)
                | (Self::SetBitcoinPaymentRequest { .. }, EasyActor::Buyer)
                | (Self::SendBitcoin, EasyActor::Seller)
                | (Self::OpenDispute, EasyActor::Buyer)
                | (Self::Chargeback, EasyActor::Bank)
                | (Self::Tick, EasyActor::Clock)
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EasyEvent {
    pub id: u64,
    pub actor: EasyActor,
    pub action: EasyAction,
}

impl EasyEvent {
    pub fn new(id: u64, actor: EasyActor, action: EasyAction) -> Self {
        Self { id, actor, action }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EasyPhase {
    Available,
    Taken,
    FiatClaimed,
    FiatConfirmed,
    Completed,
    Disputed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EasyState {
    pub phase: EasyPhase,
    pub buyer_fiat_cents: u64,
    pub seller_fiat_cents: u64,
    pub buyer_btc_sats: u64,
    pub seller_btc_sats: u64,
    pub miner_btc_sats: u64,
    pub fiat_transferred: bool,
    pub chargeback_done: bool,
    pub bitcoin_request_approved: bool,
    pub btc_delivered: bool,
    pub round: u64,
}

impl EasyState {
    pub fn new(offer: Offer) -> Self {
        Self {
            phase: EasyPhase::Available,
            buyer_fiat_cents: offer.quote_usd_cents,
            seller_fiat_cents: 0,
            buyer_btc_sats: 0,
            seller_btc_sats: offer.seller_initial_btc_sats,
            miner_btc_sats: 0,
            fiat_transferred: false,
            chargeback_done: false,
            bitcoin_request_approved: false,
            btc_delivered: false,
            round: 0,
        }
    }

    fn check(&self, offer: Offer) -> Result<(), EasyFault> {
        let fiat_total = u128::from(self.buyer_fiat_cents) + u128::from(self.seller_fiat_cents);
        let btc_total = u128::from(self.buyer_btc_sats)
            + u128::from(self.seller_btc_sats)
            + u128::from(self.miner_btc_sats);
        if fiat_total != u128::from(offer.quote_usd_cents)
            || btc_total != u128::from(offer.seller_initial_btc_sats)
        {
            return Err(EasyFault::ValueNotConserved);
        }
        if self.btc_delivered != (self.phase == EasyPhase::Completed)
            || self.fiat_transferred && self.chargeback_done
        {
            return Err(EasyFault::JournalMismatch);
        }
        Ok(())
    }

    pub fn transition(
        &self,
        offer: Offer,
        oracle: Oracle,
        now: u64,
        event: EasyEvent,
    ) -> Result<Self, EasyFault> {
        if !event.action.authorized(event.actor) {
            return Err(EasyFault::Unauthorized);
        }
        let mut next = self.clone();
        match event.action {
            EasyAction::TakeOffer { contract_price } if self.phase == EasyPhase::Available => {
                offer.verify(oracle, now, contract_price)?;
                next.phase = EasyPhase::Taken;
            }
            EasyAction::TransferFiat
                if matches!(self.phase, EasyPhase::Taken | EasyPhase::FiatClaimed)
                    && !self.fiat_transferred
                    && !self.chargeback_done =>
            {
                next.buyer_fiat_cents = next
                    .buyer_fiat_cents
                    .checked_sub(offer.quote_usd_cents)
                    .ok_or(EasyFault::InsufficientBalance)?;
                next.seller_fiat_cents = next
                    .seller_fiat_cents
                    .checked_add(offer.quote_usd_cents)
                    .ok_or(EasyFault::ArithmeticOverflow)?;
                next.fiat_transferred = true;
            }
            EasyAction::ClaimFiatSent if self.phase == EasyPhase::Taken => {
                next.phase = EasyPhase::FiatClaimed;
            }
            EasyAction::ConfirmFiat if self.phase == EasyPhase::FiatClaimed => {
                if !self.fiat_transferred {
                    return Err(EasyFault::FiatNotReceived);
                }
                next.phase = EasyPhase::FiatConfirmed;
            }
            EasyAction::SetBitcoinPaymentRequest { requested_sats }
                if matches!(
                    self.phase,
                    EasyPhase::Taken | EasyPhase::FiatClaimed | EasyPhase::FiatConfirmed
                ) =>
            {
                if requested_sats.is_some_and(|amount| amount != offer.btc_sats) {
                    return Err(EasyFault::PaymentRequestAmountMismatch);
                }
                next.bitcoin_request_approved = true;
            }
            EasyAction::SendBitcoin if self.phase == EasyPhase::FiatConfirmed => {
                if !self.bitcoin_request_approved {
                    return Err(EasyFault::UnapprovedBitcoinRequest);
                }
                let debit = offer
                    .btc_sats
                    .checked_add(SIMULATED_CHAIN_FEE_SATS)
                    .ok_or(EasyFault::ArithmeticOverflow)?;
                next.seller_btc_sats = next
                    .seller_btc_sats
                    .checked_sub(debit)
                    .ok_or(EasyFault::InsufficientBalance)?;
                next.buyer_btc_sats = next
                    .buyer_btc_sats
                    .checked_add(offer.btc_sats)
                    .ok_or(EasyFault::ArithmeticOverflow)?;
                next.miner_btc_sats = next
                    .miner_btc_sats
                    .checked_add(SIMULATED_CHAIN_FEE_SATS)
                    .ok_or(EasyFault::ArithmeticOverflow)?;
                next.phase = EasyPhase::Completed;
                next.btc_delivered = true;
            }
            EasyAction::OpenDispute
                if matches!(
                    self.phase,
                    EasyPhase::FiatClaimed | EasyPhase::FiatConfirmed
                ) && self.fiat_transferred
                    && self.round > SELLER_DELIVERY_DEADLINE_ROUNDS =>
            {
                next.phase = EasyPhase::Disputed;
            }
            EasyAction::Chargeback if self.fiat_transferred => {
                if offer.rail != Rail::SimulatedReversible {
                    return Err(EasyFault::ChargebackBlocked);
                }
                next.seller_fiat_cents = next
                    .seller_fiat_cents
                    .checked_sub(offer.quote_usd_cents)
                    .ok_or(EasyFault::InsufficientBalance)?;
                next.buyer_fiat_cents = next
                    .buyer_fiat_cents
                    .checked_add(offer.quote_usd_cents)
                    .ok_or(EasyFault::ArithmeticOverflow)?;
                next.fiat_transferred = false;
                next.chargeback_done = true;
            }
            EasyAction::Tick => {
                next.round = next
                    .round
                    .checked_add(1)
                    .ok_or(EasyFault::ArithmeticOverflow)?;
            }
            _ => return Err(EasyFault::InvalidTransition),
        }
        next.check(offer)?;
        Ok(next)
    }
}

#[derive(Clone, Debug)]
pub struct EasyNode {
    pub offer: Offer,
    pub oracle: Oracle,
    pub now: u64,
    pub state: EasyState,
    pub journal: Vec<EasyEvent>,
}

impl Default for EasyNode {
    fn default() -> Self {
        Self::new(Offer::default(), Oracle::default(), SIMULATED_NOW)
    }
}

impl EasyNode {
    pub fn new(offer: Offer, oracle: Oracle, now: u64) -> Self {
        Self {
            offer,
            oracle,
            now,
            state: EasyState::new(offer),
            journal: Vec::new(),
        }
    }

    /// Returns true when a new event commits; false for an identical replay.
    pub fn receive(&mut self, event: EasyEvent) -> Result<bool, EasyFault> {
        if let Some(existing) = self.journal.iter().find(|prior| prior.id == event.id) {
            return if *existing == event {
                Ok(false)
            } else {
                Err(EasyFault::ConflictingReplay)
            };
        }
        let candidate = self.state.transition(self.offer, self.oracle, self.now, event)?;
        self.state = candidate;
        self.journal.push(event);
        Ok(true)
    }

    pub fn replay(&self) -> Result<EasyState, EasyFault> {
        let mut rebuilt = EasyState::new(self.offer);
        let mut seen = BTreeSet::new();
        for event in &self.journal {
            if !seen.insert(event.id) {
                return Err(EasyFault::JournalMismatch);
            }
            rebuilt = rebuilt.transition(self.offer, self.oracle, self.now, *event)?;
        }
        Ok(rebuilt)
    }

    pub fn audit(&self) -> Vec<EasyFault> {
        let mut findings = Vec::new();
        if self.replay() != Ok(self.state.clone()) {
            findings.push(EasyFault::JournalMismatch);
        }
        if let Err(fault) = self.state.check(self.offer) {
            findings.push(fault);
        }
        if self.state.fiat_transferred
            && !self.state.btc_delivered
            && self.state.round > SELLER_DELIVERY_DEADLINE_ROUNDS
        {
            findings.push(EasyFault::SellerNonDelivery);
        }
        if self.state.chargeback_done && self.state.btc_delivered {
            findings.push(EasyFault::PostDeliveryChargeback);
        }
        findings.sort_by_key(|fault| fault.code());
        findings.dedup();
        findings
    }

    pub fn buyer_usd_at_risk_cents(&self) -> u64 {
        if self.audit().contains(&EasyFault::SellerNonDelivery) {
            self.offer.quote_usd_cents
        } else {
            0
        }
    }

    pub fn seller_btc_at_risk_sats(&self) -> u64 {
        if self.audit().contains(&EasyFault::PostDeliveryChargeback) {
            self.offer.btc_sats + SIMULATED_CHAIN_FEE_SATS
        } else {
            0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(id: u64, actor: EasyActor, action: EasyAction) -> EasyEvent {
        EasyEvent::new(id, actor, action)
    }

    #[test]
    fn default_is_bisq_easy() {
        assert_eq!(ProtocolProfile::default(), ProtocolProfile::BisqEasy);
        assert_eq!(ProtocolProfile::parse("bisq"), Some(ProtocolProfile::BisqEasy));
        assert_eq!(ProtocolProfile::parse("bisq-musig"), Some(ProtocolProfile::BisqMuSigModel));
    }

    #[test]
    fn offer_validates_reputation_tolerance() {
        let oracle = Oracle::default();
        let offer = Offer::default();
        assert!(offer.verify(oracle, SIMULATED_NOW, offer.fixed_price_usd_cents_per_btc).is_ok());
        let min_required = Offer {
            seller_reputation_points: 9_500,
            ..offer
        };
        // 9,500 + 5% allowance = 9,975, which is short of 10,000.
        assert_eq!(
            min_required.verify(oracle, SIMULATED_NOW, offer.fixed_price_usd_cents_per_btc),
            Err(EasyFault::LowSellerReputation)
        );
        let tolerated = Offer {
            seller_reputation_points: 9_600,
            ..offer
        };
        assert!(tolerated.verify(oracle, SIMULATED_NOW, offer.fixed_price_usd_cents_per_btc).is_ok());
    }

    #[test]
    fn oracle_rejects_age_and_skew() {
        let now = SIMULATED_NOW;
        assert_eq!(
            Oracle { market_observed_at: now - ORACLE_MAX_AGE_SECONDS, ..Oracle::default() }.verify(now),
            Err(EasyFault::StaleMarketPrice)
        );
        assert_eq!(
            Oracle { usd_observed_at: now - ORACLE_MAX_SKEW_SECONDS - 1, ..Oracle::default() }.verify(now),
            Err(EasyFault::OracleTimestampSkew)
        );
    }

    #[test]
    fn forged_contract_price_is_rejected_before_acceptance() {
        let mut node = EasyNode::default();
        let result = node.receive(event(
            1,
            EasyActor::Buyer,
            EasyAction::TakeOffer { contract_price: 3_000_000 },
        ));
        assert_eq!(result, Err(EasyFault::ContractPriceMismatch));
        assert!(node.journal.is_empty());
        assert_eq!(node.state.phase, EasyPhase::Available);
    }

    #[test]
    fn fifo_payment_first_and_conservation() {
        let mut node = EasyNode::default();
        let actions = [
            (EasyActor::Buyer, EasyAction::TakeOffer { contract_price: 5_000_000 }),
            (EasyActor::Bank, EasyAction::TransferFiat),
            (EasyActor::Buyer, EasyAction::ClaimFiatSent),
            (EasyActor::Seller, EasyAction::ConfirmFiat),
            (EasyActor::Buyer, EasyAction::SetBitcoinPaymentRequest { requested_sats: Some(100_000) }),
            (EasyActor::Seller, EasyAction::SendBitcoin),
        ];
        for (i, (actor, action)) in actions.iter().enumerate() {
            node.receive(event(i as u64 + 1, *actor, *action)).unwrap();
        }
        assert_eq!(node.state.phase, EasyPhase::Completed);
        assert_eq!(node.state.buyer_btc_sats, 100_000);
        assert_eq!(node.state.seller_fiat_cents, 5_000);
        assert_eq!(node.state.miner_btc_sats, SIMULATED_CHAIN_FEE_SATS);
        assert!(node.audit().is_empty());
    }

    #[test]
    fn same_id_altered_payload_is_rejected() {
        let mut node = EasyNode::default();
        let first = event(1, EasyActor::Buyer, EasyAction::TakeOffer { contract_price: 5_000_000 });
        assert_eq!(node.receive(first), Ok(true));
        assert_eq!(node.receive(first), Ok(false));
        assert_eq!(
            node.receive(event(1, EasyActor::Bank, EasyAction::TransferFiat)),
            Err(EasyFault::ConflictingReplay)
        );
        assert_eq!(node.journal.len(), 1);
    }

    #[test]
    fn fake_fiat_claim_cannot_authorize_btc_release() {
        let mut node = EasyNode::default();
        node.receive(event(1, EasyActor::Buyer, EasyAction::TakeOffer { contract_price: 5_000_000 })).unwrap();
        node.receive(event(2, EasyActor::Buyer, EasyAction::ClaimFiatSent)).unwrap();
        assert_eq!(
            node.receive(event(3, EasyActor::Seller, EasyAction::ConfirmFiat)),
            Err(EasyFault::FiatNotReceived)
        );
        assert_eq!(node.state.buyer_fiat_cents, 5_000);
    }

    #[test]
    fn overlarge_invoice_is_rejected_before_sending_btc() {
        let mut node = EasyNode::default();
        node.receive(event(1, EasyActor::Buyer, EasyAction::TakeOffer { contract_price: 5_000_000 })).unwrap();
        assert_eq!(
            node.receive(event(2, EasyActor::Buyer, EasyAction::SetBitcoinPaymentRequest { requested_sats: Some(1_000_000) })),
            Err(EasyFault::PaymentRequestAmountMismatch)
        );
        assert!(!node.state.bitcoin_request_approved);
    }

    #[test]
    fn cannot_release_btc_before_real_fiat_receipt() {
        let mut node = EasyNode::default();
        node.receive(event(1, EasyActor::Buyer, EasyAction::TakeOffer { contract_price: 5_000_000 })).unwrap();
        assert_eq!(
            node.receive(event(2, EasyActor::Seller, EasyAction::SendBitcoin)),
            Err(EasyFault::InvalidTransition)
        );
        assert_eq!(node.state.buyer_btc_sats, 0);
    }
}
