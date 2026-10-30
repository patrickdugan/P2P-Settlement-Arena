//! `cashout-bisq-easy`: the cashout direction on the reused Bisq Easy machine.
//!
//! Seller = sender S, buyer = provider P, and the fiat leg pays a third-party
//! payee, the recipient R. P pays R first; S confirms only on R's
//! acknowledgment or rail evidence, because S cannot see R's wallet, and only
//! then sends BTC. P bears S's non-delivery, exactly as the Bisq Easy buyer
//! bears the seller's.
//!
//! The wrapper adds what the cashout direction needs and Bisq Easy lacks: P's
//! payout ledger (one rail attempt, timeouts), R's acknowledgment and rail
//! evidence as S's only view of the payee, and P judging S's BTC output from
//! its own chain view. Every money movement still goes through
//! `EasyState::transition`. In the reused state, the seller-side fiat balance
//! is the payee's balance: S never receives fiat in this profile.

use crate::cashout::{
    sort_findings, CashoutAction, CashoutActor, CashoutEvent, CashoutFault, CashoutMachine,
    CashoutProfile, CashoutQuote, CashoutScore, ChainOutput, ChainView, Currency, Exposure,
    PayoutLedger, Runner, DEPOSIT_SCRIPT, FUNDING_OUTPOINT, OLD_OUTPOINT, PAYOUT_KEY, RETRY_KEY,
    SENDER_OUTPOINT,
};
use crate::market::{
    EasyAction, EasyActor, EasyEvent, EasyFault, EasyNode, EasyPhase, EasyState, Offer, Oracle,
    Rail, BISQ_EASY_MAX_USD_CENTS, SATS_PER_BTC, SELLER_DELIVERY_DEADLINE_ROUNDS, SIMULATED_NOW,
};

/// Bisq Easy gates the trade on the seller's reputation, which here is S's.
/// This covers the $600 cap (200 points per USD). BAN's senders sign with
/// ephemeral keys and have no reputation, so this is an assumption of the
/// profile, not a property of the cashout design.
pub const CASHOUT_SENDER_REPUTATION_POINTS: u64 = BISQ_EASY_MAX_USD_CENTS / 100 * 200;

/// Cashout actors on the reused machine. The provider is the Easy buyer, never
/// the seller; the recipient has no Easy role (it is the payee of the fiat leg).
pub const fn easy_actor(actor: CashoutActor) -> Option<EasyActor> {
    match actor {
        CashoutActor::Provider => Some(EasyActor::Buyer),
        CashoutActor::Sender => Some(EasyActor::Seller),
        CashoutActor::Rail => Some(EasyActor::Bank),
        CashoutActor::Clock => Some(EasyActor::Clock),
        CashoutActor::Recipient => None,
    }
}

const _: () = assert!(matches!(
    easy_actor(CashoutActor::Provider),
    Some(EasyActor::Buyer)
));

/// The Bisq Easy event a cashout action drives, if any.
pub fn easy_action(action: CashoutAction, offer: &Offer) -> Option<EasyAction> {
    match action {
        CashoutAction::TakeQuote => Some(EasyAction::TakeOffer {
            contract_price: offer.fixed_price_usd_cents_per_btc,
        }),
        CashoutAction::RailCredit { .. } => Some(EasyAction::TransferFiat),
        CashoutAction::ClaimPaid => Some(EasyAction::ClaimFiatSent),
        CashoutAction::ConfirmPayout => Some(EasyAction::ConfirmFiat),
        CashoutAction::PaymentRequest => Some(EasyAction::SetBitcoinPaymentRequest {
            requested_sats: Some(offer.btc_sats),
        }),
        CashoutAction::SendBitcoin { .. } => Some(EasyAction::SendBitcoin),
        CashoutAction::Tick => Some(EasyAction::Tick),
        _ => None,
    }
}

/// The Bisq Easy offer for a quote. The reused machine is USD-only and checks
/// the $6-$600 range, the fixed price and S's reputation.
pub fn easy_offer(quote: &CashoutQuote) -> Result<Offer, CashoutFault> {
    if quote.currency != Currency::USD || quote.exponent != 2 {
        return Err(CashoutFault::UnsupportedCurrency);
    }
    // The fixed price that reproduces the quote under the machine's rounding.
    let price = u128::from(quote.net_payout_minor)
        .checked_mul(SATS_PER_BTC)
        .and_then(|v| v.checked_add(u128::from(quote.amount_sats) / 2))
        .ok_or(CashoutFault::ArithmeticOverflow)?
        / u128::from(quote.amount_sats);
    let offer = Offer {
        quote_usd_cents: quote.net_payout_minor,
        btc_sats: quote.amount_sats,
        fixed_price_usd_cents_per_btc: u64::try_from(price)
            .map_err(|_| CashoutFault::ArithmeticOverflow)?,
        seller_reputation_points: CASHOUT_SENDER_REPUTATION_POINTS,
        seller_initial_btc_sats: quote.sender_initial_sats,
        rail: Rail::SimulatedIrreversible,
    };
    offer
        .verify(
            Oracle::default(),
            SIMULATED_NOW,
            offer.fixed_price_usd_cents_per_btc,
        )
        .map_err(CashoutFault::Easy)?;
    Ok(offer)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EasyTerms {
    pub quote: CashoutQuote,
    pub offer: Offer,
    pub oracle: Oracle,
    pub now: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EasyCashoutState {
    /// The reused Bisq Easy state: buyer = P, seller = S, seller fiat = R's credit.
    pub easy: EasyState,
    pub chain: ChainView,
    pub payout: PayoutLedger,
    pub recipient_acked: bool,
    pub rail_evidence: bool,
    pub btc_accepted: bool,
    /// Largest fiat P had paid R while S's BTC was overdue.
    pub provider_exposed_minor: u64,
}

impl EasyCashoutState {
    pub fn new(terms: &EasyTerms) -> Self {
        Self {
            easy: EasyState::new(terms.offer),
            chain: ChainView::fixture(&terms.quote),
            payout: PayoutLedger::default(),
            recipient_acked: false,
            rail_evidence: false,
            btc_accepted: false,
            provider_exposed_minor: 0,
        }
    }

    fn height(&self, quote: &CashoutQuote) -> Result<u64, CashoutFault> {
        quote
            .created_height
            .checked_add(self.easy.round)
            .ok_or(CashoutFault::ArithmeticOverflow)
    }

    /// P's fiat at R while S's BTC is past the Bisq Easy delivery deadline.
    fn unsecured_past_deadline(&self, offer: &Offer) -> u64 {
        let easy = &self.easy;
        if easy.fiat_transferred
            && !easy.btc_delivered
            && easy.round > SELLER_DELIVERY_DEADLINE_ROUNDS
        {
            offer.quote_usd_cents
        } else {
            0
        }
    }

    pub fn transition(&self, terms: &EasyTerms, event: CashoutEvent) -> Result<Self, CashoutFault> {
        if !event.action.authorized(event.actor) {
            return Err(CashoutFault::Unauthorized);
        }
        let mut next = self.clone();
        match event.action {
            CashoutAction::SubmitPayout { key } => {
                if self.easy.phase == EasyPhase::Available {
                    return Err(CashoutFault::InvalidTransition);
                }
                next.payout.authorize(key)?;
            }
            CashoutAction::RailCredit { key } => next.payout.credit(key)?,
            CashoutAction::RailTimeout { key } => next.payout.timeout(key)?,
            CashoutAction::RailStatus { key } => {
                next.payout.status(key)?;
            }
            CashoutAction::RecipientAck => {
                // An honest R acknowledges only a credit it received. A colluding R
                // is out of scope: acknowledgments are gameable (spec G.10).
                if !self.easy.fiat_transferred {
                    return Err(CashoutFault::Easy(EasyFault::FiatNotReceived));
                }
                next.recipient_acked = true;
            }
            CashoutAction::RailEvidence => {
                if !self.payout.rail_credited {
                    return Err(CashoutFault::Easy(EasyFault::FiatNotReceived));
                }
                next.rail_evidence = true;
            }
            CashoutAction::ConfirmPayout => {
                // S cannot see R's wallet: P's claim is never enough.
                if !self.recipient_acked && !self.rail_evidence {
                    return Err(CashoutFault::NoPayeeEvidence);
                }
            }
            CashoutAction::SendBitcoin { outpoint } => next.chain.add(ChainOutput {
                outpoint,
                script: DEPOSIT_SCRIPT,
                value_sats: terms.offer.btc_sats,
                height: self.height(&terms.quote)?,
            })?,
            CashoutAction::AcceptBitcoin { outpoint } => {
                if self.btc_accepted {
                    return Err(CashoutFault::InvalidTransition);
                }
                // P already paid, so owed BTC is accepted late; lateness is scored.
                let output =
                    self.chain
                        .judge(outpoint, &terms.quote, self.height(&terms.quote)?, false)?;
                next.chain.used_outpoints.insert(output.outpoint);
                next.btc_accepted = true;
            }
            // P never holds S's BTC unpaid in this order, so it has nothing to refund.
            CashoutAction::Refund => return Err(CashoutFault::InvalidTransition),
            CashoutAction::TakeQuote
            | CashoutAction::ClaimPaid
            | CashoutAction::PaymentRequest
            | CashoutAction::Tick => {}
        }
        if let Some(action) = easy_action(event.action, &terms.offer) {
            let actor = easy_actor(event.actor).ok_or(CashoutFault::Unauthorized)?;
            next.easy = next
                .easy
                .transition(
                    terms.offer,
                    terms.oracle,
                    terms.now,
                    EasyEvent::new(event.id, actor, action),
                )
                .map_err(CashoutFault::Easy)?;
        }
        next.provider_exposed_minor = next
            .provider_exposed_minor
            .max(next.unsecured_past_deadline(&terms.offer));
        Ok(next)
    }
}

#[derive(Clone, Debug)]
pub struct EasyCashoutNode {
    pub terms: EasyTerms,
    pub state: EasyCashoutState,
    pub journal: Vec<CashoutEvent>,
}

impl EasyCashoutNode {
    pub fn new(quote: CashoutQuote) -> Result<Self, CashoutFault> {
        quote.verify()?;
        let terms = EasyTerms {
            quote,
            offer: easy_offer(&quote)?,
            oracle: Oracle::default(),
            now: SIMULATED_NOW,
        };
        Ok(Self {
            terms,
            state: EasyCashoutState::new(&terms),
            journal: Vec::new(),
        })
    }

    /// Returns true when a new event commits; false for an identical replay.
    pub fn receive(&mut self, event: CashoutEvent) -> Result<bool, CashoutFault> {
        if let Some(result) = crate::cashout::replayed(&self.journal, &event) {
            return result;
        }
        self.state = self.state.transition(&self.terms, event)?;
        self.journal.push(event);
        Ok(true)
    }

    pub fn replay(&self) -> Result<EasyCashoutState, CashoutFault> {
        let mut rebuilt = EasyCashoutState::new(&self.terms);
        for event in &self.journal {
            rebuilt = rebuilt.transition(&self.terms, *event)?;
        }
        Ok(rebuilt)
    }

    /// The reused Bisq Easy node: the same offer and state, and the Easy events
    /// this node committed, so its own audit and exposure rules apply unchanged.
    pub fn easy_node(&self) -> EasyNode {
        let journal = self
            .journal
            .iter()
            .filter_map(|e| {
                let action = easy_action(e.action, &self.terms.offer)?;
                Some(EasyEvent::new(e.id, easy_actor(e.actor)?, action))
            })
            .collect();
        EasyNode {
            offer: self.terms.offer,
            oracle: self.terms.oracle,
            now: self.terms.now,
            state: self.state.easy.clone(),
            journal,
        }
    }

    pub fn audit(&self) -> Vec<CashoutFault> {
        let mut findings: Vec<CashoutFault> = self
            .easy_node()
            .audit()
            .into_iter()
            .map(CashoutFault::Easy)
            .collect();
        if self.replay().as_ref() != Ok(&self.state) {
            findings.push(CashoutFault::JournalMismatch);
        }
        let withheld = CashoutFault::Easy(EasyFault::SellerNonDelivery);
        if self.state.provider_exposed_minor > 0 && !findings.contains(&withheld) {
            findings.push(CashoutFault::SellerLateDelivery);
        }
        sort_findings(&mut findings);
        findings
    }
}

impl CashoutMachine for EasyCashoutNode {
    fn submit(&mut self, event: CashoutEvent) -> Result<bool, CashoutFault> {
        self.receive(event)
    }

    fn quote(&self) -> &CashoutQuote {
        &self.terms.quote
    }

    fn committed(&self) -> usize {
        self.journal.len()
    }

    fn phase_name(&self) -> String {
        format!("{:?}", self.state.easy.phase)
    }

    fn findings(&self) -> Vec<CashoutFault> {
        self.audit()
    }

    fn exposure(&self) -> Exposure {
        Exposure {
            // Bisq Easy's chargeback exposure; the cashout rail is irreversible.
            sender_btc_at_risk_sats: self.easy_node().seller_btc_at_risk_sats(),
            provider_minor_at_risk: self.state.provider_exposed_minor,
            recipient_credited_minor: self.state.easy.seller_fiat_cents,
        }
    }

    fn settled(&self) -> bool {
        self.state.easy.phase == EasyPhase::Completed
            && self.state.btc_accepted
            && self.state.easy.seller_fiat_cents == self.terms.quote.net_payout_minor
            && self.audit().is_empty()
    }
}

impl Runner<EasyCashoutNode> {
    fn take(&mut self) -> Result<(), String> {
        self.send(CashoutActor::Provider, CashoutAction::TakeQuote)
    }

    /// P authorizes once, the rail credits R, and P tells S it paid.
    fn pay(&mut self) -> Result<(), String> {
        self.send(
            CashoutActor::Provider,
            CashoutAction::SubmitPayout { key: PAYOUT_KEY },
        )?;
        self.send(
            CashoutActor::Rail,
            CashoutAction::RailCredit { key: PAYOUT_KEY },
        )?;
        self.send(CashoutActor::Provider, CashoutAction::ClaimPaid)
    }

    /// R acknowledges, S confirms on that ack, and P gives S its deposit script.
    fn confirm(&mut self) -> Result<(), String> {
        self.send(CashoutActor::Recipient, CashoutAction::RecipientAck)?;
        self.send(CashoutActor::Sender, CashoutAction::ConfirmPayout)?;
        self.send(CashoutActor::Provider, CashoutAction::PaymentRequest)
    }

    /// S sends the BTC; P accepts the output once it has `min_conf` confirmations.
    fn deliver(&mut self) -> Result<(), String> {
        self.send(
            CashoutActor::Sender,
            CashoutAction::SendBitcoin {
                outpoint: FUNDING_OUTPOINT,
            },
        )?;
        self.ticks(self.node.terms.quote.min_conf - 1)?;
        self.send(
            CashoutActor::Provider,
            CashoutAction::AcceptBitcoin {
                outpoint: FUNDING_OUTPOINT,
            },
        )
    }

    fn untouched(&self) -> bool {
        let easy = &self.node.state.easy;
        !easy.fiat_transferred
            && !easy.btc_delivered
            && easy.seller_btc_sats == self.node.terms.offer.seller_initial_btc_sats
    }
}

pub(crate) fn run_bisq_easy_cashout(
    scenario: &'static str,
    quote: CashoutQuote,
) -> Result<CashoutScore, String> {
    use CashoutActor::{Provider, Rail, Sender};
    let node = EasyCashoutNode::new(quote)
        .map_err(|fault| format!("invalid cashout-bisq-easy quote: {}", fault.code()))?;
    let net = quote.net_payout_minor;
    let mut r = Runner::new(node);
    let deadline_ticks = SELLER_DELIVERY_DEADLINE_ROUNDS + 1;
    let withheld = CashoutFault::Easy(EasyFault::SellerNonDelivery);
    let check = match scenario {
        "honest" => {
            r.take()?;
            r.pay()?;
            r.confirm()?;
            r.deliver()?;
            r.node.state.easy.buyer_btc_sats == quote.amount_sats
        }
        "provider_withholds" => {
            r.take()?;
            r.ticks(deadline_ticks)?;
            r.untouched() && r.node.state.easy.phase == EasyPhase::Taken
        }
        "claim_without_credit" => {
            r.take()?;
            r.send(Provider, CashoutAction::ClaimPaid)?;
            r.refuse(
                Sender,
                CashoutAction::ConfirmPayout,
                CashoutFault::NoPayeeEvidence,
            )?;
            r.ticks(deadline_ticks)?;
            r.blocked == 1 && r.untouched()
        }
        "sender_withholds" => {
            r.take()?;
            r.pay()?;
            r.confirm()?;
            r.ticks(deadline_ticks)?;
            r.node.audit() == vec![withheld] && r.node.state.provider_exposed_minor == net
        }
        "replay_funding" | "wrong_output" => {
            let (outpoint, fault) = if scenario == "replay_funding" {
                (OLD_OUTPOINT, CashoutFault::ReplayedFunding)
            } else {
                (SENDER_OUTPOINT, CashoutFault::WrongOutput)
            };
            r.take()?;
            r.pay()?;
            r.confirm()?;
            r.refuse(Provider, CashoutAction::AcceptBitcoin { outpoint }, fault)?;
            r.ticks(deadline_ticks)?;
            r.blocked == 1
                && r.node.audit() == vec![withheld]
                && r.node.state.provider_exposed_minor == net
        }
        "late_funding" => {
            r.take()?;
            r.pay()?;
            r.confirm()?;
            r.ticks(deadline_ticks)?;
            r.deliver()?;
            r.node.audit() == vec![CashoutFault::SellerLateDelivery]
                && r.node.state.easy.phase == EasyPhase::Completed
                && r.node.state.provider_exposed_minor == net
        }
        "duplicate_payout" => {
            r.take()?;
            r.send(Provider, CashoutAction::SubmitPayout { key: PAYOUT_KEY })?;
            r.refuse(
                Provider,
                CashoutAction::SubmitPayout { key: RETRY_KEY },
                CashoutFault::DuplicatePayout,
            )?;
            r.send(Rail, CashoutAction::RailCredit { key: PAYOUT_KEY })?;
            r.refuse(
                Rail,
                CashoutAction::RailCredit { key: PAYOUT_KEY },
                CashoutFault::DuplicateCredit,
            )?;
            r.send(Provider, CashoutAction::ClaimPaid)?;
            r.confirm()?;
            r.deliver()?;
            r.blocked == 2 && r.node.settled()
        }
        "rail_timeout" => {
            r.take()?;
            r.send(Provider, CashoutAction::SubmitPayout { key: PAYOUT_KEY })?;
            r.send(Rail, CashoutAction::RailTimeout { key: PAYOUT_KEY })?;
            r.refuse(
                Provider,
                CashoutAction::SubmitPayout { key: RETRY_KEY },
                CashoutFault::PayoutOutcomeUnknown,
            )?;
            r.send(Rail, CashoutAction::RailCredit { key: PAYOUT_KEY })?;
            r.send(Provider, CashoutAction::RailStatus { key: PAYOUT_KEY })?;
            r.send(Provider, CashoutAction::ClaimPaid)?;
            // S confirms on rail evidence here, the second evidence path.
            r.send(Rail, CashoutAction::RailEvidence)?;
            r.send(Sender, CashoutAction::ConfirmPayout)?;
            r.send(Provider, CashoutAction::PaymentRequest)?;
            r.deliver()?;
            r.blocked == 1 && r.node.settled() && !r.node.state.recipient_acked
        }
        _ => return Err(format!("no cashout-bisq-easy runner for '{scenario}'")),
    };
    r.into_score(CashoutProfile::BisqEasy, scenario, check)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cashout::{run_cashout, CASHOUT_SCENARIOS};
    use CashoutActor::{Clock, Provider, Rail, Recipient, Sender};

    fn act(
        node: &mut EasyCashoutNode,
        actor: CashoutActor,
        action: CashoutAction,
    ) -> Result<bool, CashoutFault> {
        let id = node.journal.last().map_or(1, |e| e.id + 1);
        node.receive(CashoutEvent::new(id, actor, action))
    }

    fn paid_and_claimed() -> EasyCashoutNode {
        let mut node = EasyCashoutNode::new(CashoutQuote::default()).unwrap();
        act(&mut node, Provider, CashoutAction::TakeQuote).unwrap();
        act(
            &mut node,
            Provider,
            CashoutAction::SubmitPayout { key: PAYOUT_KEY },
        )
        .unwrap();
        act(
            &mut node,
            Rail,
            CashoutAction::RailCredit { key: PAYOUT_KEY },
        )
        .unwrap();
        act(&mut node, Provider, CashoutAction::ClaimPaid).unwrap();
        node
    }

    #[test]
    fn sender_releases_btc_only_on_ack_or_rail_evidence() {
        let mut node = paid_and_claimed();
        // R really was credited, and P says so, but S has no evidence of its own.
        assert!(node.state.easy.fiat_transferred);
        assert_eq!(
            act(&mut node, Sender, CashoutAction::ConfirmPayout),
            Err(CashoutFault::NoPayeeEvidence)
        );
        assert_eq!(
            act(
                &mut node,
                Sender,
                CashoutAction::SendBitcoin {
                    outpoint: FUNDING_OUTPOINT
                }
            ),
            Err(CashoutFault::Easy(EasyFault::InvalidTransition))
        );
        assert_eq!(node.state.easy.buyer_btc_sats, 0);
        act(&mut node, Rail, CashoutAction::RailEvidence).unwrap();
        act(&mut node, Sender, CashoutAction::ConfirmPayout).unwrap();
        assert_eq!(node.state.easy.phase, EasyPhase::FiatConfirmed);
    }

    #[test]
    fn neither_ack_nor_rail_evidence_exists_without_a_rail_credit() {
        let mut node = EasyCashoutNode::new(CashoutQuote::default()).unwrap();
        act(&mut node, Provider, CashoutAction::TakeQuote).unwrap();
        act(&mut node, Provider, CashoutAction::ClaimPaid).unwrap();
        for (actor, action) in [
            (Recipient, CashoutAction::RecipientAck),
            (Rail, CashoutAction::RailEvidence),
        ] {
            assert_eq!(
                act(&mut node, actor, action),
                Err(CashoutFault::Easy(EasyFault::FiatNotReceived))
            );
        }
        // P cannot author the payee's evidence or the rail's credit.
        assert_eq!(
            act(&mut node, Provider, CashoutAction::RailEvidence),
            Err(CashoutFault::Unauthorized)
        );
        assert_eq!(
            act(&mut node, Provider, CashoutAction::RecipientAck),
            Err(CashoutFault::Unauthorized)
        );
    }

    #[test]
    fn provider_is_always_the_easy_buyer() {
        let mut r = Runner::new(EasyCashoutNode::new(CashoutQuote::default()).unwrap());
        r.take().unwrap();
        r.pay().unwrap();
        r.confirm().unwrap();
        r.deliver().unwrap();
        let easy = r.node.easy_node();
        let actor_of = |id: u64| r.node.journal.iter().find(|e| e.id == id).map(|e| e.actor);
        let mut provider_events = 0;
        for mapped in &easy.journal {
            match actor_of(mapped.id) {
                Some(Provider) => {
                    assert_eq!(mapped.actor, EasyActor::Buyer, "{mapped:?}");
                    provider_events += 1;
                }
                Some(Sender) => assert_eq!(mapped.actor, EasyActor::Seller, "{mapped:?}"),
                other => assert!(matches!(other, Some(Rail | Clock)), "{mapped:?}"),
            }
        }
        // TakeOffer, ClaimFiatSent and SetBitcoinPaymentRequest.
        assert_eq!(provider_events, 3);
        for scenario in CASHOUT_SCENARIOS {
            assert!(
                run_cashout(CashoutProfile::BisqEasy, scenario)
                    .unwrap()
                    .passed
            );
        }
    }

    #[test]
    fn reused_machine_conserves_value_and_replays() {
        let mut r = Runner::new(EasyCashoutNode::new(CashoutQuote::default()).unwrap());
        r.take().unwrap();
        r.pay().unwrap();
        r.confirm().unwrap();
        r.deliver().unwrap();
        let easy = r.node.easy_node();
        assert!(easy.audit().is_empty());
        assert_eq!(easy.replay().unwrap(), r.node.state.easy);
        assert_eq!(r.node.replay().unwrap(), r.node.state);
        // P's float became R's credit; S's BTC became P's, less the miner fee.
        let state = &easy.state;
        assert_eq!(
            (state.buyer_fiat_cents, state.seller_fiat_cents),
            (0, 5_000)
        );
        assert_eq!(state.buyer_btc_sats, 100_000);
        assert_eq!(
            state.miner_btc_sats,
            crate::market::SIMULATED_CHAIN_FEE_SATS
        );
    }

    #[test]
    fn duplicate_rail_credit_is_also_refused_by_the_reused_machine() {
        let node = paid_and_claimed();
        let again = node.state.easy.transition(
            node.terms.offer,
            node.terms.oracle,
            node.terms.now,
            EasyEvent::new(99, EasyActor::Bank, EasyAction::TransferFiat),
        );
        assert_eq!(again, Err(EasyFault::InvalidTransition));
    }

    #[test]
    fn non_usd_or_out_of_range_quotes_are_refused() {
        let ils =
            CashoutQuote::new(13_293, 5_000, Currency::parse("ILS").unwrap(), 2, 206, 2).unwrap();
        assert_eq!(
            EasyCashoutNode::new(ils).err(),
            Some(CashoutFault::UnsupportedCurrency)
        );
        let too_big = CashoutQuote::new(1_300_000, 60_100, Currency::USD, 2, 206, 2).unwrap();
        assert_eq!(
            EasyCashoutNode::new(too_big).err(),
            Some(CashoutFault::Easy(EasyFault::InvalidOfferAmount))
        );
        let usd = CashoutQuote::new(13_293, 5_000, Currency::USD, 2, 206, 2).unwrap();
        for scenario in CASHOUT_SCENARIOS {
            let score = run_cashout_with_quote(scenario, usd);
            assert!(score.passed, "{}", score.json_line());
        }
    }

    fn run_cashout_with_quote(scenario: &str, quote: CashoutQuote) -> CashoutScore {
        crate::cashout::run_cashout_with(CashoutProfile::BisqEasy, scenario, quote).unwrap()
    }

    #[test]
    fn late_delivery_cures_the_debt_but_not_the_exposure() {
        let mut r = Runner::new(EasyCashoutNode::new(CashoutQuote::default()).unwrap());
        r.take().unwrap();
        r.pay().unwrap();
        r.confirm().unwrap();
        r.ticks(SELLER_DELIVERY_DEADLINE_ROUNDS + 1).unwrap();
        assert_eq!(
            r.node.audit(),
            vec![CashoutFault::Easy(EasyFault::SellerNonDelivery)]
        );
        r.deliver().unwrap();
        assert_eq!(r.node.audit(), vec![CashoutFault::SellerLateDelivery]);
        assert_eq!(r.node.state.provider_exposed_minor, 5_000);
        act(&mut r.node, Clock, CashoutAction::Tick).unwrap();
        assert_eq!(r.node.state.provider_exposed_minor, 5_000);
    }
}
