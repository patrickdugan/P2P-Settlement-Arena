//! Seeded multi-agent Bisq Easy *model*.
//!
//! This is an abstract offer-discovery and simulated-balance environment.
//! It is not a Bisq network adapter, payment integration, or execution engine.
//! Every accepted trade is run through the single-trade EasyNode state machine.

use crate::market::{
    EasyAction, EasyActor, EasyEvent, EasyFault, EasyNode, Offer, Oracle, Rail, SATS_PER_BTC,
    SIMULATED_CHAIN_FEE_SATS, SIMULATED_NOW,
};
use std::cmp::Ordering;
use std::collections::BTreeSet;

const OFFER_LIFETIME_ROUNDS: u32 = 4;
const FIAT_QUOTES_CENTS: &[u64] = &[1_000, 2_000, 3_000, 4_000, 5_000, 7_000, 9_000];
const BTC_PRICES_CENTS: &[u64] = &[4_900_000, 4_950_000, 5_000_000, 5_050_000, 5_100_000];
const SELLER_SCORES: &[u64] = &[1_200, 4_000, 8_000, 16_000, 32_000, 80_000];

/// A local heuristic above Bisq's protocol-level reputation validation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ChoicePolicy {
    #[default]
    ReputationFirst,
    BestPrice,
}

impl ChoicePolicy {
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "reputation" | "reputation-first" => Some(Self::ReputationFirst),
            "price" | "best-price" => Some(Self::BestPrice),
            _ => None,
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Self::ReputationFirst => "reputation-first",
            Self::BestPrice => "best-price",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SwarmConfig {
    pub seed: u64,
    pub rounds: u32,
    pub buyers: usize,
    pub sellers: usize,
    pub attacker_percent: u8,
    pub policy: ChoicePolicy,
}

impl Default for SwarmConfig {
    fn default() -> Self {
        Self {
            seed: 7,
            rounds: 48,
            buyers: 10,
            sellers: 8,
            attacker_percent: 30,
            policy: ChoicePolicy::ReputationFirst,
        }
    }
}

impl SwarmConfig {
    pub fn validate(&self) -> Result<(), String> {
        if !(1..=4096).contains(&self.rounds) {
            return Err("rounds must be in 1..=4096".into());
        }
        if !(1..=128).contains(&self.buyers) || !(1..=128).contains(&self.sellers) {
            return Err("buyers and sellers must each be in 1..=128".into());
        }
        if self.attacker_percent > 100 {
            return Err("attacker-pct must be in 0..=100".into());
        }
        Ok(())
    }

    /// Parse flags after the \`swarm\` command.
    pub fn from_args(args: &[String]) -> Result<(Self, bool), String> {
        let mut config = Self::default();
        let mut trace = false;
        let mut index = 0;
        while index < args.len() {
            let flag = args[index].as_str();
            if flag == "--trace" {
                trace = true;
                index += 1;
                continue;
            }
            if flag == "--help" || flag == "-h" {
                return Err(usage().to_string());
            }
            if index + 1 >= args.len() {
                return Err(format!("missing value for {flag}\n{}", usage()));
            }
            let value = args[index + 1].as_str();
            match flag {
                "--seed" => {
                    config.seed = value
                        .parse()
                        .map_err(|_| format!("invalid --seed: {value}"))?;
                }
                "--rounds" => {
                    config.rounds = value
                        .parse()
                        .map_err(|_| format!("invalid --rounds: {value}"))?;
                }
                "--buyers" => {
                    config.buyers = value
                        .parse()
                        .map_err(|_| format!("invalid --buyers: {value}"))?;
                }
                "--sellers" => {
                    config.sellers = value
                        .parse()
                        .map_err(|_| format!("invalid --sellers: {value}"))?;
                }
                "--attacker-pct" => {
                    config.attacker_percent = value
                        .parse()
                        .map_err(|_| format!("invalid --attacker-pct: {value}"))?;
                }
                "--policy" => {
                    config.policy = ChoicePolicy::parse(value)
                        .ok_or_else(|| format!("unknown policy: {value}"))?;
                }
                _ => return Err(format!("unknown flag: {flag}\n{}", usage())),
            }
            index += 2;
        }
        config.validate()?;
        Ok((config, trace))
    }
}

pub fn usage() -> &'static str {
    "Usage: settlement-arena swarm [--seed U64] [--rounds 1..4096] [--buyers 1..128] \
     [--sellers 1..128] [--attacker-pct 0..100] [--policy reputation|price] [--trace]\n\
     Default: Bisq Easy reference model; seeded, offline, simulated-only."
}

/// A fast reproducible PRNG; deliberately unsuitable for secrets or cryptography.
struct SplitMix64(u64);

impl SplitMix64 {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    fn choose(&mut self, bound: usize) -> usize {
        (self.next_u64() % bound as u64) as usize
    }

    fn percent(&mut self, threshold: u8) -> bool {
        self.choose(100) < usize::from(threshold)
    }

    // External arrival and offer streams remain fixed across buyer policies.
    fn for_offer(seed: u64, round: u32, seller_idx: usize) -> Self {
        Self(
            seed.wrapping_add(0xa076_1d64_78bd_642f)
                .wrapping_add(u64::from(round).wrapping_mul(0xe703_7ed1_a0b4_28db))
                .wrapping_add((seller_idx as u64).wrapping_mul(0x8ebc_6af0_9c88_c6e3)),
        )
    }

    fn for_arrival(seed: u64, round: u32) -> Self {
        Self(
            seed.wrapping_add(0x5899_65cc_7537_4cc3)
                .wrapping_add(u64::from(round).wrapping_mul(0xe703_7ed1_a0b4_28db)),
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuyerBehavior {
    Honest,
    FalsePaymentClaim,
    OversizedInvoice,
    Chargeback,
}

impl BuyerBehavior {
    fn id(self) -> &'static str {
        match self {
            Self::Honest => "honest",
            Self::FalsePaymentClaim => "false_payment_claim",
            Self::OversizedInvoice => "oversized_invoice",
            Self::Chargeback => "chargeback",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SellerBehavior {
    Honest,
    WithholdBtc,
}

impl SellerBehavior {
    fn id(self) -> &'static str {
        match self {
            Self::Honest => "honest",
            Self::WithholdBtc => "withhold_btc",
        }
    }
}

#[derive(Clone, Debug)]
struct Buyer {
    fiat_cents: u64,
    btc_sats: u64,
    behavior: BuyerBehavior,
}

#[derive(Clone, Debug)]
struct Seller {
    fiat_cents: u64,
    btc_sats: u64,
    reputation: u64,
    behavior: SellerBehavior,
    rail: Rail,
}

#[derive(Clone, Debug)]
struct Listing {
    id: u64,
    seller_idx: usize,
    created_round: u32,
    quote_cents: u64,
    btc_sats: u64,
    price_cents: u64,
    rail: Rail,
    active: bool,
}

impl Listing {
    fn as_offer(&self, seller: &Seller) -> Offer {
        Offer {
            quote_usd_cents: self.quote_cents,
            btc_sats: self.btc_sats,
            fixed_price_usd_cents_per_btc: self.price_cents,
            seller_reputation_points: seller.reputation,
            seller_initial_btc_sats: seller.btc_sats,
            rail: self.rail,
        }
    }

    fn within(&self, buyer: &Buyer, seller: &Seller, policy: ChoicePolicy) -> bool {
        if !self.active || self.quote_cents > buyer.fiat_cents {
            return false;
        }
        if seller.btc_sats < self.btc_sats.saturating_add(SIMULATED_CHAIN_FEE_SATS) {
            return false;
        }
        match policy {
            ChoicePolicy::BestPrice => true,
            ChoicePolicy::ReputationFirst => {
                // Arena-level buyer caution policy, *not* a Bisq protocol rule.
                // 2x coverage and an irreversible synthetic fiat rail.
                let rounded_usd = (self.quote_cents + 50) / 100;
                let required_score = rounded_usd.saturating_mul(200);
                seller.reputation >= required_score.saturating_mul(2)
                    && self.rail == Rail::SimulatedIrreversible
            }
        }
    }
}

fn listing_cmp(a: &Listing, b: &Listing, sellers: &[Seller], policy: ChoicePolicy) -> Ordering {
    match policy {
        ChoicePolicy::BestPrice => a
            .price_cents
            .cmp(&b.price_cents)
            .then(
                sellers[b.seller_idx]
                    .reputation
                    .cmp(&sellers[a.seller_idx].reputation),
            )
            .then(a.id.cmp(&b.id)),
        ChoicePolicy::ReputationFirst => sellers[b.seller_idx]
            .reputation
            .cmp(&sellers[a.seller_idx].reputation)
            .then(a.price_cents.cmp(&b.price_cents))
            .then(a.id.cmp(&b.id)),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TradeOutcome {
    Completed,
    FiatClaimRejected,
    BuyerPaidSellerWithheld,
    FiatChargebackAfterBtc,
}

impl TradeOutcome {
    fn id(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::FiatClaimRejected => "fiat_claim_rejected",
            Self::BuyerPaidSellerWithheld => "seller_non_delivery",
            Self::FiatChargebackAfterBtc => "post_delivery_chargeback",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TradeRecord {
    pub policy: ChoicePolicy,
    pub round: u32,
    pub offer_id: u64,
    pub buyer_idx: usize,
    pub seller_idx: usize,
    pub quote_usd_cents: u64,
    pub bitcoin_sats: u64,
    pub buyer_strategy: BuyerBehavior,
    pub seller_strategy: SellerBehavior,
    pub outcome: TradeOutcome,
    pub blocked_attacks: u32,
    pub buyer_usd_at_risk_cents: u64,
    pub seller_btc_at_risk_sats: u64,
    pub miner_fee_sats: u64,
}

impl TradeRecord {
    pub fn json_line(&self) -> String {
        format!(
            "{{\"kind\":\"trade\",\"policy\":\"{}\",\"round\":{},\"offer_id\":{},\"buyer\":{},\"seller\":{},\"quote_usd_cents\":{},\"btc_sats\":{},\"buyer_strategy\":\"{}\",\"seller_strategy\":\"{}\",\"outcome\":\"{}\",\"blocked_attacks\":{},\"buyer_usd_at_risk_cents\":{},\"seller_btc_at_risk_sats\":{},\"miner_fee_sats\":{}}}",
            self.policy.id(),
            self.round,
            self.offer_id,
            self.buyer_idx,
            self.seller_idx,
            self.quote_usd_cents,
            self.bitcoin_sats,
            self.buyer_strategy.id(),
            self.seller_strategy.id(),
            self.outcome.id(),
            self.blocked_attacks,
            self.buyer_usd_at_risk_cents,
            self.seller_btc_at_risk_sats,
            self.miner_fee_sats
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SwarmReport {
    pub config: SwarmConfig,
    pub offers_posted: usize,
    pub offers_rejected: usize,
    pub offers_expired: usize,
    pub unfilled_rounds: usize,
    pub trades: Vec<TradeRecord>,
    pub completed: usize,
    pub seller_non_delivery: usize,
    pub chargeback_after_btc: usize,
    pub blocked_attacks: u64,
    pub fiat_volume_cents: u128,
    pub btc_volume_sats: u128,
    pub miner_fees_sats: u128,
    pub buyer_usd_at_risk_cents: u128,
    pub seller_btc_at_risk_sats: u128,
    pub conserved: bool,
}

impl SwarmReport {
    fn new(config: SwarmConfig) -> Self {
        Self {
            config,
            offers_posted: 0,
            offers_rejected: 0,
            offers_expired: 0,
            unfilled_rounds: 0,
            trades: Vec::new(),
            completed: 0,
            seller_non_delivery: 0,
            chargeback_after_btc: 0,
            blocked_attacks: 0,
            fiat_volume_cents: 0,
            btc_volume_sats: 0,
            miner_fees_sats: 0,
            buyer_usd_at_risk_cents: 0,
            seller_btc_at_risk_sats: 0,
            conserved: true,
        }
    }

    pub fn attack_successes(&self) -> usize {
        self.seller_non_delivery + self.chargeback_after_btc
    }

    pub fn json_line(&self) -> String {
        format!(
            "{{\"kind\":\"swarm_summary\",\"protocol\":\"bisq-easy\",\"seed\":{},\"rounds\":{},\"buyers\":{},\"sellers\":{},\"attacker_percent\":{},\"policy\":\"{}\",\"offers_posted\":{},\"offers_rejected\":{},\"offers_expired\":{},\"unfilled_rounds\":{},\"matched_trades\":{},\"completed\":{},\"seller_non_delivery\":{},\"post_delivery_chargeback\":{},\"blocked_attacks\":{},\"attack_successes\":{},\"fiat_volume_cents\":{},\"btc_volume_sats\":{},\"miner_fees_sats\":{},\"buyer_usd_at_risk_cents\":{},\"seller_btc_at_risk_sats\":{},\"conserved\":{}}}",
            self.config.seed,
            self.config.rounds,
            self.config.buyers,
            self.config.sellers,
            self.config.attacker_percent,
            self.config.policy.id(),
            self.offers_posted,
            self.offers_rejected,
            self.offers_expired,
            self.unfilled_rounds,
            self.trades.len(),
            self.completed,
            self.seller_non_delivery,
            self.chargeback_after_btc,
            self.blocked_attacks,
            self.attack_successes(),
            self.fiat_volume_cents,
            self.btc_volume_sats,
            self.miner_fees_sats,
            self.buyer_usd_at_risk_cents,
            self.seller_btc_at_risk_sats,
            self.conserved
        )
    }
}

fn dispatch(
    node: &mut EasyNode,
    event_id: &mut u64,
    actor: EasyActor,
    action: EasyAction,
) -> Result<(), String> {
    let event = EasyEvent::new(*event_id, actor, action);
    *event_id += 1;
    node.receive(event)
        .map(|_| ())
        .map_err(|fault| format!("unexpected transition {}: {fault:?}", *event_id - 1))
}

fn expect_denial(
    node: &mut EasyNode,
    event_id: &mut u64,
    actor: EasyActor,
    action: EasyAction,
    expected: EasyFault,
) -> Result<(), String> {
    let event = EasyEvent::new(*event_id, actor, action);
    *event_id += 1;
    let result = node.receive(event);
    if result != Err(expected) {
        return Err(format!(
            "expected {} rejection; observed {result:?}",
            expected.code()
        ));
    }
    Ok(())
}

struct Execution {
    node: EasyNode,
    blocked: u32,
    outcome: TradeOutcome,
}

fn execute_trade(
    offer: Offer,
    buyer_behavior: BuyerBehavior,
    seller_behavior: SellerBehavior,
) -> Result<Execution, String> {
    let mut node = EasyNode::new(offer, Oracle::default(), SIMULATED_NOW);
    let mut seq = 1;
    let mut blocked = 0;
    dispatch(
        &mut node,
        &mut seq,
        EasyActor::Buyer,
        EasyAction::TakeOffer {
            contract_price: offer.fixed_price_usd_cents_per_btc,
        },
    )?;

    if buyer_behavior == BuyerBehavior::FalsePaymentClaim {
        dispatch(
            &mut node,
            &mut seq,
            EasyActor::Buyer,
            EasyAction::ClaimFiatSent,
        )?;
        expect_denial(
            &mut node,
            &mut seq,
            EasyActor::Seller,
            EasyAction::ConfirmFiat,
            EasyFault::FiatNotReceived,
        )?;
        blocked += 1;
        return Ok(Execution {
            node,
            blocked,
            outcome: TradeOutcome::FiatClaimRejected,
        });
    }

    // The modeled bank is separate from the buyer and signals a completed transfer.
    dispatch(
        &mut node,
        &mut seq,
        EasyActor::Bank,
        EasyAction::TransferFiat,
    )?;
    dispatch(
        &mut node,
        &mut seq,
        EasyActor::Buyer,
        EasyAction::ClaimFiatSent,
    )?;
    dispatch(
        &mut node,
        &mut seq,
        EasyActor::Seller,
        EasyAction::ConfirmFiat,
    )?;

    if buyer_behavior == BuyerBehavior::OversizedInvoice {
        let inflated = offer
            .btc_sats
            .checked_mul(10)
            .ok_or("overflowed invoice amount")?;
        expect_denial(
            &mut node,
            &mut seq,
            EasyActor::Buyer,
            EasyAction::SetBitcoinPaymentRequest {
                requested_sats: Some(inflated),
            },
            EasyFault::PaymentRequestAmountMismatch,
        )?;
        blocked += 1;
    }

    if seller_behavior == SellerBehavior::WithholdBtc {
        for _ in 0..4 {
            dispatch(&mut node, &mut seq, EasyActor::Clock, EasyAction::Tick)?;
        }
        dispatch(
            &mut node,
            &mut seq,
            EasyActor::Buyer,
            EasyAction::OpenDispute,
        )?;
        return Ok(Execution {
            node,
            blocked,
            outcome: TradeOutcome::BuyerPaidSellerWithheld,
        });
    }

    dispatch(
        &mut node,
        &mut seq,
        EasyActor::Buyer,
        EasyAction::SetBitcoinPaymentRequest {
            requested_sats: Some(offer.btc_sats),
        },
    )?;
    dispatch(
        &mut node,
        &mut seq,
        EasyActor::Seller,
        EasyAction::SendBitcoin,
    )?;

    let mut outcome = TradeOutcome::Completed;
    if buyer_behavior == BuyerBehavior::Chargeback {
        if offer.rail == Rail::SimulatedReversible {
            dispatch(&mut node, &mut seq, EasyActor::Bank, EasyAction::Chargeback)?;
            outcome = TradeOutcome::FiatChargebackAfterBtc;
        } else {
            expect_denial(
                &mut node,
                &mut seq,
                EasyActor::Bank,
                EasyAction::Chargeback,
                EasyFault::ChargebackBlocked,
            )?;
            blocked += 1;
        }
    }

    if node.replay().map_err(|f| f.code().to_string())? != node.state {
        return Err("event journal replay diverged after trade".into());
    }
    Ok(Execution {
        node,
        blocked,
        outcome,
    })
}

fn total_balances(buyers: &[Buyer], sellers: &[Seller], miner_fees: u128) -> (u128, u128) {
    let fiat = buyers
        .iter()
        .map(|b| u128::from(b.fiat_cents))
        .sum::<u128>()
        + sellers
            .iter()
            .map(|s| u128::from(s.fiat_cents))
            .sum::<u128>();
    let btc = buyers.iter().map(|b| u128::from(b.btc_sats)).sum::<u128>()
        + sellers.iter().map(|s| u128::from(s.btc_sats)).sum::<u128>()
        + miner_fees;
    (fiat, btc)
}

fn apply_balances(
    execution: &Execution,
    buyer: &mut Buyer,
    seller: &mut Seller,
    initial_seller_sats: u64,
) -> Result<(), String> {
    let state = &execution.node.state;
    let spent_fiat = execution
        .node
        .offer
        .quote_usd_cents
        .checked_sub(state.buyer_fiat_cents)
        .ok_or("fiat accounting underflow")?;
    let spent_btc = initial_seller_sats
        .checked_sub(state.seller_btc_sats)
        .ok_or("BTC accounting underflow")?;
    let new_buyer_fiat = buyer
        .fiat_cents
        .checked_sub(spent_fiat)
        .ok_or("buyer overspent fiat")?;
    let new_seller_fiat = seller
        .fiat_cents
        .checked_add(state.seller_fiat_cents)
        .ok_or("seller fiat overflow")?;
    let new_seller_btc = seller
        .btc_sats
        .checked_sub(spent_btc)
        .ok_or("seller overspent BTC")?;
    let new_buyer_btc = buyer
        .btc_sats
        .checked_add(state.buyer_btc_sats)
        .ok_or("buyer BTC overflow")?;
    buyer.fiat_cents = new_buyer_fiat;
    buyer.btc_sats = new_buyer_btc;
    seller.fiat_cents = new_seller_fiat;
    seller.btc_sats = new_seller_btc;
    Ok(())
}

fn post_offer(
    seller_index: usize,
    seller: &Seller,
    round: u32,
    next_id: &mut u64,
    rng: &mut SplitMix64,
) -> Option<Listing> {
    let quote = FIAT_QUOTES_CENTS[rng.choose(FIAT_QUOTES_CENTS.len())];
    let price = BTC_PRICES_CENTS[rng.choose(BTC_PRICES_CENTS.len())];
    let sats = (u128::from(quote) * SATS_PER_BTC + u128::from(price / 2)) / u128::from(price);
    let sats = u64::try_from(sats).ok()?;
    if seller.btc_sats < sats.checked_add(SIMULATED_CHAIN_FEE_SATS)? {
        return None;
    }
    let listing = Listing {
        id: *next_id,
        seller_idx: seller_index,
        created_round: round,
        quote_cents: quote,
        btc_sats: sats,
        price_cents: price,
        rail: seller.rail,
        active: true,
    };
    *next_id += 1;
    Some(listing)
}

fn choose_listing(
    book: &[Listing],
    buyers: &[Buyer],
    sellers: &[Seller],
    buyer_idx: usize,
    policy: ChoicePolicy,
) -> Option<usize> {
    book.iter()
        .enumerate()
        .filter(|(_, listing)| {
            listing.within(&buyers[buyer_idx], &sellers[listing.seller_idx], policy)
        })
        .min_by(|(_, a), (_, b)| listing_cmp(a, b, sellers, policy))
        .map(|(index, _)| index)
}

pub fn run_swarm(config: SwarmConfig) -> Result<SwarmReport, String> {
    config.validate()?;
    let mut rng = SplitMix64(config.seed);
    let mut buyers: Vec<Buyer> = (0..config.buyers)
        .map(|_| {
            let behavior = if rng.percent(config.attacker_percent) {
                match rng.choose(3) {
                    0 => BuyerBehavior::FalsePaymentClaim,
                    1 => BuyerBehavior::OversizedInvoice,
                    _ => BuyerBehavior::Chargeback,
                }
            } else {
                BuyerBehavior::Honest
            };
            Buyer {
                fiat_cents: 30_000 + rng.choose(20_000) as u64,
                btc_sats: 0,
                behavior,
            }
        })
        .collect();
    let mut sellers: Vec<Seller> = (0..config.sellers)
        .map(|_| Seller {
            fiat_cents: 0,
            btc_sats: 2_000_000 + rng.choose(2_000_000) as u64,
            reputation: SELLER_SCORES[rng.choose(SELLER_SCORES.len())],
            behavior: if rng.percent(config.attacker_percent) {
                SellerBehavior::WithholdBtc
            } else {
                SellerBehavior::Honest
            },
            rail: if rng.percent(30) {
                Rail::SimulatedReversible
            } else {
                Rail::SimulatedIrreversible
            },
        })
        .collect();

    let initial = total_balances(&buyers, &sellers, 0);
    let mut report = SwarmReport::new(config);
    let mut book: Vec<Listing> = Vec::new();
    let mut next_offer_id = 1_u64;
    let mut seen_takes = BTreeSet::new();

    for round in 0..config.rounds {
        for listing in &mut book {
            if listing.active
                && round.saturating_sub(listing.created_round) >= OFFER_LIFETIME_ROUNDS
            {
                listing.active = false;
                report.offers_expired += 1;
            }
        }

        // Each seller may have at most one resting listing. Only the seller
        // owning a listing can replenish it; a take consumes it exactly once.
        for (idx, seller) in sellers.iter().enumerate() {
            if book
                .iter()
                .any(|listing| listing.active && listing.seller_idx == idx)
            {
                continue;
            }
            let mut offer_rng = SplitMix64::for_offer(config.seed, round, idx);
            if let Some(listing) =
                post_offer(idx, seller, round, &mut next_offer_id, &mut offer_rng)
            {
                book.push(listing);
                report.offers_posted += 1;
            }
        }

        let buyer_idx = SplitMix64::for_arrival(config.seed, round).choose(buyers.len());
        let mut took_offer = false;
        // If a malformed or under-reputation offer fails protocol checks,
        // withdraw it and attempt the next eligible offer in this round.
        while let Some(index) = choose_listing(&book, &buyers, &sellers, buyer_idx, config.policy) {
            let listing = book[index].clone();
            let seller_idx = listing.seller_idx;
            let offer = listing.as_offer(&sellers[seller_idx]);
            if offer
                .verify(
                    Oracle::default(),
                    SIMULATED_NOW,
                    offer.fixed_price_usd_cents_per_btc,
                )
                .is_err()
            {
                book[index].active = false;
                report.offers_rejected += 1;
                continue;
            }
            if !seen_takes.insert(listing.id) {
                return Err(format!("same listing matched twice: {}", listing.id));
            }
            book[index].active = false;
            let buyer_behavior = buyers[buyer_idx].behavior;
            let seller_behavior = sellers[seller_idx].behavior;
            let execution = execute_trade(offer, buyer_behavior, seller_behavior)?;
            let state = &execution.node.state;
            let audit = execution.node.audit();

            // Each outcome must have the exact expected audit finding.
            let expected = match execution.outcome {
                TradeOutcome::Completed | TradeOutcome::FiatClaimRejected => &[][..],
                TradeOutcome::BuyerPaidSellerWithheld => &[EasyFault::SellerNonDelivery][..],
                TradeOutcome::FiatChargebackAfterBtc => &[EasyFault::PostDeliveryChargeback][..],
            };
            if audit != expected {
                return Err(format!(
                    "outcome {:?}, inconsistent audit: {:?}",
                    execution.outcome, audit
                ));
            }

            let fee = state.miner_btc_sats;
            let buyer_exposure = execution.node.buyer_usd_at_risk_cents();
            let seller_exposure = execution.node.seller_btc_at_risk_sats();
            apply_balances(
                &execution,
                &mut buyers[buyer_idx],
                &mut sellers[seller_idx],
                offer.seller_initial_btc_sats,
            )?;
            report.miner_fees_sats += u128::from(fee);
            report.blocked_attacks += u64::from(execution.blocked);
            report.buyer_usd_at_risk_cents += u128::from(buyer_exposure);
            report.seller_btc_at_risk_sats += u128::from(seller_exposure);
            // Gross fiat flow includes a bank transfer even when a later
            // chargeback reverses the balance transfer.
            if execution.node.journal.iter().any(|event| event.action == EasyAction::TransferFiat) {
                report.fiat_volume_cents += u128::from(offer.quote_usd_cents);
            }
            if state.btc_delivered {
                report.btc_volume_sats += u128::from(offer.btc_sats);
                report.completed += 1;
            }
            match execution.outcome {
                TradeOutcome::Completed | TradeOutcome::FiatClaimRejected => {}
                TradeOutcome::BuyerPaidSellerWithheld => report.seller_non_delivery += 1,
                TradeOutcome::FiatChargebackAfterBtc => report.chargeback_after_btc += 1,
            }
            report.trades.push(TradeRecord {
                policy: config.policy,
                round,
                offer_id: listing.id,
                buyer_idx,
                seller_idx,
                quote_usd_cents: listing.quote_cents,
                bitcoin_sats: listing.btc_sats,
                buyer_strategy: buyer_behavior,
                seller_strategy: seller_behavior,
                outcome: execution.outcome,
                blocked_attacks: execution.blocked,
                buyer_usd_at_risk_cents: buyer_exposure,
                seller_btc_at_risk_sats: seller_exposure,
                miner_fee_sats: fee,
            });
            if total_balances(&buyers, &sellers, report.miner_fees_sats) != initial {
                return Err(format!("balance conservation failed in round {round}"));
            }
            took_offer = true;
            break;
        }
        if !took_offer {
            report.unfilled_rounds += 1;
        }
    }
    report.conserved = total_balances(&buyers, &sellers, report.miner_fees_sats) == initial;
    Ok(report)
}

/// Evaluate both buyer selection policies with shared exogenous randomness.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SwarmComparison {
    pub reputation: SwarmReport,
    pub price: SwarmReport,
}

impl SwarmComparison {
    pub fn json_line(&self) -> String {
        fn difference(price: u128, reputation: u128) -> i128 {
            // The configured caps keep measured values far below i128::MAX.
            price as i128 - reputation as i128
        }
        format!(
            "{{\"kind\":\"policy_comparison\",\"protocol\":\"bisq-easy\",\"seed\":{},\"rounds\":{},\"paired_arrivals\":true,\"paired_offer_draws\":true,\"price_minus_reputation\":{{\"matched_trades\":{},\"completed\":{},\"attack_successes\":{},\"buyer_usd_at_risk_cents\":{},\"seller_btc_at_risk_sats\":{}}}}}",
            self.reputation.config.seed,
            self.reputation.config.rounds,
            self.price.trades.len() as i128 - self.reputation.trades.len() as i128,
            self.price.completed as i128 - self.reputation.completed as i128,
            self.price.attack_successes() as i128 - self.reputation.attack_successes() as i128,
            difference(self.price.buyer_usd_at_risk_cents, self.reputation.buyer_usd_at_risk_cents),
            difference(self.price.seller_btc_at_risk_sats, self.reputation.seller_btc_at_risk_sats)
        )
    }
}

pub fn compare_policies(config: SwarmConfig) -> Result<SwarmComparison, String> {
    config.validate()?;
    let reputation = run_swarm(SwarmConfig {
        policy: ChoicePolicy::ReputationFirst,
        ..config
    })?;
    let price = run_swarm(SwarmConfig {
        policy: ChoicePolicy::BestPrice,
        ..config
    })?;
    Ok(SwarmComparison { reputation, price })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::market::EasyPhase;

    #[test]
    fn exogenous_streams_are_repeatable() {
        for round in 0..50 {
            assert_eq!(
                SplitMix64::for_arrival(17, round).choose(11),
                SplitMix64::for_arrival(17, round).choose(11)
            );
            let mut a = SplitMix64::for_offer(17, round, 3);
            let mut b = SplitMix64::for_offer(17, round, 3);
            assert_eq!(a.next_u64(), b.next_u64());
            assert_eq!(a.next_u64(), b.next_u64());
        }
    }

    #[test]
    fn paired_comparison_uses_same_seed_and_both_policies() {
        let config = SwarmConfig {
            seed: 19,
            rounds: 60,
            ..SwarmConfig::default()
        };
        let comparison = compare_policies(config).unwrap();
        assert_eq!(
            comparison.reputation.config.seed,
            comparison.price.config.seed
        );
        assert_eq!(
            comparison.reputation.config.rounds,
            comparison.price.config.rounds
        );
        assert_eq!(
            comparison.reputation.config.policy,
            ChoicePolicy::ReputationFirst
        );
        assert_eq!(comparison.price.config.policy, ChoicePolicy::BestPrice);
        assert!(comparison.reputation.conserved && comparison.price.conserved);
        assert_eq!(
            comparison.reputation,
            run_swarm(SwarmConfig {
                policy: ChoicePolicy::ReputationFirst,
                ..config
            })
            .unwrap()
        );
        assert_eq!(
            comparison.price,
            run_swarm(SwarmConfig {
                policy: ChoicePolicy::BestPrice,
                ..config
            })
            .unwrap()
        );
        assert!(comparison.json_line().contains("\"paired_arrivals\":true"));
    }

    #[test]
    fn score_counts_gross_fiat_even_after_chargeback() {
        let report = run_swarm(SwarmConfig {
            seed: 7,
            policy: ChoicePolicy::BestPrice,
            ..SwarmConfig::default()
        })
        .unwrap();
        let btc_deliveries = report
            .trades
            .iter()
            .filter(|trade| matches!(trade.outcome, TradeOutcome::Completed | TradeOutcome::FiatChargebackAfterBtc))
            .count();
        let gross_fiat: u128 = report
            .trades
            .iter()
            .filter(|trade| trade.outcome != TradeOutcome::FiatClaimRejected)
            .map(|trade| u128::from(trade.quote_usd_cents))
            .sum();
        assert_eq!(report.completed, btc_deliveries);
        assert_eq!(report.fiat_volume_cents, gross_fiat);
        assert!(report.chargeback_after_btc > 0);
        assert!(report.conserved);
    }

    #[test]
    fn deterministic_seed_reproduces_exact_trace() {
        let config = SwarmConfig::default();
        let a = run_swarm(config).unwrap();
        let b = run_swarm(config).unwrap();
        assert_eq!(a, b);
        assert_eq!(a.json_line(), b.json_line());
        assert!(!a.trades.is_empty());
    }

    #[test]
    fn zero_attacker_population_has_no_fraud_losses() {
        for seed in 0..30 {
            let report = run_swarm(SwarmConfig {
                seed,
                attacker_percent: 0,
                ..SwarmConfig::default()
            })
            .unwrap();
            assert_eq!(report.attack_successes(), 0, "seed {seed}");
            assert_eq!(report.blocked_attacks, 0, "seed {seed}");
            assert!(report.conserved);
        }
    }

    #[test]
    fn conservation_across_many_seeds_and_buyer_policies() {
        for policy in [ChoicePolicy::BestPrice, ChoicePolicy::ReputationFirst] {
            for seed in 0..60 {
                let result = run_swarm(SwarmConfig {
                    seed,
                    policy,
                    ..SwarmConfig::default()
                })
                .unwrap();
                assert!(result.conserved, "seed {seed} policy {policy:?}");
                assert_eq!(
                    result.trades.len() + result.unfilled_rounds,
                    result.config.rounds as usize
                );
                let unique: BTreeSet<_> = result.trades.iter().map(|t| t.offer_id).collect();
                assert_eq!(unique.len(), result.trades.len());
                assert_eq!(
                    result.attack_successes(),
                    result.seller_non_delivery + result.chargeback_after_btc
                );
            }
        }
    }

    #[test]
    fn reputation_first_excludes_reversible_synthetic_rails() {
        let report = run_swarm(SwarmConfig::default()).unwrap();
        assert!(report.conserved);
        assert_eq!(report.chargeback_after_btc, 0);
    }

    #[test]
    fn best_price_selects_lowest_price_then_reputation() {
        let sellers = vec![
            Seller {
                fiat_cents: 0,
                btc_sats: 1_000_000,
                reputation: 80_000,
                behavior: SellerBehavior::Honest,
                rail: Rail::SimulatedIrreversible,
            },
            Seller {
                fiat_cents: 0,
                btc_sats: 1_000_000,
                reputation: 16_000,
                behavior: SellerBehavior::Honest,
                rail: Rail::SimulatedIrreversible,
            },
        ];
        let buyers = vec![Buyer {
            fiat_cents: 10_000,
            btc_sats: 0,
            behavior: BuyerBehavior::Honest,
        }];
        let book = vec![
            Listing {
                id: 1,
                seller_idx: 0,
                created_round: 0,
                quote_cents: 4_000,
                btc_sats: 80_000,
                price_cents: 5_000_000,
                rail: Rail::SimulatedIrreversible,
                active: true,
            },
            Listing {
                id: 2,
                seller_idx: 1,
                created_round: 0,
                quote_cents: 4_000,
                btc_sats: 81_633,
                price_cents: 4_900_000,
                rail: Rail::SimulatedIrreversible,
                active: true,
            },
        ];
        assert_eq!(
            choose_listing(&book, &buyers, &sellers, 0, ChoicePolicy::BestPrice),
            Some(1)
        );
        assert_eq!(
            choose_listing(&book, &buyers, &sellers, 0, ChoicePolicy::ReputationFirst),
            Some(0)
        );
    }

    #[test]
    fn active_book_can_never_match_same_offer_twice() {
        let report = run_swarm(SwarmConfig {
            seed: 99,
            buyers: 3,
            sellers: 2,
            rounds: 200,
            attacker_percent: 100,
            policy: ChoicePolicy::BestPrice,
        })
        .unwrap();
        let ids: BTreeSet<_> = report.trades.iter().map(|t| t.offer_id).collect();
        assert_eq!(ids.len(), report.trades.len());
    }

    #[test]
    fn cli_bounds_reject_unreasonable_runs() {
        assert!(SwarmConfig {
            rounds: 0,
            ..SwarmConfig::default()
        }
        .validate()
        .is_err());
        assert!(SwarmConfig {
            attacker_percent: 101,
            ..SwarmConfig::default()
        }
        .validate()
        .is_err());
        let options = vec![
            "--seed".to_string(),
            "42".to_string(),
            "--policy".to_string(),
            "price".to_string(),
            "--trace".to_string(),
        ];
        let (config, trace) = SwarmConfig::from_args(&options).unwrap();
        assert_eq!(config.seed, 42);
        assert_eq!(config.policy, ChoicePolicy::BestPrice);
        assert!(trace);
        assert!(SwarmConfig::from_args(&["--seed".to_string()]).is_err());
    }

    #[test]
    fn honest_trade_and_fraud_detection_use_same_engine() {
        let offer = Offer::default();
        let good = execute_trade(offer, BuyerBehavior::Honest, SellerBehavior::Honest).unwrap();
        assert!(good.node.audit().is_empty());
        assert_eq!(good.node.state.phase, EasyPhase::Completed);
        let withhold =
            execute_trade(offer, BuyerBehavior::Honest, SellerBehavior::WithholdBtc).unwrap();
        assert_eq!(withhold.node.state.phase, EasyPhase::Disputed);
        assert_eq!(withhold.node.audit(), vec![EasyFault::SellerNonDelivery]);
        let false_claim = execute_trade(
            offer,
            BuyerBehavior::FalsePaymentClaim,
            SellerBehavior::Honest,
        )
        .unwrap();
        assert_eq!(false_claim.node.state.seller_fiat_cents, 0);
        assert_eq!(false_claim.blocked, 1);
    }
}
